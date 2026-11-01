//! 崩溃 AI 分析的 Tauri 命令。
//!
//! ## 为什么在应用里直接调用库，而不是转发给独立服务
//!
//! 独立服务（`crates/crash-ai/src/bin/server.rs`）的价值是让人在浏览器里迭代
//! 分析结果的呈现。到了桌面版，多一层 HTTP 就多一层失败点：端口被占、服务没起、
//! 防火墙拦截。所以这里直接 `crash_ai::analyse`，与那台服务是同一份代码。
//!
//! 模型本身仍然是外部进程（llama.cpp 服务）。启动器不内嵌推理引擎：一个 1.7B 的
//! 量化模型有好几套运行时，把其中一套塞进安装包会让体积翻倍且难以替换。模型不可
//! 用时，规则结论照常返回——这也是「模型只是补充」这个设计的直接好处。

use crash_ai::{Analysis, Environment, llm::LlmConfig};
use std::sync::Arc;
use tauri::{Emitter, Manager, State};

/// 分析服务在应用里的状态。
pub struct CrashAiState {
    config: Arc<LlmConfig>,
    /// 运行时健康状态。连续失败后自动停用模型，与独立服务同一套逻辑。
    health: std::sync::Arc<std::sync::Mutex<crash_ai::feasibility::LlmHealth>>,
}

impl Default for CrashAiState {
    fn default() -> Self {
        Self {
            config: Arc::new(LlmConfig::default()),
            health: std::sync::Arc::new(std::sync::Mutex::new(
                crash_ai::feasibility::LlmHealth::new(),
            )),
        }
    }
}

/// 在需要模型时确保推理引擎在跑。
///
/// 返回 `None` 表示引擎可用（现成的或刚拉起来的）；`Some(原因)` 表示不可用，
/// 调用方应当把原因作为一条警告附在分析结果里——规则结论照常给出。
///
/// 只有「引擎不可用」才返回原因。硬件判断说跑不动的机器不会走到这里（那一步在
/// 调用之前就短路了），所以不会出现「给一台 2 GB 的机器拉起 1.8 GB 的引擎」。
async fn ensure_engine(
    app: &tauri::AppHandle,
    config: &crash_ai::llm::LlmConfig,
) -> Option<String> {
    let Some(runtime) = app.try_state::<crate::llama_runtime::LlamaRuntime>() else {
        return Some("推理引擎状态不可用。".to_owned());
    };
    match runtime.ensure_running(app, config).await {
        Ok(()) => None,
        Err(reason) => Some(reason),
    }
}

/// 模型是否可用，以及它在哪。
///
/// 界面需要先知道这件事，才能把「模型未连接」说在前面，而不是在用户点了分析之后
/// 才给一句失败提示。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashAiStatus {
    pub ok: bool,
    pub model: String,
    pub endpoint: String,
    #[serde(default)]
    pub error: Option<String>,
    /// 这台机器能不能跑模型。
    ///
    /// 与 `ok` 是两件事：`ok` 是「模型服务在不在」，这个字段是「本机跑不跑得动」。
    /// 界面必须区分，否则用户会去给一台内存不足的机器找模型服务。
    pub feasibility: crash_ai::feasibility::ModelAssessment,
    /// 推理引擎能不能被启动（找到可执行文件、模型文件在位）。
    ///
    /// 界面用它在「模型未下载」之外再区分三种情况：引擎可用（会在需要时自动启动）、
    /// 引擎缺失（要用户指定路径）、硬件跑不动（不必折腾）。三者的修复动作完全不同。
    pub runtime: crate::llama_runtime::LlamaRuntimeStatus,
    /// 模型文件是否已下载。
    pub model_installed: bool,
}

#[tauri::command]
pub async fn crash_ai_status(
    app: tauri::AppHandle,
    state: State<'_, CrashAiState>,
) -> Result<CrashAiStatus, String> {
    let config = state.config.clone();
    let hardware = tauri::async_runtime::spawn_blocking(crash_ai::hardware::detect)
        .await
        .unwrap_or_default();
    let feasibility =
        crash_ai::feasibility::assess_llm(&hardware, config.parameters_b, config.context_tokens);
    /*
     * 引擎状态是「能不能启动」，与 `ok`（现在是否已经在跑）不同。
     *
     * 界面需要提前知道这件事：如果引擎可用，模型卡片就不该再说「请自己启动它」；
     * 如果不可用，也要在用户点分析之前就告诉他缺什么。
     */
    let runtime_status = match app.try_state::<crate::llama_runtime::LlamaRuntime>() {
        Some(runtime) => crate::llama_runtime::status(&app, &runtime).await,
        None => crate::llama_runtime::LlamaRuntimeStatus {
            available: false,
            path: None,
            source: None,
            managed_running: false,
            problem: Some("推理引擎状态不可用。".to_owned()),
        },
    };
    let model_installed = crate::crash_ai_model::installed_model_path(&app).is_some();

    let probe = crash_ai::llm::probe(&config).await;
    Ok(CrashAiStatus {
        ok: probe.is_ok(),
        model: config.model.clone(),
        endpoint: config.base_url.clone(),
        error: probe.err().map(|error| error.to_string()),
        feasibility,
        runtime: runtime_status,
        model_installed,
    })
}

/// 停止由启动器拉起的推理引擎。
///
/// 「即用即启」的另一半：用户不该只能靠等待空闲回收来拿回那 1.8 GB。设置页给一个
/// 明确的「立即停止」，比让他杀掉进程更容易理解。
#[tauri::command]
pub async fn crash_ai_stop_engine(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(runtime) = app.try_state::<crate::llama_runtime::LlamaRuntime>() {
        runtime.shutdown().await;
    }
    Ok(())
}

/// 分析一份崩溃日志。
///
/// `use_model = false` 时只跑规则。界面用它实现「只看规则」的对比开关，也是模型
/// 不可用时的自动回退路径。
///
/// 这个命令**不读文件**：日志内容由调用方读好再传进来。理由是崩溃报告可能来自
/// 实例目录之外（用户从论坛复制的），让后端按路径读取会把「能不能读任意路径」
/// 这件事变成一个额外需要防守的攻击面。
#[tauri::command]
pub async fn crash_ai_analyse(
    log: String,
    environment: Option<Environment>,
    use_model: Option<bool>,
    app: tauri::AppHandle,
    state: State<'_, CrashAiState>,
) -> Result<Analysis, String> {
    if log.trim().is_empty() {
        return Err("日志为空。".into());
    }
    let config = state.config.clone();
    let mut environment = environment.unwrap_or_default();

    /*
     * 补齐硬件信息。
     *
     * 桌面版是唯一能拿到真实机器数据的地方，所以硬件判断必须在这里补，而不是
     * 让界面或模型去猜。只在调用方没提供时补——实例级的信息（磁盘剩余空间）
     * 由调用方填得更准。
     *
     * 探测走 `spawn_blocking`：它会读 `lspci` 与 sysinfo，是阻塞操作，不该占用
     * 异步运行时的工作线程。
     */
    if environment.hardware.is_none() {
        environment.hardware = tauri::async_runtime::spawn_blocking(crash_ai::hardware::detect)
            .await
            .ok();
    }

    /*
     * 可行性判断。与独立服务同一套规则：机器跑不动就直接不用模型，
     * 不要让用户等一次注定失败的超时。
     */
    let hardware_for_status = environment.hardware.clone().unwrap_or_default();
    let assessment = crash_ai::feasibility::assess_llm(
        &hardware_for_status,
        config.parameters_b,
        config.context_tokens,
    );

    /*
     * 熔断状态：已停用则不再请求模型。
     *
     * 读一次快照后立即释放锁，不跨 await 持锁——否则一次慢请求会阻塞其他分析的
     * 健康检查。
     */
    let tripped = state
        .health
        .lock()
        .ok()
        .and_then(|health| health.disabled_reason().map(str::to_owned));
    let mut want_model = use_model.unwrap_or(true) && tripped.is_none();
    /*
     * 需要模型时先把引擎拉起来。
     *
     * 只在「硬件跑得动」时做：给一台内存不足的机器启动 1.8 GB 的引擎，会把系统
     * 直接拖进换页，而规则结论本来 4 毫秒就能给出。不可用的原因作为一条警告附上，
     * 规则部分照常返回——模型是增强，不是前提。
     */
    let mut engine_problem: Option<String> = None;
    if want_model
        && !assessment.disable_model
        && let Some(reason) = ensure_engine(&app, &config).await
    {
        engine_problem = Some(reason);
        want_model = false;
    }

    let mut analysis = crash_ai::analyse_with_status(
        &log,
        &environment,
        &config,
        want_model,
        Some(assessment.clone()),
    )
    .await;

    if let Some(reason) = engine_problem {
        analysis
            .warnings
            .push(format!("本地推理引擎未就绪：{reason}"));
    }
    if let Some(reason) = &tripped {
        analysis
            .warnings
            .retain(|warning| !warning.contains("按请求"));
        analysis.warnings.push(format!(
            "本地模型已自动停用（{reason}）。恢复模型服务后重启启动器即可重新启用。"
        ));
    }

    /*
     * 记录结果。只在「本该用模型」时统计：被硬件判断拦下的不算失败，
     * 否则一台内存不足的机器会把熔断也点上，而真正的原因已经由 `modelStatus` 说明。
     */
    if want_model
        && !assessment.disable_model
        && let Ok(mut health) = state.health.lock()
    {
        if analysis.model_used {
            health.record_success();
        } else {
            let reason = analysis
                .warnings
                .last()
                .cloned()
                .unwrap_or_else(|| "未知原因".to_owned());
            health.record_failure(&reason);
        }
    }

    Ok(analysis)
}

/// 读取一个实例最近的崩溃报告，供一键分析。
///
/// 只找启动器自己管理的目录（`<game>/crash-reports`），不接受任意路径。这样
/// 「分析这个实例的崩溃」始终是安全的，也不会因为用户输错路径而失败。
#[tauri::command]
pub async fn crash_ai_instance_report(
    instance_id: String,
    store: State<'_, launcher_core::LauncherStore>,
) -> Result<InstanceReport, String> {
    let instance = store
        .list_instances()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|item| item.id == instance_id)
        .ok_or("找不到所选实例。")?;

    let directory = instance.game_dir.join("crash-reports");
    if !directory.is_dir() {
        return Ok(InstanceReport {
            found: false,
            name: None,
            content: None,
        });
    }

    /*
     * 取最新的一份。
     *
     * 按修改时间排序而不是文件名：crash-report 的文件名是时间戳，但用户可能
     * 手工改过名字。修改时间才是「最近一次崩溃」的事实。
     */
    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    let entries = std::fs::read_dir(&directory).map_err(|error| error.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("txt") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
        if newest
            .as_ref()
            .is_none_or(|(current, _)| modified > *current)
        {
            newest = Some((modified, path));
        }
    }

    let Some((_, path)) = newest else {
        return Ok(InstanceReport {
            found: false,
            name: None,
            content: None,
        });
    };

    /*
     * 崩溃报告可能很大（含完整模组列表与日志尾部）。只读到上限为止，剩下的由
     * 分析器自己处理截断——它比这里更清楚哪些部分重要。
     */
    let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
    if metadata.len() > crash_ai::MAX_LOG_BYTES as u64 * 2 {
        return Err("崩溃报告过大，无法读取。".into());
    }
    let content =
        std::fs::read_to_string(&path).map_err(|error| format!("无法读取崩溃报告：{error}"))?;

    Ok(InstanceReport {
        found: true,
        name: Some(
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("crash-report.txt")
                .to_owned(),
        ),
        content: Some(content),
    })
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceReport {
    pub found: bool,
    pub name: Option<String>,
    pub content: Option<String>,
}

/// 列出可分析的本机崩溃报告。
///
/// ## 为什么把扫描放在后端
///
/// 浏览器预览没有文件系统访问权，而桌面版可以。把这个能力放在命令里，界面在两种
/// 环境下行为一致：能拿到列表就显示选择器，拿不到就让用户自己粘贴。
///
/// 只扫允许的位置——实例的 `crash-reports` 与 Minecraft 目录下的 `versions`。
/// 不接受任意路径参数：那会让这个命令变成「读任意文件」的入口。
#[tauri::command]
pub async fn crash_ai_local_reports(
    store: State<'_, launcher_core::LauncherStore>,
    extra_roots: Option<Vec<String>>,
) -> Result<Vec<LocalReport>, String> {
    let mut reports: Vec<LocalReport> = Vec::new();

    // 1. 启动器管理的实例。
    if let Ok(instances) = store.list_instances() {
        for instance in instances {
            collect_reports(
                &instance.game_dir.join("crash-reports"),
                &instance.name,
                &mut reports,
            );
        }
    }

    /*
     * 2. 额外根目录。
     *
     * 用户在设置里可能指定了一个自己管理的 `.minecraft`。传进来的路径同样只用于
     * 拼 versions 下的 crash-reports，不会被当作任意文件路径打开。
     */
    for root in extra_roots.unwrap_or_default().into_iter().take(4) {
        let versions = std::path::PathBuf::from(&root).join("versions");
        let Ok(entries) = std::fs::read_dir(&versions) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            collect_reports(&entry.path().join("crash-reports"), &name, &mut reports);
        }
    }

    // 最近修改的排前面。
    reports.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(reports)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalReport {
    pub instance: String,
    pub name: String,
    pub path: String,
    pub bytes: u64,
    pub modified: u64,
}

fn collect_reports(directory: &std::path::Path, instance: &str, out: &mut Vec<LocalReport>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("txt") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_owned();
        if !name.starts_with("crash-") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        out.push(LocalReport {
            instance: instance.to_owned(),
            name,
            path: path.to_string_lossy().into_owned(),
            bytes: metadata.len(),
            modified,
        });
    }
}

/// 流式分析的阶段事件（`crash-ai-progress`）。
///
/// ## 为什么桌面版也要流式
///
/// 独立页面（`crates/crash-ai/web`）早就走 SSE，规则结论先到、模型文字逐段增长。
/// 桌面版此前只有一个阻塞命令：点下分析后，界面在 6–13 秒里什么都拿不到，用户
/// 无法区分「正在读日志」和「卡死了」。模型跑在 CPU 上，这个等待是真实存在的，
/// 不该靠一个转圈图标糊过去。
///
/// 这里的做法与独立页面一致：规则部分（毫秒级）先推一次完整结果，模型部分只推
/// **增量文字**。不需要把最终结果拆成多份 JSON——前端拼好增量就能显示，而规则
/// 结果只在第一帧出现一次。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum CrashAiProgress {
    /// 规则与硬件判断完成。`analysis` 与最终返回值的规则部分相同。
    Rules { analysis: Analysis },
    /// 模型正在生成。`text` 是**本次新增**的文字片段，不是累积值。
    ///
    /// 推增量而不是累积值：累积值会让每帧的载荷随回答长度线性增长（2 KB 的回答
    /// 要传 O(n²) 字节），而前端本来就持有已生成的那份。
    Chunk { text: String },
    /// 模型阶段结束（成功或失败）。`reason` 只在失败时非空。
    ModelDone { reason: Option<String> },
}

/// 流式分析。
///
/// 与 `crash_ai_analyse` 是同一套判断，唯一区别是把中间过程通过事件推出去。
/// 拆成两个命令而不是给旧命令加一个开关：旧命令的返回值仍是完整结果，调用方
/// （如果有的话）不用改；新命令可以自由定义事件序列。
#[tauri::command]
pub async fn crash_ai_analyse_stream(
    log: String,
    environment: Option<Environment>,
    use_model: Option<bool>,
    app: tauri::AppHandle,
    state: State<'_, CrashAiState>,
) -> Result<Analysis, String> {
    if log.trim().is_empty() {
        return Err("日志为空。".into());
    }
    let config = state.config.clone();
    let mut environment = environment.unwrap_or_default();

    if environment.hardware.is_none() {
        environment.hardware = tauri::async_runtime::spawn_blocking(crash_ai::hardware::detect)
            .await
            .ok();
    }

    let hardware_for_status = environment.hardware.clone().unwrap_or_default();
    let assessment = crash_ai::feasibility::assess_llm(
        &hardware_for_status,
        config.parameters_b,
        config.context_tokens,
    );

    let tripped = state
        .health
        .lock()
        .ok()
        .and_then(|health| health.disabled_reason().map(str::to_owned));
    let want_model = use_model.unwrap_or(true) && tripped.is_none();

    /*
     * 拉起引擎（需要模型且硬件跑得动时）。
     *
     * 放在「规则部分推出去」之前而不是之后：启动要 10–20 秒，如果先推规则再启
     * 引擎，用户会看到规则结论后干等二十秒才开始出现模型文字——而实际上那段时间
     * 引擎还在加载，界面只能显示「正在阅读日志」。先启动、再推规则，两者就能
     * 重叠：用户读规则的时间正是引擎加载的时间。
     */
    let mut engine_problem: Option<String> = None;
    let mut want_model = want_model;
    if want_model
        && !assessment.disable_model
        && let Some(reason) = ensure_engine(&app, &config).await
    {
        engine_problem = Some(reason);
        want_model = false;
    }

    /*
     * 本地部分先算完并**立即**推出去。
     *
     * 这一步是 4–14 毫秒，用户几乎瞬时就能看到「日志里读出了什么」。把它推迟到
     * 模型结束之后，等于白白让最有用的一半等了十几秒。
     */
    let (findings, cause, capability, facts, mut warnings) =
        crash_ai::analyse_local(&log, &environment);

    let blocked_by_hardware = assessment.disable_model;
    if blocked_by_hardware {
        warnings.push(format!("已停用本地模型：{}", assessment.summary));
    } else if let Some(reason) = &engine_problem {
        warnings.push(format!("本地推理引擎未就绪：{reason}"));
    } else if !want_model {
        warnings.push(if tripped.is_some() {
            "本地模型已自动停用".to_owned()
        } else {
            "已跳过模型分析（按请求）".to_owned()
        });
    }

    let mut analysis = Analysis {
        findings,
        cause,
        capability,
        model_status: Some(assessment.clone()),
        narrative: None,
        facts,
        model_used: false,
        warnings,
    };
    // 规则结果先到：前端此刻就能渲染原因与建议。
    let _ = app.emit(
        "crash-ai-progress",
        CrashAiProgress::Rules {
            analysis: analysis.clone(),
        },
    );

    if blocked_by_hardware || !want_model {
        let reason = if blocked_by_hardware {
            Some(assessment.summary.clone())
        } else {
            None
        };
        let _ = app.emit("crash-ai-progress", CrashAiProgress::ModelDone { reason });
        return Ok(analysis);
    }

    /*
     * 模型阶段。
     *
     * 用 `spawn_blocking` + 一个转发通道，而不是直接在异步任务里回调 `emit`：
     * 生成器是异步的，但 `StreamHandler` 是同步闭包，两者之间需要一个缓冲。
     * 通道同时把「模型线程」和「事件线程」解耦——一个慢的 webview 不会拖慢生成。
     */
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Result<String, String>>();
    let emit_app = app.clone();
    let forward = tauri::async_runtime::spawn(async move {
        while let Some(message) = rx.recv().await {
            match message {
                Ok(text) => {
                    let _ = emit_app.emit("crash-ai-progress", CrashAiProgress::Chunk { text });
                }
                Err(reason) => {
                    let _ = emit_app.emit(
                        "crash-ai-progress",
                        CrashAiProgress::ModelDone {
                            reason: Some(reason),
                        },
                    );
                }
            }
        }
    });

    let facts_for_model = analysis.facts.clone();
    let findings_for_model = analysis.findings.clone();
    let cause_for_model = analysis.cause.clone();
    let environment_for_model = environment.clone();
    let config_for_model = config.clone();
    let sender = tx.clone();
    let narrated = tauri::async_runtime::spawn_blocking(move || {
        tauri::async_runtime::block_on(async move {
            let mut handler = |chunk: &str| {
                let _ = sender.send(Ok(chunk.to_owned()));
            };
            crash_ai::llm::narrate_streaming(
                &config_for_model,
                &facts_for_model,
                &findings_for_model,
                cause_for_model.as_ref(),
                &environment_for_model,
                &mut handler,
            )
            .await
        })
    })
    .await;

    match narrated {
        Ok(Ok(narrative)) => {
            analysis.narrative = Some(narrative);
            analysis.model_used = true;
            let _ = app.emit(
                "crash-ai-progress",
                CrashAiProgress::ModelDone { reason: None },
            );
        }
        Ok(Err(error)) => {
            let reason = error.to_string();
            analysis.warnings.push(reason.clone());
            let _ = app.emit(
                "crash-ai-progress",
                CrashAiProgress::ModelDone {
                    reason: Some(reason),
                },
            );
        }
        Err(error) => {
            let reason = format!("模型任务失败：{error}");
            analysis.warnings.push(reason.clone());
            let _ = app.emit(
                "crash-ai-progress",
                CrashAiProgress::ModelDone {
                    reason: Some(reason),
                },
            );
        }
    }
    drop(tx);
    let _ = forward.await;

    if !assessment.disable_model
        && let Ok(mut health) = state.health.lock()
    {
        if analysis.model_used {
            health.record_success();
        } else {
            let reason = analysis
                .warnings
                .last()
                .cloned()
                .unwrap_or_else(|| "未知原因".to_owned());
            health.record_failure(&reason);
        }
    }

    Ok(analysis)
}
