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

/*
 * ---------------------------------------------------------------------------
 * Microsoft 应用（公共客户端）ID 的运行时配置
 * ---------------------------------------------------------------------------
 *
 * ## 为什么需要
 *
 * 应用 ID 不是密钥，但它与发布者绑定，不能借用别人的。于是每个自部署的人都要
 * 提供一个——而只支持构建期注入意味着「想登录就得重新编译」。这不是用户能接受的
 * 前提。
 *
 * ## 存在哪里
 *
 * 系统凭据库（与账户令牌、CurseForge key 同一个 vault），不进 localStorage：
 * 前端读不到它，也不会被页面脚本带走。数据库里不落任何副本。
 *
 * ## 读取顺序
 *
 * 命令行传入 > 用户保存的 > 构建期默认值。用户保存的排在构建期之前：一个自己
 * 填过 ID 的人，显然是想用自己的，而不是发行包里那个。
 */
const MICROSOFT_CLIENT_ID_ACCOUNT: &str = "microsoft-client-id";

/// 客户端 ID 的形状：GUID。只做这一层校验，不判断它是否真的在 Entra 注册过
/// （那要联网，而这里的职责是挡住明显的输入错误）。
pub(crate) fn looks_like_client_id(value: &str) -> bool {
    let text = value.trim();
    if text.len() != 36 {
        return false;
    }
    text.char_indices().all(|(index, ch)| match index {
        8 | 13 | 18 | 23 => ch == '-',
        _ => ch.is_ascii_hexdigit(),
    })
}

/// 读取用户保存的客户端 ID。读不到时返回 `None`，不把它变成登录失败。
async fn stored_client_id() -> Option<String> {
    tauri::async_runtime::spawn_blocking(|| {
        let store = vault().ok()?;
        store
            .get(MICROSOFT_CLIENT_ID_ACCOUNT)
            .ok()
            .flatten()
            .map(|secret| secret.expose().trim().to_owned())
            .filter(|value| !value.is_empty())
    })
    .await
    .ok()
    .flatten()
}

/// 解析最终使用的客户端 ID：显式传入 > 已保存 > 构建期默认值。
pub(crate) async fn resolve_client_id(explicit: Option<&str>) -> Option<String> {
    if let Some(value) = explicit.map(str::trim).filter(|value| !value.is_empty()) {
        return Some(value.to_owned());
    }
    if let Some(value) = stored_client_id().await {
        return Some(value);
    }
    option_env!("MFMCL_MICROSOFT_CLIENT_ID")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientIdStatus {
    /// 是否已有可用的 ID（用户保存的或构建期内置的）。
    pub configured: bool,
    /// 来源：`builtin`（编译时注入）或 `user`（运行时填写）。
    pub source: Option<&'static str>,
    /// 没配置时的原因（凭据库不可用等）。
    pub problem: Option<String>,
}

/// 只回答「有没有可用的 ID」，不返回 ID 本身。
///
/// 不回传的理由与 CurseForge key 一致：它是一个与发布者身份绑定的标识，前端没有
/// 使用它的场景，泄露只会让别人把他人的 ID 打进自己的构建。
#[tauri::command]
pub async fn microsoft_client_id_status() -> Result<ClientIdStatus, String> {
    let saved = tauri::async_runtime::spawn_blocking(|| {
        let store = match vault() {
            Ok(store) => store,
            Err(problem) => return (false, Some(problem)),
        };
        match store.get(MICROSOFT_CLIENT_ID_ACCOUNT) {
            Ok(Some(secret)) => {
                let value = secret.expose().trim().to_owned();
                (!value.is_empty(), None)
            }
            Ok(None) => (false, None),
            Err(error) => (false, Some(error.to_string())),
        }
    })
    .await
    .map_err(|error| error.to_string())?;

    let (configured, problem) = saved;
    if configured {
        return Ok(ClientIdStatus {
            configured: true,
            source: Some("user"),
            problem,
        });
    }
    let builtin = option_env!("MFMCL_MICROSOFT_CLIENT_ID")
        .map(str::trim)
        .filter(|value| !value.is_empty());
    Ok(ClientIdStatus {
        configured: builtin.is_some(),
        source: builtin.map(|_| "builtin"),
        problem,
    })
}

/// 保存运行时的客户端 ID。
///
/// 先校验形状再写入：一个明显打错的 ID 存进去只会让后续登录以一个难懂的 OAuth
/// 错误结束，而在保存时就报错要清楚得多。
#[tauri::command]
pub async fn microsoft_save_client_id(client_id: String) -> Result<ClientIdStatus, String> {
    let value = client_id.trim().to_owned();
    if !looks_like_client_id(&value) {
        return Err(
            "应用 ID 应当是 36 位 GUID，形如 00000000-0000-0000-0000-000000000000。".into(),
        );
    }
    tauri::async_runtime::spawn_blocking(move || {
        let store = vault()?;
        store
            .put(
                MICROSOFT_CLIENT_ID_ACCOUNT,
                &launcher_auth::Secret::new(value),
            )
            .map_err(auth_error)
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(ClientIdStatus {
        configured: true,
        source: Some("user"),
        problem: None,
    })
}

/// 清除用户保存的 ID，回落到构建期默认值（若有）。
#[tauri::command]
pub async fn microsoft_clear_client_id() -> Result<ClientIdStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let store = vault()?;
        store
            .delete(MICROSOFT_CLIENT_ID_ACCOUNT)
            .map_err(auth_error)
    })
    .await
    .map_err(|error| error.to_string())??;
    microsoft_client_id_status().await
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
/// Best-effort keyring removal when the user deletes an account.
/// A missing entry is success; a failing store surfaces as an error string.
pub fn delete_credential(id: &str) -> Result<(), String> {
    vault()?.delete(id).map_err(auth_error)
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
    let client_id = match resolve_client_id(Some(&client_id)).await {
        Some(value) => value,
        None => {
            return Err(
                "此构建尚未配置 Microsoft 应用 ID。请在登录窗口里填入你注册并获准调用 Minecraft 服务的公共客户端 ID（形如 36 位 GUID）；不会借用其他启动器的 ID。"
                    .into(),
            );
        }
    };
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

#[cfg(test)]
mod client_id_tests {
    use super::looks_like_client_id;

    #[test]
    fn accepts_a_plain_guid() {
        assert!(looks_like_client_id("00000000-0000-0000-0000-000000000000"));
        assert!(looks_like_client_id("3f2504e0-4f89-11d3-9a0c-0305e82c3301"));
        // 前后空白不应当导致失败：用户是从网页上复制的。
        assert!(looks_like_client_id(
            "  3f2504e0-4f89-11d3-9a0c-0305e82c3301  "
        ));
    }

    #[test]
    fn rejects_obvious_input_mistakes() {
        for bad in [
            "",
            "not-a-guid",
            // 少一段
            "3f2504e0-4f89-11d3-9a0c",
            // 多一位
            "3f2504e0-4f89-11d3-9a0c-0305e82c33011",
            // 分隔符位置不对
            "3f2504e-04f89-11d3-9a0c-0305e82c3301",
            // 非十六进制字符
            "3f2504e0-4f89-11d3-9a0c-0305e82c33zz",
        ] {
            assert!(!looks_like_client_id(bad), "{bad:?} should be rejected");
        }
    }
}
