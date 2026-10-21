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
    let response = response.map_err(|_| AuthError::Network)?;
    if !response.status().is_success() {
        return Err(AuthError::Http {
            service,
            status: response.status().as_u16(),
        });
    }
    Ok(response)
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
