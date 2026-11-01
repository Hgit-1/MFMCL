use super::*;
use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

struct Mock {
    url: String,
    requests: Arc<StdMutex<Vec<String>>>,
    task: JoinHandle<()>,
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}
#[derive(Clone)]
struct Reply {
    status: &'static str,
    headers: String,
    body: Vec<u8>,
    advertised: Option<usize>,
    delay: Duration,
}
impl Reply {
    fn ok(body: &[u8]) -> Self {
        Self {
            status: "200 OK",
            headers: String::new(),
            body: body.to_vec(),
            advertised: None,
            delay: Duration::ZERO,
        }
    }
}
async fn mock(handler: impl Fn(usize, &str) -> Reply + Send + Sync + 'static) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/artifact", listener.local_addr().unwrap());
    let requests = Arc::new(StdMutex::new(Vec::new()));
    let seen = requests.clone();
    let handler = Arc::new(handler);
    let count = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let handler = handler.clone();
            let seen = seen.clone();
            let count = count.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut buffer = [0; 1024];
                while !bytes.ends_with(b"\r\n\r\n") && bytes.len() < 65536 {
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let request = String::from_utf8_lossy(&bytes).to_string();
                seen.lock().unwrap().push(request.clone());
                let reply = handler(count.fetch_add(1, Ordering::SeqCst), &request);
                let head = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
                    reply.status,
                    reply.advertised.unwrap_or(reply.body.len()),
                    reply.headers
                );
                let _ = socket.write_all(head.as_bytes()).await;
                if !reply.delay.is_zero() {
                    tokio::time::sleep(reply.delay).await;
                }
                let _ = socket.write_all(&reply.body).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    Mock {
        url,
        requests,
        task,
    }
}
struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("mfmcl-download-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn artifact(dir: &TestDir, url: &str, body: &[u8]) -> ArtifactSpec {
    ArtifactSpec {
        id: "test".into(),
        label: "test artifact".into(),
        url: url.into(),
        mirror_url: None,
        destination: dir.0.join("artifact.bin"),
        size: Some(body.len() as u64),
        sha1: Some(hex::encode(Sha1::digest(body))),
        sha256: Some(hex::encode(Sha256::digest(body))),
    }
}
fn engine() -> DownloadEngine {
    DownloadEngine::new(DownloadSettings {
        aria2_enabled: false,
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn bounded_client_refuses_oversized_response_without_publishing() {
    let dir = TestDir::new();
    let server = mock(|_, _| Reply::ok(b"oversized binary content")).await;
    let mut spec = artifact(&dir, &server.url, b"oversized binary content");
    // The official authlib-injector release metadata may omit file size.
    spec.size = None;
    let result = engine().with_max_file_size(4).download(&spec, None).await;
    assert!(result.is_err());
    assert!(!spec.destination.exists());
}

#[test]
fn secure_sources_and_exact_mirror_hosts() {
    let original = Url::parse("https://resources.download.minecraft.net/ab/abcdef").unwrap();
    assert_eq!(
        bmclapi_url(&original).unwrap().as_str(),
        "https://bmclapi2.bangbang93.com/assets/ab/abcdef"
    );
    assert!(
        bmclapi_url(&Url::parse("https://libraries.minecraft.net.attacker.invalid/file").unwrap())
            .is_none()
    );
    assert!(
        bmclapi_url(&Url::parse("https://libraries.minecraft.net:8080/file").unwrap()).is_none()
    );
    assert!(
        source_candidates(
            Url::parse("file:///etc/passwd").unwrap(),
            SourcePreference::Official,
            true
        )
        .is_empty()
    );
    assert!(
        source_candidates(
            Url::parse("http://example.com/file").unwrap(),
            SourcePreference::Official,
            true
        )
        .is_empty()
    );
    assert!(
        source_candidates_with_mirror(
            original.clone(),
            Some("https://attacker.invalid/file"),
            SourcePreference::Mixed,
            true
        )
        .iter()
        .all(|url| url.host_str() != Some("attacker.invalid"))
    );
    assert!(
        source_candidates(
            Url::parse("https://user:password@example.com/file").unwrap(),
            SourcePreference::Official,
            true
        )
        .is_empty()
    );
    assert!(
        source_candidates(
            Url::parse("https://bmclapi2.bangbang93.com/file").unwrap(),
            SourcePreference::Official,
            false
        )
        .is_empty()
    );
    assert_eq!(
        source_candidates(original.clone(), SourcePreference::Mixed, false),
        vec![original.clone()]
    );
    assert_eq!(
        source_candidates(original, SourcePreference::Mixed, true).len(),
        2
    );
}

#[test]
fn hash_pinned_partial_identity_survives_source_changes() {
    let dir = TestDir::new();
    let a = artifact(&dir, "https://example.com/first?token=old", b"hello");
    let mut changed = a.clone();
    changed.url = "https://another.example/second?token=new".into();
    assert_eq!(partial_path(&a), partial_path(&changed));
    changed.sha256 = Some("different".into());
    assert_ne!(partial_path(&a), partial_path(&changed));
}

#[tokio::test]
async fn unsatisfiable_range_discards_partial_and_restarts() {
    let dir = TestDir::new();
    let server = mock(|index, request| match index {
        0 => {
            let mut reply = Reply::ok(b"hel");
            reply.advertised = Some(5);
            reply
        }
        1 => {
            assert!(request.to_ascii_lowercase().contains("range: bytes=3-"));
            let mut reply = Reply::ok(b"");
            reply.status = "416 Range Not Satisfiable";
            reply.headers = "Content-Range: bytes */5\r\n".into();
            reply
        }
        _ => {
            assert!(!request.to_ascii_lowercase().contains("range:"));
            Reply::ok(b"hello")
        }
    })
    .await;
    let a = artifact(&dir, &server.url, b"hello");
    engine().download(&a, None).await.unwrap();
    assert_eq!(server.requests.lock().unwrap().len(), 3);
    assert_eq!(fs::read(a.destination).await.unwrap(), b"hello");
}

#[tokio::test]
async fn full_hash_failure_preserves_existing_destination() {
    let dir = TestDir::new();
    let server = mock(|_, _| Reply::ok(b"wrong")).await;
    let a = artifact(&dir, &server.url, b"right");
    fs::write(&a.destination, b"prior").await.unwrap();
    assert!(matches!(
        engine().download(&a, None).await,
        Err(DownloadError::Integrity(_))
    ));
    assert_eq!(fs::read(&a.destination).await.unwrap(), b"prior");
    assert_eq!(server.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn retries_transient_http_failures_bounded() {
    let dir = TestDir::new();
    let server = mock(|i, _| {
        let mut reply = Reply::ok(b"hello");
        if i < 2 {
            reply.status = "503 Service Unavailable";
        }
        reply
    })
    .await;
    let a = artifact(&dir, &server.url, b"hello");
    engine().download(&a, None).await.unwrap();
    assert_eq!(server.requests.lock().unwrap().len(), 3);
    assert_eq!(fs::read(a.destination).await.unwrap(), b"hello");
}

#[tokio::test]
async fn resumes_interrupted_body_with_range_and_if_range() {
    let dir = TestDir::new();
    let server = mock(|i, request| {
        let mut r = Reply::ok(if i == 0 { b"hello" } else { b" world" });
        r.headers = "ETag: \"stable\"\r\n".into();
        if i == 0 {
            r.advertised = Some(11);
        } else {
            assert!(request.to_ascii_lowercase().contains("range: bytes=5-"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("if-range: \"stable\"")
            );
            r.status = "206 Partial Content";
            r.headers.push_str("Content-Range: bytes 5-10/11\r\n");
        }
        r
    })
    .await;
    let a = artifact(&dir, &server.url, b"hello world");
    engine().download(&a, None).await.unwrap();
    assert_eq!(server.requests.lock().unwrap().len(), 2);
    assert_eq!(fs::read(a.destination).await.unwrap(), b"hello world");
}

#[tokio::test]
async fn range_ignored_restarts_instead_of_appending() {
    let dir = TestDir::new();
    let server = mock(|i, request| {
        if i == 0 {
            let mut r = Reply::ok(b"hello");
            r.advertised = Some(11);
            r.headers = "ETag: \"v1\"\r\n".into();
            r
        } else {
            assert!(request.to_ascii_lowercase().contains("range: bytes=5-"));
            Reply::ok(b"hello world")
        }
    })
    .await;
    let a = artifact(&dir, &server.url, b"hello world");
    engine().download(&a, None).await.unwrap();
    assert_eq!(fs::read(a.destination).await.unwrap(), b"hello world");
}

#[tokio::test]
async fn invalid_content_range_is_discarded_before_retry() {
    let dir = TestDir::new();
    let server = mock(|i, _| {
        if i == 0 {
            let mut r = Reply::ok(b"hel");
            r.advertised = Some(5);
            r
        } else if i == 1 {
            let mut r = Reply::ok(b"lo");
            r.status = "206 Partial Content";
            r.headers = "Content-Range: bytes 2-3/5\r\n".into();
            r
        } else {
            Reply::ok(b"hello")
        }
    })
    .await;
    let a = artifact(&dir, &server.url, b"hello");
    engine().download(&a, None).await.unwrap();
    assert_eq!(server.requests.lock().unwrap().len(), 3);
    assert_eq!(fs::read(a.destination).await.unwrap(), b"hello");
}

#[tokio::test]
async fn mirror_failure_falls_back_to_official() {
    let dir = TestDir::new();
    let mirror = mock(|_, _| Reply::ok(b"wrong")).await;
    let official = mock(|_, _| Reply::ok(b"right")).await;
    let mut a = artifact(&dir, &official.url, b"right");
    a.mirror_url = Some(mirror.url.clone());
    let e = DownloadEngine::new(DownloadSettings {
        source: SourcePreference::Mixed,
        aria2_enabled: false,
        ..Default::default()
    })
    .unwrap();
    e.download(&a, None).await.unwrap();
    assert_eq!(fs::read(a.destination).await.unwrap(), b"right");
    assert_eq!(mirror.requests.lock().unwrap().len(), 3);
    assert_eq!(official.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancel_interrupts_stalled_response_and_retry_wait() {
    let dir = TestDir::new();
    let server = mock(|_, _| {
        let mut r = Reply::ok(b"hello");
        r.delay = Duration::from_secs(30);
        r
    })
    .await;
    let a = artifact(&dir, &server.url, b"hello");
    let control = DownloadControl::new();
    let e = engine().with_control(control.clone());
    let task = tokio::spawn(async move { e.download(&a, None).await });
    tokio::time::sleep(Duration::from_millis(60)).await;
    control.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap(),
        Err(DownloadError::Cancelled)
    ));
    let server = mock(|_, _| {
        let mut r = Reply::ok(b"");
        r.status = "503 Service Unavailable";
        r
    })
    .await;
    let a = artifact(&dir, &server.url, b"hello");
    let control = DownloadControl::new();
    let e = engine().with_control(control.clone());
    let task = tokio::spawn(async move { e.download(&a, None).await });
    tokio::time::sleep(Duration::from_millis(60)).await;
    control.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap(),
        Err(DownloadError::Cancelled)
    ));
}

/// Opt-in end-to-end test against a REAL aria2 sidecar binary (not a mock):
/// set MFMCL_ARIA2_BINARY to a 1.37.0 aria2c. Mirrors the runtime path —
/// loopback RPC session, checksum-pinned download, verification back in the
/// engine, and graceful shutdown — without touching the public network.
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires a real aria2 1.37.0 binary via MFMCL_ARIA2_BINARY"]
async fn real_aria2_sidecar_downloads_and_verifies_through_the_engine() {
    let binary =
        std::env::var("MFMCL_ARIA2_BINARY").expect("MFMCL_ARIA2_BINARY must point to aria2c");
    let dir = TestDir::new();
    let server = mock(|_, _| Reply::ok(b"real aria2 payload")).await;
    let session = aria2::Aria2Session::start(
        std::path::Path::new(&binary),
        &dir.0.join("aria2-home"),
        aria2::Aria2Options::default(),
    )
    .await
    .expect("real aria2 must start with a loopback RPC session");
    let good = artifact(&dir, &server.url, b"real aria2 payload");
    DownloadEngine::new(DownloadSettings {
        aria2_enabled: true,
        ..Default::default()
    })
    .unwrap()
    .with_aria2(session.clone())
    .download(&good, None)
    .await
    .expect("verified download via the real sidecar");
    assert_eq!(
        fs::read(&good.destination).await.unwrap(),
        b"real aria2 payload"
    );
    // A tampered checksum must be rejected by the engine, not published.
    let mut tampered = artifact(&dir, &server.url, b"real aria2 payload");
    tampered.destination = dir.0.join("tampered.bin");
    tampered.sha1 = Some("0".repeat(40));
    let result = DownloadEngine::new(DownloadSettings {
        aria2_enabled: true,
        ..Default::default()
    })
    .unwrap()
    .with_aria2(session.clone())
    .download(&tampered, None)
    .await;
    assert!(
        matches!(result, Err(DownloadError::Integrity(_))),
        "{result:?}"
    );
    assert!(!tampered.destination.exists());
    session.shutdown().await.expect("graceful shutdown");
}

#[cfg(unix)]
#[tokio::test]
async fn aria2_failure_falls_back_with_a_single_source_and_redacts_secret() {
    let dir = TestDir::new();
    let rpc = mock(|_, _| {
        let mut reply = Reply::ok(b"unavailable");
        reply.status = "503 Service Unavailable";
        reply
    })
    .await;
    let official = mock(|_, _| Reply::ok(b"hello")).await;
    let session = aria2::Aria2Session::failing_test_session(rpc.url.clone(), &dir.0);
    assert!(!format!("{session:?}").contains("test-secret-must-not-appear"));
    let a = artifact(&dir, &official.url, b"hello");
    DownloadEngine::new(DownloadSettings::default())
        .unwrap()
        .with_aria2(session)
        .download(&a, None)
        .await
        .unwrap();
    assert_eq!(official.requests.lock().unwrap().len(), 1);
    assert_eq!(fs::read(a.destination).await.unwrap(), b"hello");
}

#[tokio::test]
async fn pause_holds_an_active_response_until_resumed() {
    let dir = TestDir::new();
    let server = mock(|_, _| {
        let mut reply = Reply::ok(b"hello");
        reply.delay = Duration::from_millis(120);
        reply
    })
    .await;
    let a = artifact(&dir, &server.url, b"hello");
    let control = DownloadControl::new();
    let e = engine().with_control(control.clone());
    let task = tokio::spawn(async move { e.download(&a, None).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    control.pause();
    tokio::time::sleep(Duration::from_millis(180)).await;
    assert!(!task.is_finished(), "paused transfer must not complete");
    control.resume();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn pause_before_start_and_cancel_while_paused() {
    let dir = TestDir::new();
    let server = mock(|_, _| Reply::ok(b"hello")).await;
    let a = artifact(&dir, &server.url, b"hello");
    let control = DownloadControl::new();
    control.pause();
    let e = engine().with_control(control.clone());
    let task = tokio::spawn(async move { e.download(&a, None).await });
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(server.requests.lock().unwrap().is_empty());
    control.resume();
    task.await.unwrap().unwrap();
    control.pause();
    let waiting = control.clone();
    let task = tokio::spawn(async move { waiting.checkpoint().await });
    control.cancel();
    assert!(matches!(task.await.unwrap(), Err(DownloadError::Cancelled)));
}

#[tokio::test]
async fn clones_share_global_concurrency_and_queued_cancel() {
    let dir = TestDir::new();
    let server = mock(|_, _| {
        let mut r = Reply::ok(b"hello");
        r.delay = Duration::from_millis(150);
        r
    })
    .await;
    let e = DownloadEngine::new(DownloadSettings {
        max_concurrent_downloads: 1,
        aria2_enabled: false,
        ..Default::default()
    })
    .unwrap();
    let a = artifact(&dir, &server.url, b"hello");
    let mut b = a.clone();
    b.destination = dir.0.join("other");
    let e1 = e.clone();
    let t1 = tokio::spawn(async move { e1.download(&a, None).await });
    while server.requests.lock().unwrap().is_empty() {
        tokio::task::yield_now().await;
    }
    let control = DownloadControl::new();
    let e2 = e.with_control(control.clone());
    let t2 = tokio::spawn(async move { e2.download(&b, None).await });
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(server.requests.lock().unwrap().len(), 1);
    control.cancel();
    assert!(matches!(t2.await.unwrap(), Err(DownloadError::Cancelled)));
    t1.await.unwrap().unwrap();
}

#[tokio::test]
async fn unpinned_resume_requires_strong_matching_etag() {
    let dir = TestDir::new();
    let server = mock(|i, request| {
        if i == 0 {
            let mut r = Reply::ok(b"hel");
            r.advertised = Some(5);
            r.headers = "ETag: W/\"weak\"\r\n".into();
            r
        } else {
            assert!(!request.to_ascii_lowercase().contains("range:"));
            Reply::ok(b"hello")
        }
    })
    .await;
    let mut a = artifact(&dir, &server.url, b"hello");
    a.sha1 = None;
    a.sha256 = None;
    engine().download(&a, None).await.unwrap();
    assert_eq!(fs::read(a.destination).await.unwrap(), b"hello");
}

#[tokio::test]
async fn stable_partial_survives_cancel_and_new_engine() {
    let dir = TestDir::new();
    let server = mock(|_, request| {
        let mut r = Reply::ok(b" world");
        assert!(request.to_ascii_lowercase().contains("range: bytes=5-"));
        r.status = "206 Partial Content";
        r.headers = "Content-Range: bytes 5-10/11\r\nETag: \"entity\"\r\n".into();
        r
    })
    .await;
    let a = artifact(&dir, &server.url, b"hello world");
    let path = partial_path(&a);
    fs::write(&path, b"hello").await.unwrap();
    let meta = ResumeMetadata {
        source: fingerprint(&a.url),
        final_source: fingerprint(&a.url),
        etag: Some("\"entity\"".into()),
        total: Some(11),
    };
    fs::write(metadata_path(&path), serde_json::to_vec(&meta).unwrap())
        .await
        .unwrap();
    let control = DownloadControl::new();
    control.cancel();
    assert!(matches!(
        engine().with_control(control).download(&a, None).await,
        Err(DownloadError::Cancelled)
    ));
    engine().download(&a, None).await.unwrap();
    assert_eq!(fs::read(a.destination).await.unwrap(), b"hello world");
}

// ---------------------------------------------------------------------------
// 单文件分段并发
// ---------------------------------------------------------------------------

/// 支持 Range 的服务端：按请求头里的 `bytes=start-end` 返回对应切片。
async fn range_mock(body: Vec<u8>, honour_ranges: bool) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/artifact", listener.local_addr().unwrap());
    let requests = Arc::new(StdMutex::new(Vec::new()));
    let seen = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let body = body.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut buffer = [0; 1024];
                while !bytes.ends_with(b"\r\n\r\n") && bytes.len() < 65536 {
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let request = String::from_utf8_lossy(&bytes).to_string();
                seen.lock().unwrap().push(request.clone());
                let lower = request.to_ascii_lowercase();
                let range = lower
                    .lines()
                    .find_map(|line| line.strip_prefix("range: bytes="))
                    .and_then(|value| value.trim().split_once('-'))
                    .and_then(|(start, end)| {
                        Some((start.parse::<usize>().ok()?, end.parse::<usize>().ok()?))
                    });
                let (head, payload) = match range.filter(|_| honour_ranges) {
                    Some((start, end)) => {
                        let end = end.min(body.len() - 1);
                        let slice = body[start..=end].to_vec();
                        (
                            format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                                slice.len(),
                                start,
                                end,
                                body.len()
                            ),
                            slice,
                        )
                    }
                    None => (
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        ),
                        body.clone(),
                    ),
                };
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&payload).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    Mock {
        url,
        requests,
        task,
    }
}

fn segment_engine(connections: u8) -> DownloadEngine {
    DownloadEngine::new(DownloadSettings {
        aria2_enabled: false,
        connections_per_file: connections,
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn segments_split_the_file_across_parallel_connections() {
    let dir = TestDir::new();
    // 4 个分片 × 至少 256 KiB，确保真的会切分。
    let body: Vec<u8> = (0..=(1024 * 1024)).map(|i| (i % 251) as u8).collect();
    let server = range_mock(body.clone(), true).await;
    let a = artifact(&dir, &server.url, &body);
    segment_engine(4).download(&a, None).await.unwrap();
    assert_eq!(fs::read(a.destination).await.unwrap(), body);
    let requests = server.requests.lock().unwrap();
    let ranged = requests
        .iter()
        .filter(|r| r.to_ascii_lowercase().contains("range: bytes="))
        .count();
    assert_eq!(ranged, 4, "expected one request per segment: {requests:?}");
}

#[tokio::test]
async fn server_without_range_support_falls_back_to_one_connection() {
    let dir = TestDir::new();
    let body: Vec<u8> = (0..=(1024 * 1024)).map(|i| (i % 97) as u8).collect();
    let server = range_mock(body.clone(), false).await;
    let a = artifact(&dir, &server.url, &body);
    segment_engine(4).download(&a, None).await.unwrap();
    // 用 `&a.destination` 而不是 `a.destination`：下面还要用 `&a` 查临时文件，
    // 移出字段会让 `a` 处于部分移动状态，无法再整体借用。
    assert_eq!(fs::read(&a.destination).await.unwrap(), body);
    /*
     * 4 个分片各探测一次（服务端都回 200），然后退回单连接再请求一次，共 5 次。
     * 分片之间通过 abort 标志互相叫停，因此不会出现「每个分片都把整份文件收完」
     * 的情况；这条断言把请求数量钉住，防止退回路径变成重复下载。
     */
    let requests = server.requests.lock().unwrap().len();
    assert_eq!(requests, 5, "expected 4 range probes plus 1 fallback fetch");
    assert!(
        !partial_path(&a).exists(),
        "fallback should clean up the partial file"
    );
}

#[tokio::test]
async fn small_files_and_single_connection_skip_segmentation() {
    let dir = TestDir::new();
    let body = vec![7u8; 64 * 1024];
    let server = range_mock(body.clone(), true).await;
    let a = artifact(&dir, &server.url, &body);
    segment_engine(4).download(&a, None).await.unwrap();
    assert_eq!(fs::read(a.destination).await.unwrap(), body);
    let requests = server.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .all(|r| !r.to_ascii_lowercase().contains("range: bytes=")),
        "small file should not be split: {requests:?}"
    );
}

#[test]
fn segment_ranges_cover_the_whole_file_without_gaps() {
    for size in [1u64, 1024, 300 * 1024, 1024 * 1024 + 1, 5 * 1024 * 1024] {
        for segments in [1usize, 2, 4, 16] {
            let ranges = segment_ranges(size, segments);
            assert!(!ranges.is_empty(), "size {size} segments {segments}");
            assert_eq!(ranges[0].0, 0);
            assert_eq!(ranges.last().unwrap().1, size - 1);
            for pair in ranges.windows(2) {
                assert_eq!(pair[1].0, pair[0].1 + 1, "gap at size {size}");
            }
            for (start, end) in &ranges {
                assert!(start <= end);
            }
        }
    }
}
