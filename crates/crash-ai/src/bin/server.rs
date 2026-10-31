//! Standalone analysis service.
//!
//! Serves the web page and the analysis API on one port. Built as a separate
//! binary so the page can be opened on its own while the launcher work is not
//! ready, and so the LLM endpoint can be swapped without rebuilding the launcher.
//!
//! ## Why a hand-written HTTP server
//!
//! The workspace has no HTTP server crate and adding axum/fastapi would pull in a
//! dependency tree far larger than this service. What is actually needed is:
//! three routes, JSON in, JSON out, static files. That is ~200 lines over tokio.
//!
//! The parsing is deliberately minimal but not sloppy: request line, headers,
//! `Content-Length` body, and a hard cap on body size. Anything it does not
//! understand gets a clear 4xx rather than a confusing hang.

use crash_ai::{Environment, llm::LlmConfig};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Bodies larger than this are refused. A crash log is text; 32 MB is far past
/// any real log and keeps a malformed request from exhausting memory.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

const WEB_INDEX: &str = include_str!("../../web/index.html");
const WEB_APP_JS: &str = include_str!("../../web/app.js");
const WEB_STYLE: &str = include_str!("../../web/style.css");

#[derive(Clone)]
struct AppState {
    config: Arc<LlmConfig>,
    /**
     * 模型的运行时健康状态。
     *
     * 用 `Mutex` 而不是原子变量：熔断需要「连续失败计数 + 原因文本」一起更新，
     * 分开的原子操作会让两个请求交错时记错次数。
     */
    health: Arc<std::sync::Mutex<crash_ai::feasibility::LlmHealth>>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::var("CRASH_AI_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8790);
    let host = std::env::var("CRASH_AI_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned());

    let config = LlmConfig::default();
    let address = format!("{host}:{port}");

    /*
     * Print the model endpoint at startup.
     *
     * The most common failure is "the page loaded but analysis says the model is
     * down" — usually because the llama.cpp server is on another port. Showing
     * what we will try to reach makes that a one-glance diagnosis.
     */
    println!("crash-ai listening on http://{address}");
    println!("model endpoint: {} ({})", config.base_url, config.model);

    let state = Arc::new(AppState {
        config: Arc::new(config),
        health: Arc::new(std::sync::Mutex::new(crash_ai::feasibility::LlmHealth::new())),
    });
    let listener = TcpListener::bind(&address).await?;
    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(value) => value,
            Err(error) => {
                eprintln!("accept failed: {error}");
                continue;
            }
        };
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = handle(stream, state).await {
                eprintln!("connection error: {error}");
            }
        });
    }
}

async fn handle(mut stream: TcpStream, state: Arc<AppState>) -> std::io::Result<()> {
    let Some(request) = read_request(&mut stream).await? else {
        return Ok(());
    };

    let response = route(&request, &state).await;
    write_response(&mut stream, response).await
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    extra_headers: Vec<(&'static str, String)>,
}

impl Response {
    fn json(status: u16, value: serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec()),
            extra_headers: Vec::new(),
        }
    }

    fn text(status: u16, content_type: &'static str, body: &str) -> Self {
        Self {
            status,
            content_type,
            body: body.as_bytes().to_vec(),
            extra_headers: Vec::new(),
        }
    }

    fn error(status: u16, message: &str) -> Self {
        Self::json(status, serde_json::json!({ "error": message }))
    }
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buffer = Vec::with_capacity(8192);
    let mut chunk = [0u8; 4096];

    // Read until the headers are complete. A request that never sends a blank
    // line is dropped rather than buffered forever.
    let header_end = loop {
        if let Some(position) = find_header_end(&buffer) {
            break position;
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > 64 * 1024 {
            return Ok(None);
        }
    };

    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let Some(request_line) = lines.next() else {
        return Ok(None);
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("/").to_owned();

    let mut headers: HashMap<String, String> = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }

    let length: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if length > MAX_BODY_BYTES {
        return Ok(None);
    }

    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(length);

    Ok(Some(Request { method, path, body }))
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

async fn write_response(stream: &mut TcpStream, response: Response) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        status_text(response.status),
        response.content_type,
        response.body.len()
    );
    for (name, value) in &response.extra_headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&response.body).await?;
    stream.flush().await
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

async fn route(request: &Request, state: &AppState) -> Response {
    // Strip the query string: nothing here uses one, and keeping it would make
    // every match below fragile.
    let path = request.path.split('?').next().unwrap_or("/");

    match (request.method.as_str(), path) {
        ("GET", "/") | ("GET", "/index.html") => {
            Response::text(200, "text/html; charset=utf-8", WEB_INDEX)
        }
        ("GET", "/app.js") => Response::text(200, "text/javascript; charset=utf-8", WEB_APP_JS),
        ("GET", "/style.css") => Response::text(200, "text/css; charset=utf-8", WEB_STYLE),
        ("GET", "/api/health") => health(state).await,
        ("GET", "/api/sources") => sources().await,
        ("POST", "/api/read") => read_controlled(request).await,
        ("POST", "/api/analyse") => analyse(request, state).await,
        (_, "/api/analyse") => Response::error(405, "只接受 POST"),
        _ => Response::error(404, "没有这个地址"),
    }
}

/// Report whether the model is reachable.
///
/// The page calls this on load so it can state "model offline" up front instead
/// of failing on the first analysis.
async fn health(state: &AppState) -> Response {
    /*
     * 把「能不能跑模型」与「模型现在在不在」一起报告。
     *
     * 两者是不同的失败：前者是机器不够，后者是服务没启动。界面上必须能区分，
     * 否则用户会去修错的東西（给一台 2 GB 的机器找模型服务）。
     */
    let hardware = tauri_hardware();
    let assessment = crash_ai::feasibility::assess_llm(
        &hardware,
        state.config.parameters_b,
        state.config.context_tokens,
    );
    let tripped = state
        .health
        .lock()
        .ok()
        .and_then(|health| health.disabled_reason().map(str::to_owned));

    match crash_ai::llm::probe(&state.config).await {
        Ok(models) => Response::json(
            200,
            serde_json::json!({
                "ok": true,
                "model": state.config.model,
                "endpoint": state.config.base_url,
                "available": models,
                "feasibility": assessment,
                "disabledReason": tripped,
            }),
        ),
        Err(error) => Response::json(
            200,
            serde_json::json!({
                "ok": false,
                "model": state.config.model,
                "endpoint": state.config.base_url,
                "error": error.to_string(),
                "feasibility": assessment,
                "disabledReason": tripped,
            }),
        ),
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnalyseRequest {
    log: String,
    #[serde(default)]
    environment: Environment,
    /// Set false to get a rules-only result. The page exposes this as a switch so
    /// a user can see exactly what the model added.
    #[serde(default = "default_true")]
    use_model: bool,
}

fn default_true() -> bool {
    true
}

async fn analyse(request: &Request, state: &AppState) -> Response {
    if request.body.len() > MAX_BODY_BYTES {
        return Response::error(413, "日志过大");
    }
    let parsed: AnalyseRequest = match serde_json::from_slice(&request.body) {
        Ok(value) => value,
        Err(error) => return Response::error(400, &format!("请求格式无效：{error}")),
    };
    if parsed.log.trim().is_empty() {
        return Response::error(400, "日志为空");
    }

    /*
     * 补齐硬件信息。
     *
     * 调用方（页面）拿不到本机硬件，所以由服务自己探测。这也让「资源不足」的
     * 判断有真实依据——而不是让模型去猜。
     *
     * 只在调用方没有提供时补：桌面版会把更精确的硬件信息（含实例所在磁盘空间）
     * 一起传进来，那种情况不该被这里的探测覆盖。
     */
    let mut environment = parsed.environment;
    if environment.hardware.is_none() {
        environment.hardware = Some(crash_ai::hardware::detect());
    }

    /*
     * 可行性判断。
     *
     * 放在调用模型之前，因为结论决定**要不要**调用。判断本身是 CPU + 文件读取，
     * 放到 blocking 池里跑，避免阻塞异步运行时。
     */
    let config_for_status = state.config.clone();
    let hardware_for_status = environment.hardware.clone();
    let assessment = tokio::task::spawn_blocking(move || {
        let hardware = hardware_for_status.unwrap_or_else(crash_ai::hardware::detect);
        crash_ai::feasibility::assess_llm(
            &hardware,
            config_for_status.parameters_b,
            config_for_status.context_tokens,
        )
    })
    .await
    .ok();

    /*
     * 熔断状态：已经停用就直接跳过模型，不再等一次超时。
     *
     * 没有这一步，一台跑不动模型的机器每次点分析都要干等 90 秒，而规则结论
     * 本来几毫秒就能出来。
     */
    let tripped = state
        .health
        .lock()
        .ok()
        .and_then(|health| health.disabled_reason().map(str::to_owned));
    let want_model = parsed.use_model && tripped.is_none();

    let started = std::time::Instant::now();
    let mut analysis =
        crash_ai::analyse_with_status(&parsed.log, &environment, &state.config, want_model, assessment.clone())
            .await;

    /*
     * 熔断已被触发过：把那条误导性的「按请求跳过」换掉。
     *
     * 内部调用传的是 `want_model = false`，框架层于是记了一条「已跳过模型分析
     * （按请求）」——但用户并没有跳过，是熔断器停用的。两条并存会让提示自相
     * 矛盾，所以先移除框架那条，再写真实原因。
     */
    if let Some(reason) = &tripped {
        analysis
            .warnings
            .retain(|warning| !warning.contains("按请求"));
        analysis
            .warnings
            .push(format!("本地模型已自动停用（{reason}）。恢复模型服务后重启本程序即可重新启用。"));
    }

    /*
     * 更新健康状态。
     *
     * 只看「请求了模型但没拿到结果」：模型本来就被硬件判断拦下时不算失败，
     * 否则一台 2 GB 的机器会把熔断也点上，而真正的原因已经由 `modelStatus` 说明。
     */
    if want_model && assessment.as_ref().is_none_or(|status| !status.disable_model) {
        let failed = !analysis.model_used;
        if let Ok(mut health) = state.health.lock() {
            if failed {
                let reason = analysis
                    .warnings
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "未知原因".to_owned());
                if health.record_failure(&reason) {
                    println!("model circuit breaker tripped: {reason}");
                }
            } else {
                health.record_success();
            }
        }
    }
    println!(
        "analyse: {} bytes, model={}, findings={}, {} ms",
        parsed.log.len(),
        analysis.model_used,
        analysis.findings.len(),
        started.elapsed().as_millis()
    );

    let mut value = match serde_json::to_value(&analysis) {
        Ok(value) => value,
        Err(error) => return Response::error(500, &format!("无法序列化结果：{error}")),
    };
    // Surface timing: "the model took 40 seconds" is useful when deciding whether
    // to keep it on, and invisible otherwise.
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "elapsedMs".to_owned(),
            serde_json::json!(started.elapsed().as_millis() as u64),
        );
    }
    Response::json(200, value)
}

// ---------------------------------------------------------------------------
// 受控文件读取
// ---------------------------------------------------------------------------

/// 允许读取的根目录。
///
/// ## 为什么是白名单根目录，而不是「任意路径 + 校验」
///
/// 这个服务会在用户本机跑，如果它能读任意文件，任何能访问 8790 端口的进程就能
/// 借它读走 `~/.ssh/id_rsa`。所以读取被限定在启动器自己的数据根下：
///
///   - `MFMCL_MINECRAFT_DIR` 指定的目录（默认 `~/.minecraft`）
///   - 当前工作目录下的 `crash-reports`
///
/// 判定用的是 **canonicalize 之后的前缀比较**，而不是字符串比较：`../` 与符号
/// 链接都能绕过字面前缀，但绕不过真实路径。
fn allowed_roots() -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Ok(value) = std::env::var("MFMCL_MINECRAFT_DIR") {
        let path = std::path::PathBuf::from(value);
        if let Ok(real) = path.canonicalize() {
            roots.push(real);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let path = std::path::PathBuf::from(home).join(".minecraft");
        if let Ok(real) = path.canonicalize() {
            roots.push(real);
        }
    }
    if let Ok(real) = std::env::current_dir() {
        roots.push(real);
    }
    roots
}

/// 判定一个路径是否在允许范围内，并返回其真实路径。
fn resolve_allowed(path: &str) -> Result<std::path::PathBuf, String> {
    let candidate = std::path::PathBuf::from(path);
    if !candidate.is_absolute() {
        return Err("需要绝对路径".into());
    }
    let real = candidate
        .canonicalize()
        .map_err(|error| format!("无法解析路径：{error}"))?;
    if !real.starts_with("/") {
        return Err("路径无效".into());
    }
    let roots = allowed_roots();
    if !roots.iter().any(|root| real.starts_with(root)) {
        return Err(format!(
            "拒绝读取该路径：它不在允许的范围内（{}）",
            roots.iter().map(|root| root.display().to_string()).collect::<Vec<_>>().join("、")
        ));
    }
    if !real.is_file() {
        return Err("不是一个文件".into());
    }
    Ok(real)
}

/// 列出可用的崩溃报告，让页面不必让用户自己找路径。
///
/// 只扫描允许根目录下的 `versions/*/crash-reports/*.txt`，不递归进任意位置。
async fn sources() -> Response {
    let mut found: Vec<serde_json::Value> = Vec::new();
    for root in allowed_roots() {
        let versions = root.join("versions");
        let Ok(entries) = std::fs::read_dir(&versions) else { continue };
        for entry in entries.flatten() {
            let reports = entry.path().join("crash-reports");
            let Ok(files) = std::fs::read_dir(&reports) else { continue };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|value| value.to_str()) != Some("txt") {
                    continue;
                }
                let name = path.file_name().and_then(|value| value.to_str()).unwrap_or("");
                if !name.starts_with("crash-") {
                    continue;
                }
                let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
                let modified = file
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0);
                found.push(serde_json::json!({
                    "instance": entry.file_name().to_string_lossy(),
                    "name": name,
                    "path": path.to_string_lossy(),
                    "bytes": size,
                    "modified": modified,
                }));
            }
        }
    }
    // 最近修改的排前面：用户要找的几乎总是刚崩溃的那次。
    found.sort_by(|a, b| b["modified"].as_u64().cmp(&a["modified"].as_u64()));
    Response::json(200, serde_json::json!({ "reports": found }))
}

#[derive(serde::Deserialize)]
struct ReadRequest {
    path: String,
}

/// 读取一个崩溃报告。路径必须落在允许的根目录内。
async fn read_controlled(request: &Request) -> Response {
    let parsed: ReadRequest = match serde_json::from_slice(&request.body) {
        Ok(value) => value,
        Err(error) => return Response::error(400, &format!("请求格式无效：{error}")),
    };
    let path = match resolve_allowed(&parsed.path) {
        Ok(path) => path,
        Err(message) => return Response::error(403, &message),
    };
    /*
     * 崩溃报告可能非常大（含完整模组列表）。超过分析上限的部分由分析器自己截断，
     * 但读入内存前先挡掉明显过大的文件，避免一次读几百 MB。
     */
    let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    if size > (crash_ai::MAX_LOG_BYTES as u64) * 2 {
        return Response::error(413, "文件过大");
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => Response::json(
            200,
            serde_json::json!({
                "path": path.to_string_lossy(),
                "name": path.file_name().and_then(|value| value.to_str()).unwrap_or(""),
                "content": content,
            }),
        ),
        Err(error) => Response::error(500, &format!("无法读取文件：{error}")),
    }
}

/// 探测硬件，供可行性判断使用。
///
/// 同步调用会读 `lspci` 与 sysinfo，几毫秒量级；`/api/health` 本来就不是热路径，
/// 这里直接同步读可以接受。真正的分析路径在 `analyse()` 里走 blocking 池。
fn tauri_hardware() -> crash_ai::hardware::Hardware {
    crash_ai::hardware::detect()
}
