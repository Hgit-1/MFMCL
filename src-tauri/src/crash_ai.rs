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
}

impl Default for CrashAiState {
    fn default() -> Self {
        Self {
            config: Arc::new(LlmConfig::default()),
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
}

#[tauri::command]
pub async fn crash_ai_status(state: State<'_, CrashAiState>) -> Result<CrashAiStatus, String> {
    let config = state.config.clone();
    match crash_ai::llm::probe(&config).await {
        Ok(_) => Ok(CrashAiStatus {
            ok: true,
            model: config.model.clone(),
            endpoint: config.base_url.clone(),
            error: None,
        }),
        Err(error) => Ok(CrashAiStatus {
            ok: false,
            model: config.model.clone(),
            endpoint: config.base_url.clone(),
            error: Some(error.to_string()),
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
    let environment = environment.unwrap_or_default();
    Ok(crash_ai::analyse(&log, &environment, &config, use_model.unwrap_or(true)).await)
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
