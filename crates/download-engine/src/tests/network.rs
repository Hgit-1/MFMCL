use super::*;

fn configured(settings: DownloadSettings) -> DownloadEngine {
    DownloadEngine::new(settings).unwrap()
}

#[test]
fn legacy_engine_settings_use_safe_defaults_and_reject_invalid_network_values() {
    let value = serde_json::json!({
        "source": "official", "maxConcurrentDownloads": 2,
        "connectionsPerFile": 1, "aria2Enabled": true
    });
    let settings: DownloadSettings = serde_json::from_value(value).unwrap();
    assert_eq!(settings.proxy_mode, ProxyMode::None);
    assert!(settings.proxy_url.is_empty());
    assert_eq!(settings.timeout_sec, 60);
    assert_eq!(settings.retries, 2);
    for invalid in [
        "",
        "proxy.example:7890",
        "socks5://127.0.0.1:7890",
        "file:///proxy",
        "http://user:secret@proxy.test",
        "http://proxy.test/path",
        "http://proxy.test?token=x",
        "http://proxy.test#fragment",
        "http://127.0.0.1:0",
        "http://proxy.test\n",
    ] {
        let result = DownloadEngine::new(DownloadSettings {
            proxy_mode: ProxyMode::Http,
            proxy_url: invalid.into(),
            ..Default::default()
        });
        assert!(
            matches!(result, Err(DownloadError::Configuration(_))),
            "{invalid:?}"
        );
    }
    for settings in [
        DownloadSettings {
            timeout_sec: 0,
            ..Default::default()
        },
        DownloadSettings {
            timeout_sec: 601,
            ..Default::default()
        },
        DownloadSettings {
            retries: 11,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            DownloadEngine::new(settings),
            Err(DownloadError::Configuration(_))
        ));
    }
    for proxy in [
        "http://127.0.0.1:7890",
        "https://proxy.test:8443",
        "http://[::1]:7890",
    ] {
        DownloadSettings {
            proxy_mode: ProxyMode::Http,
            proxy_url: proxy.into(),
            ..Default::default()
        }
        .validate_network()
        .unwrap();
    }
}

#[tokio::test]
async fn explicit_proxy_receives_the_download_and_preserves_integrity_checks() {
    let dir = TestDir::new();
    let proxy = mock(|_, _| Reply::ok(b"proxied")).await;
    // A closed local origin proves the response is coming from the proxy, not directly.
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/artifact", origin.local_addr().unwrap());
    drop(origin);
    let spec = artifact(&dir, &url, b"proxied");
    let engine = configured(DownloadSettings {
        proxy_mode: ProxyMode::Http,
        proxy_url: proxy.url.trim_end_matches("/artifact").into(),
        retries: 0,
        ..Default::default()
    });
    engine.download(&spec, None).await.unwrap();
    assert_eq!(fs::read(&spec.destination).await.unwrap(), b"proxied");
    assert!(proxy.requests.lock().unwrap()[0].starts_with(&format!("GET {url} HTTP/1.1")));
    let bad = mock(|_, _| Reply::ok(b"altered")).await;
    let mut spec = spec;
    spec.destination = dir.0.join("tampered");
    let engine = configured(DownloadSettings {
        proxy_mode: ProxyMode::Http,
        proxy_url: bad.url.trim_end_matches("/artifact").into(),
        retries: 0,
        ..Default::default()
    });
    assert!(matches!(
        engine.download(&spec, None).await,
        Err(DownloadError::Integrity(_))
    ));
    assert!(!spec.destination.exists());
}

#[tokio::test]
async fn https_downloads_use_connect_through_the_explicit_proxy() {
    let dir = TestDir::new();
    let proxy = mock(|_, _| {
        let mut reply = Reply::ok(b"");
        reply.status = "502 Bad Gateway";
        reply
    })
    .await;
    let engine = configured(DownloadSettings {
        proxy_mode: ProxyMode::Http,
        proxy_url: proxy.url.trim_end_matches("/artifact").into(),
        timeout_sec: 2,
        retries: 0,
        ..Default::default()
    });
    let spec = artifact(&dir, "https://downloads.example.invalid/artifact", b"hello");
    assert!(engine.download(&spec, None).await.is_err());
    let requests = proxy.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("CONNECT downloads.example.invalid:443 HTTP/1.1"));
    assert!(!spec.destination.exists());
}

#[tokio::test]
async fn retry_setting_is_additional_attempts_per_source_and_never_retries_404() {
    for retries in [0, 1, 3] {
        let dir = TestDir::new();
        let server = mock(|_, _| {
            let mut reply = Reply::ok(b"");
            reply.status = "503 Service Unavailable";
            reply
        })
        .await;
        let spec = artifact(&dir, &server.url, b"hello");
        let engine = configured(DownloadSettings {
            retries,
            ..Default::default()
        });
        assert!(engine.download(&spec, None).await.is_err());
        assert_eq!(
            server.requests.lock().unwrap().len(),
            usize::from(retries) + 1
        );
        assert!(!spec.destination.exists());
    }
    let dir = TestDir::new();
    let server = mock(|_, _| {
        let mut reply = Reply::ok(b"");
        reply.status = "404 Not Found";
        reply
    })
    .await;
    let spec = artifact(&dir, &server.url, b"hello");
    assert!(
        configured(DownloadSettings {
            retries: 3,
            ..Default::default()
        })
        .download(&spec, None)
        .await
        .is_err()
    );
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn configured_timeout_bounds_body_and_allows_a_retry_to_succeed() {
    let dir = TestDir::new();
    let server = mock(|i, _| {
        let mut reply = Reply::ok(b"hello");
        if i == 0 {
            reply.delay = Duration::from_secs(10);
        }
        reply
    })
    .await;
    let spec = artifact(&dir, &server.url, b"hello");
    let engine = configured(DownloadSettings {
        timeout_sec: 1,
        retries: 1,
        ..Default::default()
    });
    tokio::time::timeout(Duration::from_secs(5), engine.download(&spec, None))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(server.requests.lock().unwrap().len(), 2);
    assert_eq!(fs::read(spec.destination).await.unwrap(), b"hello");

    let server = mock(|_, _| {
        let mut reply = Reply::ok(b"hello");
        reply.delay = Duration::from_secs(10);
        reply
    })
    .await;
    let mut spec = artifact(&dir, &server.url, b"hello");
    spec.destination = dir.0.join("timeout.bin");
    let engine = configured(DownloadSettings {
        timeout_sec: 1,
        retries: 0,
        ..Default::default()
    });
    let result = tokio::time::timeout(Duration::from_secs(5), engine.download(&spec, None))
        .await
        .unwrap();
    assert!(matches!(result, Err(DownloadError::Network(error)) if error.is_timeout()));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
    assert!(!spec.destination.exists());
}

#[tokio::test]
async fn proxy_environment_child() {
    let Ok(mode) = std::env::var("MFMCL_PROXY_TEST_MODE") else {
        return;
    };
    let settings = DownloadSettings {
        proxy_mode: if mode == "none" {
            ProxyMode::None
        } else {
            ProxyMode::System
        },
        timeout_sec: 3,
        retries: 0,
        ..Default::default()
    };
    let client = settings.build_client().unwrap();
    let response = client
        .get(std::env::var("MFMCL_PROXY_TEST_URL").unwrap())
        .send()
        .await
        .unwrap();
    let expected = if mode == "system" { "proxy" } else { "origin" };
    assert_eq!(response.text().await.unwrap(), expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn none_ignores_system_proxy_while_system_honors_proxy_and_no_proxy() {
    let proxy = mock(|_, _| Reply::ok(b"proxy")).await;
    let origin = mock(|_, _| Reply::ok(b"origin")).await;
    let proxy_url = proxy.url.trim_end_matches("/artifact");
    for (mode, bypass) in [("none", ""), ("system", ""), ("system-bypass", "127.0.0.1")] {
        // Environment mutation stays in an isolated child, never racing parallel Rust tests.
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::network::proxy_environment_child",
                "--nocapture",
            ])
            .env("MFMCL_PROXY_TEST_MODE", mode)
            .env("MFMCL_PROXY_TEST_URL", &origin.url)
            .env("HTTP_PROXY", proxy_url)
            .env("http_proxy", proxy_url)
            .env("HTTPS_PROXY", proxy_url)
            .env("https_proxy", proxy_url)
            .env("ALL_PROXY", proxy_url)
            .env("all_proxy", proxy_url)
            .env("NO_PROXY", bypass)
            .env("no_proxy", bypass)
            .env_remove("REQUEST_METHOD")
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "mode={mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(proxy.requests.lock().unwrap().len(), 1);
    assert_eq!(origin.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn mock_reads_complete_json_rpc_post_bodies() {
    let rpc = mock(|_, _| Reply::ok(b"{}")).await;
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": "test", "method": "aria2.addUri",
        "params": ["x".repeat(2048)]
    });
    let client = DownloadSettings::default().build_client().unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        client.post(&rpc.url).json(&body).send(),
    )
    .await
    .unwrap()
    .unwrap()
    .error_for_status()
    .unwrap();
    let requests = rpc.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let (headers, received_body) = requests[0].split_once("\r\n\r\n").unwrap();
    assert!(headers.starts_with("POST /artifact HTTP/1.1"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(received_body).unwrap(),
        body
    );
}

#[cfg(unix)]
#[tokio::test]
async fn attached_sidecar_forwards_http_policy_and_falls_back_but_excludes_system_proxy() {
    let dir = TestDir::new();
    let rpc = mock(|_, _| Reply::ok(b"{}")).await;
    let proxy = mock(|i, _| {
        let mut reply = Reply::ok(b"hello");
        if i == 0 {
            reply.status = "503 Service Unavailable";
        }
        reply
    })
    .await;
    let session = aria2::Aria2Session::failing_test_session(rpc.url.clone(), &dir.0);
    let system_engine = configured(DownloadSettings {
        proxy_mode: ProxyMode::System,
        aria2_enabled: true,
        ..Default::default()
    })
    .with_aria2(session.clone());
    assert!(system_engine.aria2.is_none());
    assert!(rpc.requests.lock().unwrap().is_empty());
    let engine = configured(DownloadSettings {
        proxy_mode: ProxyMode::Http,
        proxy_url: proxy.url.trim_end_matches("/artifact").into(),
        timeout_sec: 1,
        retries: 1,
        aria2_enabled: true,
        ..Default::default()
    })
    .with_aria2(session.clone());
    assert!(engine.aria2.is_some());
    let spec = artifact(&dir, "http://127.0.0.1:1/artifact", b"hello");
    tokio::time::timeout(Duration::from_secs(5), engine.download(&spec, None))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fs::read(&spec.destination).await.unwrap(), b"hello");
    {
        let proxy_requests = proxy.requests.lock().unwrap();
        assert_eq!(proxy_requests.len(), 2);
        assert!(
            proxy_requests
                .iter()
                .all(|request| request.starts_with(&format!("GET {} HTTP/1.1", spec.url)))
        );
    }
    let calls: Vec<serde_json::Value> = rpc
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap())
        .collect();
    let add_uri: Vec<_> = calls
        .iter()
        .filter(|call| call["method"] == "aria2.addUri")
        .collect();
    assert_eq!(add_uri.len(), 1);
    assert_eq!(add_uri[0]["params"][1], serde_json::json!([spec.url]));
    let options = &add_uri[0]["params"][2];
    let proxy_url = proxy.url.trim_end_matches("/artifact");
    assert_eq!(options["all-proxy"], proxy_url);
    assert_eq!(options["http-proxy"], proxy_url);
    assert_eq!(options["https-proxy"], proxy_url);
    assert_eq!(options["no-proxy"], "");
    assert_eq!(options["connect-timeout"], "1");
    assert_eq!(options["timeout"], "1");
    assert_eq!(options["max-tries"], "2");
    assert_eq!(options["retry-wait"], "1");
    assert_eq!(
        options["checksum"],
        format!("sha-256={}", spec.sha256.as_ref().unwrap())
    );
    let _ = session.shutdown().await;
}
