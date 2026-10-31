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
use tauri::State;

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
}

#[tauri::command]
pub async fn crash_ai_status(state: State<'_, CrashAiState>) -> Result<CrashAiStatus, String> {
    let config = state.config.clone();
    let hardware = tauri::async_runtime::spawn_blocking(crash_ai::hardware::detect)
        .await
        .unwrap_or_default();
    let feasibility = crash_ai::feasibility::assess_llm(
        &hardware,
        config.parameters_b,
        config.context_tokens,
    );
    match crash_ai::llm::probe(&config).await {
        Ok(_) => Ok(CrashAiStatus {
            ok: true,
            model: config.model.clone(),
            endpoint: config.base_url.clone(),
            error: None,
            feasibility,
        }),
        Err(error) => Ok(CrashAiStatus {
            ok: false,
            model: config.model.clone(),
            endpoint: config.base_url.clone(),
            error: Some(error.to_string()),
            feasibility,
        }),
    }
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
    let want_model = use_model.unwrap_or(true) && tripped.is_none();

    let mut analysis = crash_ai::analyse_with_status(
        &log,
        &environment,
        &config,
        want_model,
        Some(assessment.clone()),
    )
    .await;

    if let Some(reason) = &tripped {
        analysis.warnings.retain(|warning| !warning.contains("按请求"));
        analysis
            .warnings
            .push(format!("本地模型已自动停用（{reason}）。恢复模型服务后重启启动器即可重新启用。"));
    }

    /*
     * 记录结果。只在「本该用模型」时统计：被硬件判断拦下的不算失败，
     * 否则一台内存不足的机器会把熔断也点上，而真正的原因已经由 `modelStatus` 说明。
     */
    if want_model && !assessment.disable_model
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
        let Ok(metadata) = entry.metadata() else { continue };
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
        if newest.as_ref().is_none_or(|(current, _)| modified > *current) {
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
    let content = std::fs::read_to_string(&path).map_err(|error| format!("无法读取崩溃报告：{error}"))?;

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
            collect_reports(&instance.game_dir.join("crash-reports"), &instance.name, &mut reports);
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
        let Ok(entries) = std::fs::read_dir(&versions) else { continue };
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
    let Ok(entries) = std::fs::read_dir(directory) else { return };
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
        let Ok(metadata) = entry.metadata() else { continue };
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
