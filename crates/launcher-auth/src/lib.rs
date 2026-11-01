//! Authentication for Minecraft Java Edition.
//!
//! Clients only send secrets over HTTPS, never follow redirects, and impose a
//! 30-second request timeout. Microsoft application IDs and redirect URIs must
//! come from the launcher's configuration; this crate contains no application ID.
//!
//! Keep credentials backend-only. Sessions and authorization requests implement
//! neither `Debug` nor `Serialize`. Credentials deliberately lack `Debug`, and
//! their serialization is exclusively for the system-keyring adapter.
#![forbid(unsafe_code)]

#[cfg(feature = "native-credentials")]
mod vault;
#[cfg(feature = "native-credentials")]
pub use vault::CredentialStore;
#[cfg(test)]
mod tests;

use zeroize::{Zeroize, Zeroizing};

/// Explicit backend secret for use with the credential store.
pub struct Secret(Zeroizing<String>);
impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
pub type Result<T, E = AuthError> = std::result::Result<T, E>;

mod flow;
mod microsoft;
pub use flow::MicrosoftFlow;
mod yggdrasil;

pub use microsoft::{MicrosoftAuthorization, MicrosoftClient, MicrosoftConfig, PkceVerifier};
use rand::{RngCore, rngs::OsRng};
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use url::Url;
pub use yggdrasil::{YggdrasilClient, YggdrasilLogin};

/// A validated Minecraft profile. IDs are normalized to 32 lowercase hex digits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameProfile {
    pub id: String,
    pub name: String,
}
impl GameProfile {
    pub fn new(id: &str, name: &str) -> Result<Self, AuthError> {
        let valid_id = (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
            || (id.len() == 36
                && id.bytes().enumerate().all(|(i, b)| {
                    if [8, 13, 18, 23].contains(&i) {
                        b == b'-'
                    } else {
                        b.is_ascii_hexdigit()
                    }
                }));
        if !valid_id {
            return Err(AuthError::InvalidProfile);
        }
        let id = uuid::Uuid::parse_str(id).map_err(|_| AuthError::InvalidProfile)?;
        if id.is_nil()
            || name.is_empty()
            || name.len() > 16
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(AuthError::InvalidProfile);
        }
        Ok(Self {
            id: id.simple().to_string(),
            name: name.to_owned(),
        })
    }
    pub(crate) fn validated(&self) -> Result<Self, AuthError> {
        Self::new(&self.id, &self.name)
    }
}

/// Persist only in an OS credential store. Contains live secrets.
/// No password or Microsoft application secret is stored. Serialization is only
/// for the backend OS credential-store adapter, never frontend IPC or logs.
#[derive(Deserialize, Serialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum Credentials {
    Yggdrasil {
        server: Url,
        access_token: String,
        client_token: String,
        profile: GameProfile,
    },
    Microsoft {
        refresh_token: String,
    },
}
impl Drop for Credentials {
    fn drop(&mut self) {
        match self {
            Self::Yggdrasil {
                access_token,
                client_token,
                ..
            } => {
                access_token.zeroize();
                client_token.zeroize();
            }
            Self::Microsoft { refresh_token } => refresh_token.zeroize(),
        }
    }
}

/// A short-lived launch session. Never serialize or log the access token.
#[derive(Zeroize)]
pub struct AuthSession {
    pub profile_id: String,
    pub profile_name: String,
    pub access_token: Zeroizing<String>,
    /// Xbox user ID used by Minecraft's optional launch argument.
    pub xuid: Option<String>,
    /// Public Microsoft application ID; absent for Yggdrasil.
    pub client_id: Option<String>,
    /// Unix timestamp, when the provider supplies an expiry.
    pub expires_at: Option<u64>,
}
/// A launch session and replacement credentials to save to the keyring.
pub struct AuthenticatedAccount {
    pub session: AuthSession,
    pub credentials: Credentials,
}

/// Errors contain static descriptions and status codes only. Remote response
/// bodies and reqwest errors (which may contain URLs) are deliberately discarded.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("invalid authentication configuration: {0}")]
    InvalidConfiguration(&'static str),
    #[error("the Yggdrasil server is not explicitly trusted")]
    UntrustedServer,
    #[error("invalid authentication input: {0}")]
    InvalidInput(&'static str),
    #[error("authentication network request failed")]
    Network,
    /// 响应来自非 HTTPS 传输。请求体里带着令牌，明文往返等于把它们交给路径上的
    /// 任何人，所以这里宁可失败也不继续。
    #[error("refusing to handle a {0} response over an insecure transport")]
    InsecureTransport(&'static str),
    #[error("{service} returned HTTP {status}")]
    Http { service: &'static str, status: u16 },
    #[error("{0} returned an invalid authentication response")]
    InvalidResponse(&'static str),
    #[error("invalid Minecraft profile")]
    InvalidProfile,
    #[error("a Minecraft profile must be selected")]
    ProfileSelectionRequired,
    #[error("the selected profile is not available for this account")]
    ProfileUnavailable,
    #[error("credentials do not belong to this authentication client")]
    CredentialMismatch,
    #[error("authorization callback or state is invalid")]
    InvalidCallback,
    #[error("Microsoft authorization was declined or failed")]
    AuthorizationDenied,
    #[error("secure random generation failed")]
    Randomness,
    #[error("authentication flow expired")]
    Expired,
    #[error("OS credential store is unavailable or denied access")]
    CredentialStore,
    #[error("this account does not own Minecraft Java Edition")]
    MissingEntitlement,
    #[error("this account has no Minecraft Java profile")]
    MissingMinecraftProfile,
}
fn random_bytes(bytes: &mut [u8]) -> Result<(), AuthError> {
    OsRng
        .try_fill_bytes(bytes)
        .map_err(|_| AuthError::Randomness)
}
fn http_client() -> Result<Client, AuthError> {
    Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| AuthError::InvalidConfiguration("could not create HTTPS client"))
}
pub(crate) async fn checked_response(
    response: Result<Response, reqwest::Error>,
    service: &'static str,
) -> Result<Response, AuthError> {
    checked_response_with(response, service, cfg!(test)).await
}

/// 与 `checked_response` 相同，但可以显式关掉回环豁免。
///
/// 只有测试用它：测试里唯一能连上的服务器就在回环上，如果豁免不可关闭，那么
/// 「拒绝明文」这条分支永远走不到，删掉拦截也不会让任何测试变红。
#[cfg(test)]
pub(crate) async fn checked_response_strict(
    response: Result<Response, reqwest::Error>,
    service: &'static str,
) -> Result<Response, AuthError> {
    checked_response_with(response, service, false).await
}

/// `checked_response` 的实现，回环豁免与否由调用方决定。
///
/// 拆出参数是为了能测：如果豁免硬编码成 `cfg!(test)`，那么测试里唯一能连上的
/// 服务器（回环地址）恰好总是被豁免，拦截分支就永远走不到——删掉拦截，测试照样
/// 全绿。让测试显式传 `false`，验证的才是真正那条拒绝路径。
async fn checked_response_with(
    response: Result<Response, reqwest::Error>,
    service: &'static str,
    allow_loopback: bool,
) -> Result<Response, AuthError> {
    let response = response.map_err(|_| AuthError::Network)?;
    /*
     * 拒绝把响应交给调用方，除非它来自受信任的传输。
     *
     * ## 为什么在这里检查
     *
     * 这个函数是所有认证响应的**唯一**必经之路（OAuth、Xbox、XSTS、登录、
     * 拥有的游戏、档案，六处都从这里走）。请求体里带着 refresh token 和 access
     * token，一次明文往返就等于把它们交给路径上的任何人。
     *
     * 计划里的调用方都在编译期固定成 HTTPS 常量，但「现在没有明文调用」和「不会
     * 出现明文调用」是两回事：`MicrosoftEndpoints` 可被内部代码改写，将来一次
     * 重构、一个环境变量覆盖、一次忘记加 `s` 的手改，都会把令牌送出去，而且
     * 事后从代码里读不出来——它只是另一个 URL 字符串。
     *
     * 所以把约束放在运行时：明文响应一律拒绝。代码扫描报的也是这一点
     * （`rust/cleartext-transmission`）；与其用抑制注释让它闭嘴，不如让它指的
     * 那条路径根本走不通。
     *
     * 回环地址例外，只在测试构建里：端到端测试用本地假服务器（`http://127.0.0.1`）
     * 验证整条链路，而它不出本机、不经过任何网络接口。发布构建中 `cfg!(test)`
     * 为 false，这个分支被编译器删掉。
     */
    if response.url().scheme() != "https" && !is_loopback_url(response.url(), allow_loopback) {
        return Err(AuthError::InsecureTransport(service));
    }
    if !response.status().is_success() {
        return Err(AuthError::Http {
            service,
            status: response.status().as_u16(),
        });
    }
    Ok(response)
}

/// 回环地址上的明文 URL 是否被豁免。
///
/// `allow` 由 `checked_response` 传入 `cfg!(test)`，所以发布构建里恒为 false。
/// 单独成函数而不是内联，是为了让「这个例外只存在于测试构建」一眼可见。
fn is_loopback_url(url: &reqwest::Url, allow: bool) -> bool {
    if !allow {
        return false;
    }
    url.scheme() == "http"
        && url
            .host_str()
            .is_some_and(|host| host == "127.0.0.1" || host == "localhost")
}
pub(crate) async fn decode<T: serde::de::DeserializeOwned>(
    response: Response,
    service: &'static str,
) -> Result<T, AuthError> {
    let mut response = response;
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|_| AuthError::Network)? {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(AuthError::InvalidResponse(service));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| AuthError::InvalidResponse(service))
}
pub(crate) fn valid_secret(value: &str) -> bool {
    !value.is_empty() && value.len() <= 65_536 && !value.chars().any(char::is_control)
}
pub(crate) fn secrets_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}
pub(crate) fn validate_https_url(url: &Url) -> Result<(), AuthError> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AuthError::InvalidConfiguration(
            "expected an HTTPS URL without credentials, query, or fragment",
        ));
    }
    Ok(())
}
