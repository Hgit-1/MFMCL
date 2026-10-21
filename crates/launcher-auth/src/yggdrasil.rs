use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    AuthError, AuthSession, AuthenticatedAccount, Credentials, GameProfile, checked_response,
    decode, http_client, secrets_equal, valid_secret, validate_https_url,
};

/// A Yggdrasil authentication base URL (for example an authlib-injector
/// server's `/authserver/` URL), explicitly approved by the user/configuration.
/// Trust is compared against exact normalized base URLs, not host suffixes.
pub struct YggdrasilClient {
    http: Client,
    server: Url,
}

/// The result of password authentication, possibly requiring profile selection.
/// Passwords are never retained. This type contains secrets and is not Debug.
pub struct YggdrasilLogin {
    pub available_profiles: Vec<GameProfile>,
    selected_profile: Option<GameProfile>,
    server: Url,
    access_token: String,
    client_token: String,
}
impl Drop for YggdrasilLogin {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.client_token.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenResponse {
    access_token: String,
    client_token: String,
    #[serde(default)]
    available_profiles: Vec<GameProfile>,
    selected_profile: Option<GameProfile>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshRequest<'a> {
    access_token: &'a str,
    client_token: &'a str,
    selected_profile: &'a GameProfile,
    request_user: bool,
}

fn normalize_base(mut server: Url) -> Result<Url, AuthError> {
    validate_https_url(&server)?;
    if !server.path().ends_with('/') {
        let path = format!("{}/", server.path());
        server.set_path(&path);
    }
    Ok(server)
}

impl YggdrasilClient {
    pub fn new(server: Url, trusted_servers: &[Url]) -> Result<Self, AuthError> {
        let server = normalize_base(server)?;
        let trusted = trusted_servers
            .iter()
            .any(|trusted| normalize_base(trusted.clone()).is_ok_and(|base| base == server));
        if !trusted {
            return Err(AuthError::UntrustedServer);
        }
        Ok(Self {
            http: http_client()?,
            server,
        })
    }

    pub fn server(&self) -> &Url {
        &self.server
    }

    fn endpoint(&self, operation: &str) -> Result<Url, AuthError> {
        self.server.join(operation).map_err(|_| {
            AuthError::InvalidConfiguration("invalid Yggdrasil authentication base URL")
        })
    }

    /// Authenticate against an already trusted HTTPS server. A fresh client
    /// token binds the resulting credential to this launcher login.
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<YggdrasilLogin, AuthError> {
        if !valid_secret(username) || !valid_secret(password) {
            return Err(AuthError::InvalidInput(
                "username and password are required",
            ));
        }
        #[derive(Serialize)]
        struct Agent {
            name: &'static str,
            version: u8,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Request<'a> {
            agent: Agent,
            username: &'a str,
            password: &'a str,
            client_token: &'a str,
            request_user: bool,
        }
        let mut client_token_bytes = [0_u8; 16];
        crate::random_bytes(&mut client_token_bytes)?;
        client_token_bytes[6] = (client_token_bytes[6] & 0x0f) | 0x40;
        client_token_bytes[8] = (client_token_bytes[8] & 0x3f) | 0x80;
        let client_token = uuid::Uuid::from_bytes(client_token_bytes).to_string();
        let response = checked_response(
            self.http
                .post(self.endpoint("authenticate")?)
                .json(&Request {
                    agent: Agent {
                        name: "Minecraft",
                        version: 1,
                    },
                    username,
                    password,
                    client_token: &client_token,
                    request_user: false,
                })
                .send()
                .await,
            "Yggdrasil",
        )
        .await?;
        let response: TokenResponse = decode(response, "Yggdrasil").await?;
        validate_token(&response, &client_token)?;
        let available_profiles = response
            .available_profiles
            .iter()
            .map(GameProfile::validated)
            .collect::<Result<Vec<_>, _>>()?;
        let selected_profile = response
            .selected_profile
            .as_ref()
            .map(GameProfile::validated)
            .transpose()?;
        Ok(YggdrasilLogin {
            available_profiles,
            selected_profile,
            server: self.server.clone(),
            access_token: response.access_token,
            client_token,
        })
    }

    /// Consume a pending login. An explicit selection is checked against the
    /// server-provided profiles, then bound to the access token by refresh.
    /// With no explicit selection, use the server's selected profile or its
    /// sole available profile; never silently pick one from multiple profiles.
    pub async fn select_profile(
        &self,
        mut login: YggdrasilLogin,
        profile_id: Option<&str>,
    ) -> Result<AuthenticatedAccount, AuthError> {
        if login.server != self.server {
            return Err(AuthError::CredentialMismatch);
        }
        let profile = choose_profile(
            &login.available_profiles,
            login.selected_profile.as_ref(),
            profile_id,
        )?;
        if login.selected_profile.as_ref() == Some(&profile) {
            return Ok(account(
                self.server.clone(),
                std::mem::take(&mut login.access_token),
                std::mem::take(&mut login.client_token),
                profile,
            ));
        }
        self.refresh_values(&login.access_token, &login.client_token, &profile)
            .await
    }

    /// Refresh stored credentials and preserve the selected profile. The caller
    /// must replace the keyring entry with the returned credentials.
    pub async fn refresh(
        &self,
        credentials: &Credentials,
    ) -> Result<AuthenticatedAccount, AuthError> {
        let Credentials::Yggdrasil {
            server,
            access_token,
            client_token,
            profile,
        } = credentials
        else {
            return Err(AuthError::CredentialMismatch);
        };
        if normalize_base(server.clone())? != self.server {
            return Err(AuthError::CredentialMismatch);
        }
        self.refresh_values(access_token, client_token, &profile.validated()?)
            .await
    }

    /// Accept only HTTP 204 as validation success. Credentials must belong to
    /// this exact trusted root; validation does not establish Minecraft ownership.
    pub async fn validate(&self, credentials: &Credentials) -> Result<(), AuthError> {
        let Credentials::Yggdrasil {
            server,
            access_token,
            client_token,
            profile,
        } = credentials
        else {
            return Err(AuthError::CredentialMismatch);
        };
        if normalize_base(server.clone())? != self.server {
            return Err(AuthError::CredentialMismatch);
        }
        profile.validated()?;
        if !valid_secret(access_token) || !valid_secret(client_token) {
            return Err(AuthError::InvalidInput("invalid stored Yggdrasil token"));
        }
        let response = self
            .http
            .post(self.endpoint("validate")?)
            .json(&serde_json::json!({"accessToken":access_token,"clientToken":client_token}))
            .send()
            .await
            .map_err(|_| AuthError::Network)?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            Ok(())
        } else {
            Err(AuthError::Http {
                service: "Yggdrasil",
                status: response.status().as_u16(),
            })
        }
    }

    async fn refresh_values(
        &self,
        access_token: &str,
        client_token: &str,
        profile: &GameProfile,
    ) -> Result<AuthenticatedAccount, AuthError> {
        if !valid_secret(access_token) || !valid_secret(client_token) {
            return Err(AuthError::InvalidInput("invalid stored Yggdrasil token"));
        }
        let response = checked_response(
            self.http
                .post(self.endpoint("refresh")?)
                .json(&RefreshRequest {
                    access_token,
                    client_token,
                    selected_profile: profile,
                    request_user: false,
                })
                .send()
                .await,
            "Yggdrasil",
        )
        .await?;
        let response: TokenResponse = decode(response, "Yggdrasil").await?;
        validate_token(&response, client_token)?;
        let selected = response
            .selected_profile
            .ok_or(AuthError::ProfileUnavailable)?
            .validated()?;
        if &selected != profile {
            return Err(AuthError::ProfileUnavailable);
        }
        Ok(account(
            self.server.clone(),
            response.access_token,
            response.client_token,
            selected,
        ))
    }
}

fn validate_token(response: &TokenResponse, client_token: &str) -> Result<(), AuthError> {
    if !valid_secret(&response.access_token) || !secrets_equal(&response.client_token, client_token)
    {
        return Err(AuthError::InvalidResponse("Yggdrasil"));
    }
    Ok(())
}

fn choose_profile(
    available: &[GameProfile],
    selected: Option<&GameProfile>,
    requested: Option<&str>,
) -> Result<GameProfile, AuthError> {
    if let Some(requested) = requested {
        let id = uuid::Uuid::parse_str(requested)
            .map_err(|_| AuthError::InvalidProfile)?
            .simple()
            .to_string();
        return available
            .iter()
            .chain(selected)
            .find(|profile| profile.id == id)
            .cloned()
            .ok_or(AuthError::ProfileUnavailable);
    }
    if let Some(selected) = selected {
        if selected.validated()? != *selected || !available.contains(selected) {
            return Err(AuthError::ProfileUnavailable);
        }
        return Ok(selected.clone());
    }
    if let [profile] = available {
        return Ok(profile.clone());
    }
    Err(AuthError::ProfileSelectionRequired)
}

fn account(
    server: Url,
    access_token: String,
    client_token: String,
    profile: GameProfile,
) -> AuthenticatedAccount {
    AuthenticatedAccount {
        session: AuthSession {
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            access_token: Zeroizing::new(access_token.clone()),
            xuid: None,
            client_id: None,
            expires_at: None,
        },
        credentials: Credentials::Yggdrasil {
            server,
            access_token,
            client_token,
            profile,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> GameProfile {
        GameProfile::new("123456781234423481231234567890ab", "Player_1").unwrap()
    }

    #[test]
    fn trust_is_exact_and_https_only() {
        let base = Url::parse("https://auth.example/authserver").unwrap();
        assert!(YggdrasilClient::new(base.clone(), &[]).is_err());
        assert!(YggdrasilClient::new(base.clone(), &[base.clone()]).is_ok());
        for bad in [
            "http://auth.example/authserver",
            "https://auth.example.evil/authserver",
            "https://user:password@auth.example/authserver",
            "https://auth.example/other",
            "https://auth.example/authserver?token=secret",
            "https://auth.example/authserver#fragment",
        ] {
            assert!(YggdrasilClient::new(Url::parse(bad).unwrap(), &[base.clone()]).is_err());
        }
    }

    #[test]
    fn selection_is_explicit_and_available() {
        let first = profile();
        let second = GameProfile::new("223456781234423481231234567890ab", "Second").unwrap();
        assert_eq!(
            choose_profile(&[], None, None),
            Err(AuthError::ProfileSelectionRequired)
        );
        assert_eq!(
            choose_profile(&[first.clone(), second.clone()], None, None),
            Err(AuthError::ProfileSelectionRequired)
        );
        assert_eq!(choose_profile(&[first.clone()], None, None).unwrap(), first);
        assert_eq!(
            choose_profile(&[first.clone(), second.clone()], None, Some(&second.id)).unwrap(),
            second
        );
        assert_eq!(
            choose_profile(&[first], None, Some("323456781234423481231234567890ab")),
            Err(AuthError::ProfileUnavailable)
        );
    }

    #[tokio::test]
    async fn refresh_rotates_access_and_retains_selected_profile() {
        let expected_profile = profile();
        let (base, server) = crate::tests::mock_server(vec![(
            200,
            serde_json::json!({
                "accessToken": "rotated-secret", "clientToken": "client-secret",
                "selectedProfile": expected_profile,
            }),
        )])
        .await;
        // Test-only HTTP transport: public constructors remain HTTPS-only.
        let client = YggdrasilClient {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
            server: Url::parse(&format!("{base}/authserver/")).unwrap(),
        };
        let result = client
            .refresh_values("old-secret", "client-secret", &expected_profile)
            .await
            .unwrap();
        assert_eq!(result.session.access_token.as_str(), "rotated-secret");
        assert_eq!(result.session.profile_id, expected_profile.id);
        assert!(
            matches!(&result.credentials, Credentials::Yggdrasil { access_token, .. } if access_token == "rotated-secret")
        );
        let requests = server.await.unwrap();
        assert!(requests[0].starts_with("POST /authserver/refresh "));
        let payload: serde_json::Value =
            serde_json::from_str(requests[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(payload["selectedProfile"]["id"], expected_profile.id);
        assert_eq!(payload["accessToken"], "old-secret");
    }

    #[tokio::test]
    async fn refresh_rejects_changed_profile_and_remote_errors_are_redacted() {
        for (status, body, expected_error) in [
            (
                200,
                serde_json::json!({ "accessToken": "secret", "clientToken": "client",
                "selectedProfile": { "id": "223456781234423481231234567890ab", "name": "Other" } }),
                AuthError::ProfileUnavailable,
            ),
            (
                403,
                serde_json::json!({ "errorMessage": "secret-password-and-token" }),
                AuthError::Http {
                    service: "Yggdrasil",
                    status: 403,
                },
            ),
            (
                302,
                serde_json::json!({ "accessToken": "secret" }),
                AuthError::Http {
                    service: "Yggdrasil",
                    status: 302,
                },
            ),
        ] {
            let (base, server) = crate::tests::mock_server(vec![(status, body)]).await;
            let client = YggdrasilClient {
                http: reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap(),
                server: Url::parse(&format!("{base}/")).unwrap(),
            };
            let Err(error) = client
                .refresh_values("old-secret", "client", &profile())
                .await
            else {
                panic!("invalid response accepted")
            };
            assert_eq!(error, expected_error);
            assert!(!format!("{error:?}: {error}").contains("secret"));
            assert_eq!(server.await.unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn local_mock_refresh_validate_and_redirect_rejection() {
        let (base, server) = crate::tests::mock_server(vec![
            (200, serde_json::json!({"accessToken":"new", "clientToken":"client", "selectedProfile":{"id":"123456781234423481231234567890ab", "name":"Player_1"}})),
            (204, serde_json::Value::Null), (302, serde_json::Value::Null),
        ]).await;
        let client = YggdrasilClient {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            server: Url::parse(&format!("{base}/authserver/")).unwrap(),
        };
        // Test-only HTTP endpoint injection. Production constructors enforce HTTPS.
        let account = client
            .refresh_values("old", "client", &profile())
            .await
            .unwrap();
        assert_eq!(account.session.access_token.as_str(), "new");
        let response = client
            .http
            .post(client.endpoint("validate").unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        let response = checked_response(
            client
                .http
                .post(client.endpoint("authenticate").unwrap())
                .send()
                .await,
            "Yggdrasil",
        )
        .await;
        assert!(matches!(response, Err(AuthError::Http { status: 302, .. })));
        let requests = server.await.unwrap();
        assert!(requests[0].contains("selectedProfile"));
        assert!(requests[0].starts_with("POST /authserver/refresh"));
        assert_eq!(requests.len(), 3);
    }

    #[tokio::test]
    async fn local_mock_rejects_profile_substitution() {
        let (base, server) = crate::tests::mock_server(vec![(200, serde_json::json!({"accessToken":"new", "clientToken":"client", "selectedProfile":{"id":"223456781234423481231234567890ab", "name":"Player_1"}}))]).await;
        let client = YggdrasilClient {
            http: reqwest::Client::new(),
            server: Url::parse(&format!("{base}/")).unwrap(),
        };
        assert!(matches!(
            client.refresh_values("old", "client", &profile()).await,
            Err(AuthError::ProfileUnavailable)
        ));
        server.await.unwrap();
    }

    #[test]
    fn token_response_must_match_client_token() {
        let mut response = TokenResponse {
            access_token: "secret".into(),
            client_token: "client".into(),
            available_profiles: vec![],
            selected_profile: None,
        };
        assert!(validate_token(&response, "client").is_ok());
        assert!(validate_token(&response, "other").is_err());
        response.access_token.clear();
        assert!(validate_token(&response, "client").is_err());
    }
}
