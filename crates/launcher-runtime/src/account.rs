//! Credential lifecycle without the GUI parts of login.
//!
//! ## What moved here and what stayed
//!
//! Refreshing a session, storing/reading/deleting keyring entries, and creating
//! an offline profile are all frontend-independent. They live here so the CLI can
//! launch an online account exactly the way the GUI does.
//!
//! The *interactive* login flows — Microsoft's browser redirect and the
//! skin-server profile picker — stay in `src-tauri`, because they need a system
//! browser, a callback page, and a dialog to choose a role. A command line will
//! grow its own versions of those; they are not shared code, they are shared
//! protocols.

use crate::{Result, RuntimeError};
use launcher_auth::{
    AuthSession, CredentialStore, Credentials, MicrosoftClient, MicrosoftConfig, Secret,
    YggdrasilClient,
};
use launcher_core::{AccountKind, AccountProfile};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Vault namespace. Must match the desktop app so both see the same entries.
pub const CREDENTIAL_SERVICE: &str = "dev.mfmcl.launcher";

/// What the keyring holds for one account.
///
/// Serialization exists only for the credential-store adapter. This type contains
/// live secrets and is never written to SQLite, logs, or frontend events.
#[derive(Serialize, Deserialize)]
pub struct StoredCredential {
    pub profile_id: String,
    pub client_id: Option<String>,
    pub credentials: Credentials,
}

fn auth_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Auth(format!("认证未完成：{error}"))
}

fn vault() -> Result<CredentialStore> {
    CredentialStore::new(CREDENTIAL_SERVICE).map_err(auth_error)
}

/// Keyring calls block; run them off the async runtime.
pub async fn read_credential(id: String) -> Result<StoredCredential> {
    tokio::task::spawn_blocking(move || {
        let secret = vault()?
            .get(&id)
            .map_err(auth_error)?
            .ok_or_else(|| RuntimeError::message("系统凭据库中没有此账户；请重新登录。"))?;
        serde_json::from_str(secret.expose())
            .map_err(|_| RuntimeError::message("系统凭据条目损坏；请重新登录。"))
    })
    .await
    .map_err(|_| RuntimeError::message("凭据读取任务未能完成。"))?
}

pub async fn write_credential(id: String, credential: StoredCredential) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let text = serde_json::to_string(&credential)
            .map_err(|_| RuntimeError::message("无法序列化凭据。"))?;
        let secret = Secret::new(text);
        vault()?.put(&id, &secret).map_err(auth_error)
    })
    .await
    .map_err(|_| RuntimeError::message("凭据写入任务未能完成。"))?
}

/// Delete a credential entry. A missing entry counts as success.
pub async fn delete_credential(id: String) -> Result<()> {
    tokio::task::spawn_blocking(move || vault()?.delete(&id).map_err(auth_error))
        .await
        .map_err(|_| RuntimeError::message("凭据删除任务未能完成。"))?
}

fn root_url(value: &str) -> Result<reqwest::Url> {
    let mut root =
        reqwest::Url::parse(value).map_err(|_| RuntimeError::message("认证地址格式错误。"))?;
    if root.scheme() != "https"
        || root.host_str().is_none()
        || !root.username().is_empty()
        || root.password().is_some()
        || root.query().is_some()
        || root.fragment().is_some()
    {
        return Err(RuntimeError::message(
            "皮肤站必须使用不含账号、查询参数和片段的 HTTPS 根地址。",
        ));
    }
    if !root.path().ends_with('/') {
        root.set_path(&format!("{}/", root.path()));
    }
    Ok(root)
}

fn skin_client(root: &reqwest::Url) -> Result<YggdrasilClient> {
    let server = root.join("authserver/").map_err(auth_error)?;
    YggdrasilClient::new(server.clone(), &[server]).map_err(auth_error)
}

/// Refresh an online account's session.
///
/// The redirect URI passed to `MicrosoftConfig` is a fixed placeholder: a refresh
/// grant never redirects, and `MicrosoftConfig` validates the shape of the URI
/// rather than reaching it. The desktop code does the same.
pub async fn refresh(account: &AccountProfile) -> Result<AuthSession> {
    let key = account
        .credential_ref
        .clone()
        .ok_or_else(|| RuntimeError::message("账户没有凭据引用，请重新登录。"))?;
    let stored = read_credential(key.clone()).await?;
    let result = match account.kind {
        AccountKind::Microsoft => {
            let client_id = stored
                .client_id
                .clone()
                .ok_or_else(|| RuntimeError::message("Microsoft 账户配置缺失。"))?;
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
            let root = root_url(
                account
                    .auth_server
                    .as_deref()
                    .ok_or_else(|| RuntimeError::message("皮肤站地址缺失。"))?,
            )?;
            skin_client(&root)?
                .refresh(&stored.credentials)
                .await
                .map_err(auth_error)?
        }
        AccountKind::Offline => return Err(RuntimeError::message("离线档案不需要认证会话。")),
    };
    /*
     * A refresh that returns a different profile means the stored token belongs
     * to another UUID. Continuing would launch the game as the wrong player.
     */
    if result.session.profile_id != stored.profile_id {
        return Err(RuntimeError::message(
            "认证返回了与保存账户不同的角色；请重新登录。",
        ));
    }
    write_credential(
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

/// Create an offline profile.
///
/// Offline names are the player name sent to the game, so the same 3–16
/// alphanumeric/underscore rule applies here as in the desktop handler.
pub fn offline_profile(player_name: &str) -> Result<AccountProfile> {
    let name = player_name.trim();
    if !(3..=16).contains(&name.len())
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(RuntimeError::message(
            "离线档案名称需为 3–16 位英文字母、数字或下划线。",
        ));
    }
    Ok(AccountProfile {
        id: uuid::Uuid::new_v4().to_string(),
        display_name: name.into(),
        kind: AccountKind::Offline,
        auth_server: None,
        credential_ref: None,
        selected: true,
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// The still-secret pieces of a session, for building launch arguments.
///
/// Wrapped so a caller cannot accidentally log it: no `Debug`, no `Serialize`.
pub struct SessionSecrets {
    pub access_token: Zeroizing<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_names_follow_the_game_rule() {
        assert!(offline_profile("Player_1").is_ok());
        assert!(offline_profile("ab").is_err(), "too short");
        assert!(offline_profile("way_too_long_a_name").is_err());
        assert!(offline_profile("bad name").is_err(), "space");
        assert!(offline_profile("bad-name").is_err(), "hyphen");
    }

    #[test]
    fn skin_server_roots_must_be_plain_https() {
        assert!(root_url("https://skin.example.com").is_ok());
        assert!(root_url("http://skin.example.com").is_err(), "cleartext");
        assert!(
            root_url("https://u:p@skin.example.com").is_err(),
            "credentials"
        );
        assert!(root_url("https://skin.example.com/?a=1").is_err(), "query");
        assert!(root_url("https://skin.example.com/#x").is_err(), "fragment");
    }

    #[test]
    fn root_url_normalises_a_trailing_slash() {
        let root = root_url("https://skin.example.com/base").unwrap();
        assert_eq!(root.as_str(), "https://skin.example.com/base/");
    }

    #[test]
    fn vault_namespace_matches_the_desktop_app() {
        // Both frontends must read the same keyring entries, otherwise an account
        // added in the GUI is invisible to the CLI and vice versa.
        assert_eq!(CREDENTIAL_SERVICE, "dev.mfmcl.launcher");
    }
}
