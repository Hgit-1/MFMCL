//! 本地推理引擎（llama.cpp 的 `llama-server`）的解析与按需启动。
//!
//! ## 为什么需要「即用即启」
//!
//! 之前启动器只下载模型文件，然后要求用户自己在终端里把 `llama-server` 跑起来。
//! 这对开发者不算事，对玩家等于「功能不存在」：他下载了 1.1 GB，点「分析崩溃」，
//! 得到的却是「模型未连接」。界面上那句免责声明说的是实话，但一个需要读文档才能
//! 用起来的功能，实际等于没有。
//!
//! 所以这里把引擎的生命周期接过来：
//!   - 需要分析时**先探测**，端口上已经有服务就直接用（可能是用户自己起的）；
//!   - 没有就**拉起来**，等它把模型加载完再开始分析；
//!   - 分析完不立刻关，闲置一段时间后**自动回收**（模型常驻 1.8 GB，不能一直占着）；
//!   - 应用退出时一并结束。
//!
//! ## 为什么不把二进制塞进安装包
//!
//! llama.cpp 的发布物按平台 × 加速后端（CPU / CUDA / Vulkan / Metal）分叉，每种
//! 都上百 MB。全带上会让安装包翻几倍，而只带 CPU 版则会让有显卡的用户白白慢几倍。
//! 所以这里只做「解析与启动」，二进制从哪来由调用方决定：
//!
//!   1. 用户在设置里指定（最可控，也用于自编译版本）；
//!   2. `MFMCL_LLAMA_SERVER` 环境变量（便于脚本与排障）；
//!   3. 构建附带的 `resources/llama/<os>/<arch>/llama-server`——必须与
//!      `llama-manifest.json` 里的 SHA-256 一致（与 aria2 同一条规则：没有 pin 的
//!      二进制绝不执行）；
//!   4. 应用数据目录的 `runtimes/llama/`（预留给将来「像模型一样下载引擎」）；
//!   5. `PATH` 上的 `llama-server`（用户自己装过的情况）。
//!
//! ## 版本兼容是一个真实失败模式
//!
//! LFM3 用较新的 `lfm3` 架构，较早的 llama.cpp 构建会直接拒绝加载模型。这类失败
//! 发生在引擎这一侧，界面上只能看到「等了很久然后超时」。所以启动失败时会把引擎
//! 自己输出的最后几行带上——「unknown model architecture: lfm3」比「启动超时」
//! 有用得多。

use crate::crash_ai_model;
use crash_ai::llm::LlmConfig;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// 启动后等待服务就绪的上限。
///
/// 1.1 GB 的模型在机械硬盘上加载可能要几十秒；CPU 上还要建 KV cache。120 秒是
/// 「慢但合理」与「明显坏了」的分界。超过这个时间基本都是引擎崩了或模型不兼容，
/// 而不是还在加载。
const READY_TIMEOUT: Duration = Duration::from_secs(120);

/// 就绪检测的轮询间隔。太密会让引擎的启动日志被探测请求刷屏。
const READY_POLL: Duration = Duration::from_millis(500);

/// 闲置多久后回收引擎。
///
/// 5 分钟：用户连着分析几份日志不会反复重启（每次重启要 10–20 秒），而关掉分析页
/// 去干别的之后，1.8 GB 内存会在几分钟内还回去。
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// 引擎解析结果，供界面显示。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlamaRuntimeStatus {
    /// 能否启动引擎（找到了可执行的二进制）。
    pub available: bool,
    /// 解析到的路径。
    pub path: Option<String>,
    /// 路径来自哪：`settings` / `env` / `bundled` / `appdata` / `path`。
    pub source: Option<String>,
    /// 当前是否由启动器拉起的进程在运行。
    pub managed_running: bool,
    /// 不可用的原因。
    pub problem: Option<String>,
}

/// 托管的引擎进程。
///
/// 用 `tokio::sync::Mutex` 而不是 `std` 的：`ensure_running` 会在持锁期间 await
/// （启动 + 轮询就绪），std 的守卫不能跨 await 持有。
#[derive(Default)]
pub struct LlamaRuntime {
    inner: tokio::sync::Mutex<RuntimeInner>,
    /// 最近一次使用的时间戳（毫秒），供空闲回收判断。
    ///
    /// 单独用原子量而不是塞进 Mutex：分析过程中不需要为了「记一下时间」去抢
    /// 启动锁，而那把锁可能正被一个漫长的启动过程持有。
    last_used_ms: AtomicU64,
}

#[derive(Default)]
struct RuntimeInner {
    /// 由我们拉起的子进程。`None` 表示没有托管进程。
    child: Option<tokio::process::Child>,
}

/// 引擎日志的位置。固定路径，排障时能一句话说清。
fn engine_log_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法定位应用数据目录：{e}"))?
        .join("logs")
        .join("llama-server.log"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl LlamaRuntime {
    pub fn touch(&self) {
        self.last_used_ms.store(now_ms(), Ordering::Relaxed);
    }

    fn idle_for(&self) -> Duration {
        let last = self.last_used_ms.load(Ordering::Relaxed);
        if last == 0 {
            return Duration::ZERO;
        }
        let now = now_ms();
        Duration::from_millis(now.saturating_sub(last))
    }

    /// 结束托管进程。外部服务不受影响。
    pub async fn shutdown(&self) {
        let mut guard = self.inner.lock().await;
        if let Some(mut child) = guard.child.take() {
            // 先礼后兵：SIGTERM 让引擎有机会释放端口与显存，等不到就强杀。
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        }
    }

    /// 闲置回收。由后台任务周期调用。
    pub async fn reap_if_idle(&self) {
        let has_child = {
            let guard = self.inner.lock().await;
            guard.child.is_some()
        };
        if !has_child {
            return;
        }
        if self.idle_for() >= IDLE_TIMEOUT {
            self.shutdown().await;
        }
    }

    /// 确保引擎在运行，必要时拉起它。
    ///
    /// **幂等**：端口上已有服务（无论是否我们起的）时直接返回，不会重复启动。
    pub async fn ensure_running(&self, app: &AppHandle, config: &LlmConfig) -> Result<(), String> {
        self.touch();
        // 先探测。这一步很快（本地 HTTP），也是「复用用户自己的服务」的关键。
        if crash_ai::llm::probe(config).await.is_ok() {
            return Ok(());
        }

        let mut guard = self.inner.lock().await;

        // 拿锁之后再探测一次：可能在我们等锁的时候别的请求已经把它拉起来了。
        if crash_ai::llm::probe(config).await.is_ok() {
            return Ok(());
        }

        /*
         * 已有托管进程但探测失败：说明它崩了或还在加载。
         *
         * 先看它是否已经退出——退出了就清掉残留并重新启动；没退出就继续等，
         * 不要因为一次探测失败就杀掉一个正在加载模型的好进程。
         */
        if let Some(child) = guard.child.as_mut() {
            match child.try_wait() {
                Ok(None) => {
                    // 还活着：等它就绪即可。
                    let inner = &mut *guard;
                    let log_path = engine_log_path(app)?;
                    return wait_ready(config, inner, &log_path).await;
                }
                _ => {
                    guard.child = None;
                }
            }
        }

        /*
         * 端口不是本机回环地址时不启动。
         *
         * `base_url` 可以是远端服务（用户在另一台机器上跑推理）。那种情况下
         * 「启动本地引擎」毫无意义，而且会把一个配置问题变成一条莫名其妙的本地进程。
         */
        let (host, port) = match parse_loopback(&config.base_url) {
            Some(pair) => pair,
            None => {
                return Err(format!(
                    "模型地址 {} 不是本机回环地址，启动器不会在那里启动引擎；请确认该地址上的服务可用。",
                    config.base_url
                ));
            }
        };

        let model = crash_ai_model::installed_model_path(app).ok_or_else(|| {
            "还没有下载模型文件。请先在「设置 → 下载 → 本地 AI 模型」里下载。".to_owned()
        })?;

        let (binary, source) = resolve_binary(app)?;

        /*
         * 引擎自身的输出写到固定文件。
         *
         * 不接到应用日志里：引擎是逐 token 刷屏的，混进启动器日志会把有用信息淹掉。
         * 而启动失败时我们又需要它——所以单独一份，失败时读尾部几行附到错误里。
         */
        let log_path = engine_log_path(app)?;
        /*
         * 用 `std::fs` 而不是 `tokio::fs`：这两个句柄要交给
         * `std::process::Stdio`，而它只接受同步的 `std::fs::File`。
         * 一次打开一次克隆，用量极小，不值得为它把子进程的 IO 也改成异步。
         */
        std::fs::create_dir_all(log_path.parent().unwrap_or(&log_path)).ok();
        let log_out = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_path)
            .ok();
        let log_err = log_out.as_ref().and_then(|file| file.try_clone().ok());

        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(32);

        let mut command = tokio::process::Command::new(&binary);
        command
            .arg("-m")
            .arg(&model)
            .arg("--host")
            .arg(host)
            .arg("--port")
            .arg(port.to_string())
            .arg("-c")
            .arg(config.context_tokens.to_string())
            .arg("-t")
            .arg(threads.to_string())
            /*
             * `-np 1`：单并行槽。
             *
             * llama.cpp 默认开 4 个槽，KV cache 会翻四倍——实测 1.7B 模型因此从
             * 1.8 GB 涨到 4 GB 以上，在 8 GB 的机器上直接把系统拖进换页。启动器
             * 同一时刻只分析一份日志，不需要并发槽。
             */
            .arg("-np")
            .arg("1")
            .stdin(std::process::Stdio::null())
            .stdout(
                log_out
                    .map(std::process::Stdio::from)
                    .unwrap_or_else(std::process::Stdio::null),
            )
            .stderr(
                log_err
                    .map(std::process::Stdio::from)
                    .unwrap_or_else(std::process::Stdio::null),
            )
            .kill_on_drop(true);
        // The engine is a server, not a window: keep it off the desk.
        launcher_core::process::hide_console_async(&mut command);

        let child = command
            .spawn()
            .map_err(|error| format!("无法启动推理引擎（{binary:?}，来源 {source}）：{error}"))?;
        guard.child = Some(child);

        wait_ready(config, &mut guard, &log_path).await
    }
}

/// 等托管进程就绪。
///
/// 期间同时盯两件事：探测成功、子进程退出。后者要立刻失败——引擎因为
/// 「不认识 lfm3 架构」退出时，等满 120 秒只会让用户以为卡住了。
async fn wait_ready(
    config: &LlmConfig,
    inner: &mut RuntimeInner,
    log_path: &Path,
) -> Result<(), String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if crash_ai::llm::probe(config).await.is_ok() {
            return Ok(());
        }
        if let Some(child) = inner.child.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    inner.child = None;
                    let tail = read_log_tail(log_path).await;
                    return Err(format!("推理引擎启动后立即退出（{status}）。{tail}"));
                }
                Ok(None) => {}
                Err(error) => {
                    inner.child = None;
                    return Err(format!("无法查询推理引擎状态：{error}"));
                }
            }
        } else {
            return Err("推理引擎进程已不在运行。".into());
        }
        if Instant::now() >= deadline {
            // 超时也把日志尾巴带上：最常见的原因是模型架构不受支持。
            let tail = read_log_tail(log_path).await;
            return Err(format!(
                "推理引擎在 {} 秒内没有就绪。{tail}",
                READY_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

/// 读引擎日志的最后几行，用于错误信息。
async fn read_log_tail(path: &Path) -> String {
    let Ok(bytes) = tokio::fs::read(path).await else {
        return String::new();
    };
    // 只取尾部 4 KB：引擎的启动日志前面全是加载细节，有价值的在最后。
    let start = bytes.len().saturating_sub(4096);
    let text = String::from_utf8_lossy(&bytes[start..]);
    let tail: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if tail.is_empty() {
        String::new()
    } else {
        format!("引擎日志末尾：{}", tail.join(" / "))
    }
}

/// 从 `base_url` 取回环地址的 host 与 port。
///
/// 只接受 `127.0.0.1` / `localhost` / `[::1]`：远端地址上的引擎不该由我们启动。
fn parse_loopback(base_url: &str) -> Option<(&'static str, u16)> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let host = url.host_str()?;
    let is_loopback = host == "127.0.0.1" || host == "localhost" || host == "::1";
    if !is_loopback {
        return None;
    }
    Some(("127.0.0.1", url.port_or_known_default().unwrap_or(8099)))
}

/// 解析可执行文件路径。
///
/// 顺序即优先级，理由：
///   1. 用户显式指定最可信，也覆盖自编译版本；
///   2. 环境变量便于脚本化与排障；
///   3. 附带资源必须过哈希校验（与 aria2 同一条规则，见模块头）；
///   4. 应用数据目录留给「将来像模型一样下载引擎」；
///   5. `PATH` 兜底。
fn resolve_binary(app: &AppHandle) -> Result<(PathBuf, &'static str), String> {
    let mut tried: Vec<String> = Vec::new();

    // 1) 设置里的路径
    if let Some(store) = app.try_state::<launcher_core::LauncherStore>() {
        if let Ok(settings) = store.load_settings() {
            if let Some(path) = settings.llama_server_path.as_ref() {
                if is_executable(path) {
                    return Ok((path.clone(), "settings"));
                }
                tried.push(format!("设置中的路径不可执行：{}", path.display()));
            }
        }
    }

    // 2) 环境变量
    if let Ok(value) = std::env::var("MFMCL_LLAMA_SERVER") {
        let path = PathBuf::from(&value);
        if is_executable(&path) {
            return Ok((path, "env"));
        }
        tried.push(format!("MFMCL_LLAMA_SERVER 指向的文件不可执行：{value}"));
    }

    // 3) 附带资源（要求清单 pin）
    if let Ok(resource_dir) = app.path().resource_dir() {
        let candidates = [resource_dir
            .join("resources")
            .join("llama")
            .join(std::env::consts::OS)
            .join(std::env::consts::ARCH)
            .join(exe_name())];
        for path in candidates {
            if path.is_file() {
                match verify_manifest(&resource_dir, &path) {
                    Ok(()) => return Ok((path, "bundled")),
                    Err(problem) => {
                        tried.push(format!("附带引擎未通过校验（{problem}）"));
                    }
                }
            }
        }
    }

    // 4) 应用数据目录
    if let Ok(root) = app.path().app_data_dir() {
        let path = root.join("runtimes").join("llama").join(exe_name());
        if is_executable(&path) {
            return Ok((path, "appdata"));
        }
    }

    // 5) PATH
    if let Some(path) = find_on_path(exe_name()) {
        return Ok((path, "path"));
    }

    let detail = if tried.is_empty() {
        String::new()
    } else {
        format!("（已尝试：{}）", tried.join("；"))
    };
    Err(format!(
        "没有找到 llama-server 可执行文件。请在设置里指定它的位置，或把文件放到应用数据目录的 runtimes/llama/ 下。{detail}"
    ))
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

fn is_executable(path: &Path) -> bool {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return false,
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return meta.permissions().mode() & 0o111 != 0;
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// 校验附带引擎与清单里的 SHA-256 一致。
///
/// 清单缺失即视为「未附带」而不是「跳过校验」：没有 pin 的二进制绝不执行，这条
/// 规则与 `sidecar.rs` 完全一致。
fn verify_manifest(resource_dir: &Path, binary: &Path) -> Result<(), String> {
    let manifest_path = resource_dir.join("resources").join("llama-manifest.json");
    let bytes = std::fs::read(&manifest_path).map_err(|_| "缺少版本清单".to_owned())?;
    #[derive(serde::Deserialize)]
    struct Manifest {
        platforms: std::collections::HashMap<String, Pin>,
    }
    #[derive(serde::Deserialize)]
    struct Pin {
        sha256: String,
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| "清单无效".to_owned())?;
    let key = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let pin = manifest.platforms.get(&key).ok_or("清单未覆盖此平台")?;
    if pin.sha256.len() != 64 || !pin.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("校验值无效".into());
    }
    use sha2::Digest;
    let data = std::fs::read(binary).map_err(|_| "无法读取文件".to_owned())?;
    let actual: String = sha2::Sha256::digest(&data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != pin.sha256.to_ascii_lowercase() {
        return Err("哈希与清单不一致".into());
    }
    Ok(())
}

/// 在 `PATH` 里找可执行文件。
///
/// 自己走一遍 PATH 而不是调 `which`：Windows 上没有 `which`，而多引一个 crate
/// 只为这件事不值得。
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// 当前状态，供设置面板显示。
pub async fn status(app: &AppHandle, runtime: &LlamaRuntime) -> LlamaRuntimeStatus {
    let (available, path, source, problem) = match resolve_binary(app) {
        Ok((path, source)) => (
            true,
            Some(path.to_string_lossy().into_owned()),
            Some(source.to_owned()),
            None,
        ),
        Err(problem) => (false, None, None, Some(problem)),
    };
    let managed_running = {
        let mut guard = runtime.inner.lock().await;
        match guard.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    };
    LlamaRuntimeStatus {
        available,
        path,
        source,
        managed_running,
        problem,
    }
}

/// 启动后台的空闲回收任务。
///
/// 由 `lib.rs` 的 setup 调用一次。用固定周期轮询而不是「每次用完之后定时」：
/// 后者在应用长时间空闲时仍会留下一堆待触发的定时器，而这里只有一条循环。
pub fn spawn_idle_reaper(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Some(runtime) = app.try_state::<LlamaRuntime>() {
                runtime.reap_if_idle().await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_detection_accepts_only_local_hosts() {
        // 远端地址不该触发本地引擎启动。
        assert_eq!(
            parse_loopback("http://127.0.0.1:8099"),
            Some(("127.0.0.1", 8099))
        );
        assert_eq!(
            parse_loopback("http://localhost:9000"),
            Some(("127.0.0.1", 9000))
        );
        /*
         * 不带端口时用 URL 的默认端口（http → 80），而不是硬编码 8099。
         *
         * 端口必须与 `base_url` 完全一致：引擎起在 80 而探测 8099 的话，我们会
         * 永远等不到就绪。默认值 8099 由 `LlmConfig::default()` 提供，不在这里覆盖。
         */
        assert_eq!(parse_loopback("http://127.0.0.1"), Some(("127.0.0.1", 80)));
        assert_eq!(parse_loopback("http://192.168.1.10:8099"), None);
        assert_eq!(parse_loopback("https://llm.example.com"), None);
    }

    #[test]
    fn exe_name_matches_the_platform() {
        if cfg!(windows) {
            assert_eq!(exe_name(), "llama-server.exe");
        } else {
            assert_eq!(exe_name(), "llama-server");
        }
    }

    /// 真实拉起引擎并等到就绪。
    ///
    /// 默认忽略：需要本机的 llama-server 与模型文件。跑法：
    ///
    /// ```sh
    /// MFMCL_TEST_LLAMA_SERVER=/path/to/llama-server \
    /// MFMCL_TEST_MODEL=/path/to/model.gguf \
    /// cargo test -p mfmcl-desktop --lib auto_start -- --ignored --nocapture
    /// ```
    ///
    /// 这条测试守的是整条「即用即启」链路：命令参数是否被引擎接受、模型能否加载、
    /// 就绪探测能否在合理时间内成功。用假实现测不了——命令行的兼容性恰恰是这里
    /// 最容易随版本变化的部分。
    #[tokio::test]
    #[ignore = "需要本机 llama-server 与模型文件"]
    async fn auto_start_spawns_and_becomes_ready() {
        let binary = std::env::var("MFMCL_TEST_LLAMA_SERVER").expect("set MFMCL_TEST_LLAMA_SERVER");
        let model = std::env::var("MFMCL_TEST_MODEL").expect("set MFMCL_TEST_MODEL");
        let port: u16 = std::env::var("MFMCL_TEST_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(8131);

        let config = LlmConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            ..Default::default()
        };
        // 端口已被占用时先判定失败，避免测试「通过」其实连的是别人的服务。
        assert!(
            crash_ai::llm::probe(&config).await.is_err(),
            "端口 {port} 上已有服务，换一个 MFMCL_TEST_PORT"
        );

        let log_path = std::env::temp_dir().join(format!("mfmcl-llama-test-{port}.log"));
        let child = tokio::process::Command::new(&binary)
            .arg("-m")
            .arg(&model)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("-c")
            .arg("8192")
            .arg("-t")
            .arg("8")
            .arg("-np")
            .arg("1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn llama-server");

        let mut inner = RuntimeInner { child: Some(child) };
        let outcome = wait_ready(&config, &mut inner, &log_path).await;
        let _ = std::fs::remove_file(&log_path);
        outcome.expect("engine becomes ready");
        assert!(
            crash_ai::llm::probe(&config).await.is_ok(),
            "probe succeeds after ready"
        );
    }

    /// 引擎起不来时必须**快速失败**，而不是干等 120 秒。
    ///
    /// 最常见的原因是模型架构不受支持（较早的 llama.cpp 不认识 `lfm3`）。这种情况下
    /// 引擎会立刻退出，而用户如果只看到「等了很久然后超时」，会以为是自己机器太慢。
    ///
    /// 用一个不存在的模型路径制造必然失败——引擎会在加载阶段退出。
    #[tokio::test]
    #[ignore = "需要本机 llama-server"]
    async fn a_crashing_engine_fails_fast_with_a_reason() {
        let binary = std::env::var("MFMCL_TEST_LLAMA_SERVER").expect("set MFMCL_TEST_LLAMA_SERVER");
        let port: u16 = 8134;
        let config = LlmConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            ..Default::default()
        };
        let log_path = std::env::temp_dir().join("mfmcl-llama-crash-test.log");
        std::fs::write(&log_path, b"").ok();

        let child = tokio::process::Command::new(&binary)
            .arg("-m")
            .arg("/nonexistent/definitely-missing.gguf")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn llama-server");

        let mut inner = RuntimeInner { child: Some(child) };
        let started = Instant::now();
        let error = wait_ready(&config, &mut inner, &log_path)
            .await
            .expect_err("a broken engine must not be reported as ready");
        let elapsed = started.elapsed();
        let _ = std::fs::remove_file(&log_path);

        assert!(
            elapsed < Duration::from_secs(20),
            "应当快速失败，实际等了 {elapsed:?}"
        );
        assert!(
            error.contains("退出"),
            "错误信息应当说明引擎已退出，实际是：{error}"
        );
    }
}
