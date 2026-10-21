//! Backend-only credential lifecycle. Frontend receives profile metadata, never tokens.
use launcher_auth::{
    AuthSession, AuthenticatedAccount, CredentialStore, Credentials, GameProfile, MicrosoftClient,
    MicrosoftConfig, MicrosoftFlow, Secret, YggdrasilClient, YggdrasilLogin,
};
use launcher_core::{AccountKind, AccountProfile, LauncherStore};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{AppHandle, State};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use zeroize::Zeroizing;

struct PendingSkin {
    login: YggdrasilLogin,
    root: reqwest::Url,
    started: Instant,
}
#[derive(Default)]
pub struct AuthState {
    pending: Mutex<HashMap<String, PendingSkin>>,
    browser_login: tokio::sync::Mutex<()>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinLoginResult {
    pending_id: String,
    profiles: Vec<GameProfile>,
}
#[derive(Serialize, Deserialize)]
struct StoredCredential {
    profile_id: String,
    client_id: Option<String>,
    credentials: Credentials,
}

fn auth_error(error: impl std::fmt::Display) -> String {
    format!("认证未完成：{error}")
}
fn root_url(value: &str) -> Result<reqwest::Url, String> {
    let mut root = reqwest::Url::parse(value).map_err(|_| "认证地址格式错误。")?;
    if root.scheme() != "https"
        || root.host_str().is_none()
        || !root.username().is_empty()
        || root.password().is_some()
        || root.query().is_some()
        || root.fragment().is_some()
    {
        return Err("皮肤站必须使用不含账号、查询参数和片段的 HTTPS 根地址。".into());
    }
    if !root.path().ends_with('/') {
        root.set_path(&format!("{}/", root.path()));
    }
    Ok(root)
}
fn skin_client(root: &reqwest::Url) -> Result<YggdrasilClient, String> {
    let server = root.join("authserver/").map_err(auth_error)?;
    YggdrasilClient::new(server.clone(), &[server]).map_err(auth_error)
}
fn vault() -> Result<CredentialStore, String> {
    CredentialStore::new("dev.mfmcl.launcher").map_err(auth_error)
}
async fn put_credentials(id: String, stored: StoredCredential) -> Result<(), String> {
    let value = Zeroizing::new(serde_json::to_string(&stored).map_err(|_| "凭据编码失败。")?);
    tauri::async_runtime::spawn_blocking(move || {
        vault()?
            .put(&id, &Secret::new(value.as_str()))
            .map_err(auth_error)
    })
    .await
    .map_err(auth_error)?
}
async fn get_credentials(id: String) -> Result<StoredCredential, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let secret = vault()?
            .get(&id)
            .map_err(auth_error)?
            .ok_or("系统凭据库中没有此账户；请重新登录。")?;
        serde_json::from_str(secret.expose()).map_err(|_| "系统凭据条目损坏；请重新登录。".into())
    })
    .await
    .map_err(auth_error)?
}
async fn save_account(
    store: &LauncherStore,
    result: AuthenticatedAccount,
    kind: AccountKind,
    root: Option<String>,
    client_id: Option<String>,
) -> Result<AccountProfile, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let account = AccountProfile {
        id: id.clone(),
        display_name: result.session.profile_name.clone(),
        kind,
        auth_server: root,
        credential_ref: Some(id.clone()),
        selected: true,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    put_credentials(
        id.clone(),
        StoredCredential {
            profile_id: result.session.profile_id,
            credentials: result.credentials,
            client_id,
        },
    )
    .await?;
    if let Err(error) = store.add_account(&account) {
        let _ =
            tauri::async_runtime::spawn_blocking(move || vault()?.delete(&id).map_err(auth_error))
                .await;
        return Err(error.to_string());
    }
    Ok(account)
}

#[tauri::command]
pub async fn skin_login(
    server: String,
    username: String,
    password: String,
    trusted: bool,
    auth: State<'_, AuthState>,
) -> Result<SkinLoginResult, String> {
    let password = Zeroizing::new(password);
    if !trusted {
        return Err("请明确确认你信任此皮肤站；凭据仅发送到确认的 HTTPS 认证地址。".into());
    }
    let root = root_url(&server)?;
    let client = skin_client(&root)?;
    let login = client
        .authenticate(&username, &password)
        .await
        .map_err(auth_error)?;
    let profiles = login.available_profiles.clone();
    let pending_id = uuid::Uuid::new_v4().to_string();
    let mut pending = auth.pending.lock().map_err(|_| "登录状态不可用。")?;
    pending.retain(|_, item| item.started.elapsed() < Duration::from_secs(300));
    if pending.len() >= 8 {
        return Err("待选择的登录过多，请稍后重试。".into());
    }
    pending.insert(
        pending_id.clone(),
        PendingSkin {
            login,
            root,
            started: Instant::now(),
        },
    );
    Ok(SkinLoginResult {
        pending_id,
        profiles,
    })
}
#[tauri::command]
pub async fn skin_select_profile(
    pending_id: String,
    profile_id: Option<String>,
    auth: State<'_, AuthState>,
    store: State<'_, LauncherStore>,
) -> Result<AccountProfile, String> {
    let pending = auth
        .pending
        .lock()
        .map_err(|_| "登录状态不可用。")?
        .remove(&pending_id)
        .ok_or("登录已过期，请重试。")?;
    if pending.started.elapsed() > Duration::from_secs(300) {
        return Err("登录已过期，请重试。".into());
    }
    let result = skin_client(&pending.root)?
        .select_profile(pending.login, profile_id.as_deref())
        .await
        .map_err(auth_error)?;
    save_account(
        &store,
        result,
        AccountKind::SkinServer,
        Some(pending.root.to_string()),
        None,
    )
    .await
}

#[tauri::command]
pub async fn microsoft_login(
    client_id: String,
    auth: State<'_, AuthState>,
    store: State<'_, LauncherStore>,
    _app: AppHandle,
) -> Result<AccountProfile, String> {
    let _guard = auth
        .browser_login
        .try_lock()
        .map_err(|_| "已有 Microsoft 登录正在进行。")?;
    let client_id = if client_id.trim().is_empty() {
        option_env!("MFMCL_MICROSOFT_CLIENT_ID")
            .unwrap_or("")
            .to_owned()
    } else {
        client_id.trim().to_owned()
    };
    if client_id.is_empty() {
        return Err("此开发构建尚未配置 Microsoft 应用 ID。请填入你注册并获准调用 Minecraft 服务的公共客户端 ID；不会借用其他启动器的 ID。".into());
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|_| "无法打开本机登录回调端口。")?;
    let redirect = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr().map_err(auth_error)?.port()
    );
    let flow = MicrosoftFlow::begin(client_id.clone(), &redirect).map_err(auth_error)?;
    let url = flow.authorization_url().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        open::that(url).map_err(|_| "无法打开系统浏览器。".to_owned())
    })
    .await
    .map_err(auth_error)??;
    let callback = tokio::time::timeout(
        Duration::from_secs(180),
        receive_callback(&listener, &redirect),
    )
    .await
    .map_err(|_| "登录等待超时，请重试。")??;
    let result = flow.finish_callback(&callback).await.map_err(auth_error)?;
    save_account(
        &store,
        result,
        AccountKind::Microsoft,
        None,
        Some(client_id),
    )
    .await
}
async fn receive_callback(listener: &TcpListener, redirect: &str) -> Result<reqwest::Url, String> {
    loop {
        let (mut stream, address) = listener.accept().await.map_err(|_| "无法接受登录回调。")?;
        if !address.ip().is_loopback() {
            continue;
        }
        let mut request = Vec::new();
        let read_result = tokio::time::timeout(Duration::from_secs(5), async {
            let mut buf = [0_u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let size = stream.read(&mut buf).await?;
                if size == 0 || request.len() + size > 16384 {
                    return Err(std::io::Error::other("invalid callback"));
                }
                request.extend_from_slice(&buf[..size]);
            }
            Ok::<_, std::io::Error>(())
        })
        .await;
        if !matches!(read_result, Ok(Ok(()))) {
            continue;
        }
        let target = std::str::from_utf8(&request)
            .ok()
            .and_then(|text| text.lines().next())
            .and_then(|line| line.strip_prefix("GET "))
            .and_then(|line| line.strip_suffix(" HTTP/1.1"));
        let Some(target) = target.filter(|t| t.starts_with("/callback?") && !t.starts_with("//"))
        else {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            continue;
        };
        let base = reqwest::Url::parse(redirect).map_err(|_| "回调配置无效。")?;
        let callback = base.join(target).map_err(|_| "回调地址无效。")?;
        let body = "Authorization received. You may close this window and return to MFMCL. Account verification continues in the launcher.";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        return Ok(callback);
    }
}

pub async fn refresh(account: &AccountProfile) -> Result<AuthSession, String> {
    let key = account
        .credential_ref
        .clone()
        .ok_or("账户没有凭据引用，请重新登录。")?;
    let stored = get_credentials(key.clone()).await?;
    let result = match account.kind {
        AccountKind::Microsoft => {
            let client_id = stored.client_id.clone().ok_or("Microsoft 账户配置缺失。")?;
            let client = MicrosoftClient::new(
                MicrosoftConfig::new(
                    client_id,
                    reqwest::Url::parse("http://127.0.0.1/callback").map_err(auth_error)?,
                )
                .map_err(auth_error)?,
            )
            .map_err(auth_error)?;
            client
                .refresh(&stored.credentials)
                .await
                .map_err(auth_error)?
        }
        AccountKind::SkinServer => {
            let root = root_url(account.auth_server.as_deref().ok_or("皮肤站地址缺失。")?)?;
            skin_client(&root)?
                .refresh(&stored.credentials)
                .await
                .map_err(auth_error)?
        }
        AccountKind::Offline => return Err("离线档案不需要认证会话。".into()),
    };
    if result.session.profile_id != stored.profile_id {
        return Err("认证返回了与保存账户不同的角色；请重新登录。".into());
    }
    put_credentials(
        key,
        StoredCredential {
            profile_id: stored.profile_id,
            credentials: result.credentials,
            client_id: stored.client_id,
        },
    )
    .await?;
    Ok(result.session)
}

pub async fn injector(
    root: &reqwest::Url,
    data_root: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(auth_error)?;
    let mut response = client
        .get("https://authlib-injector.yushi.moe/artifact/latest.json")
        .send()
        .await
        .map_err(|_| "无法获取 Authlib-Injector 版本信息。")?
        .error_for_status()
        .map_err(|_| "Authlib-Injector 版本服务不可用。")?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "认证代理元数据读取失败。")?
    {
        if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 {
            return Err("认证代理元数据超过大小限制。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "Authlib-Injector 元数据无效。")?;
    let url = meta
        .get("download_url")
        .and_then(|v| v.as_str())
        .ok_or("缺少认证代理地址。")?;
    let parsed = reqwest::Url::parse(url).map_err(|_| "认证代理地址无效。")?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("authlib-injector.yushi.moe")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.path().starts_with("/artifact/")
    {
        return Err("认证代理不来自允许的发布域名。".into());
    }
    let size = meta.get("size").and_then(serde_json::Value::as_u64);
    if size.is_some_and(|size| size == 0 || size > 64 * 1024 * 1024) {
        return Err("认证代理文件大小超过限制。".into());
    }
    let sha256 = meta
        .get("checksums")
        .and_then(|v| v.get("sha256"))
        .and_then(|v| v.as_str())
        .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or("认证代理缺少有效 SHA-256 校验值。")?;
    let path = data_root
        .join("authlib-injector")
        .join(format!("{sha256}.jar"));
    let engine = download_engine::DownloadEngine::new(download_engine::DownloadSettings::default())
        .map_err(auth_error)?
        .with_max_file_size(64 * 1024 * 1024);
    engine
        .download(
            &download_engine::ArtifactSpec {
                id: "authlib-injector".into(),
                label: "皮肤站认证代理".into(),
                url: parsed.to_string(),
                mirror_url: None,
                destination: path.clone(),
                size,
                sha1: None,
                sha256: Some(sha256.into()),
            },
            None,
        )
        .await
        .map_err(|_| "无法下载或验证认证代理。")?;
    let _ = root;
    Ok(path)
}
