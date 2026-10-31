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

    let state = Arc::new(AppState { config: Arc::new(config) });
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
    match crash_ai::llm::probe(&state.config).await {
        Ok(models) => Response::json(
            200,
            serde_json::json!({
                "ok": true,
                "model": state.config.model,
                "endpoint": state.config.base_url,
                "available": models,
            }),
        ),
        Err(error) => Response::json(
            200,
            serde_json::json!({
                "ok": false,
                "model": state.config.model,
                "endpoint": state.config.base_url,
                "error": error.to_string(),
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

    let started = std::time::Instant::now();
    let analysis =
        crash_ai::analyse(&parsed.log, &parsed.environment, &state.config, parsed.use_model).await;
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
