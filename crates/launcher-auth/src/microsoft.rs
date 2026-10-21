use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::Zeroizing;

use crate::{
    AuthError, AuthSession, AuthenticatedAccount, Credentials, GameProfile, checked_response,
    decode, http_client, secrets_equal, valid_secret,
};

const AUTHORIZE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const SCOPE: &str = "XboxLive.signin offline_access";
const XBOX_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MINECRAFT_LOGIN: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MINECRAFT_ENTITLEMENTS: &str = "https://api.minecraftservices.com/entitlements/mcstore";
const MINECRAFT_PROFILE: &str = "https://api.minecraftservices.com/minecraft/profile";

/// Public-client registration supplied by the launcher configuration. Register
/// this application yourself with Microsoft; no borrowed ID or client secret is
/// used. The redirect URI must match the application's registered redirect URI.
#[derive(Clone, Debug)]
pub struct MicrosoftConfig {
    client_id: String,
    redirect_uri: Url,
}

impl MicrosoftConfig {
    pub fn new(client_id: impl Into<String>, redirect_uri: Url) -> Result<Self, AuthError> {
        let client_id = client_id.into();
        if client_id.is_empty()
            || client_id.len() > 256
            || !client_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(AuthError::InvalidConfiguration(
                "a Microsoft application client ID is required",
            ));
        }
        let secure = redirect_uri.scheme() == "https";
        let loopback = redirect_uri.scheme() == "http"
            && (matches!(redirect_uri.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
                || matches!(redirect_uri.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback()));
        if !(secure || loopback)
            || redirect_uri.host_str().is_none()
            || !redirect_uri.username().is_empty()
            || redirect_uri.password().is_some()
            || redirect_uri.query().is_some()
            || redirect_uri.fragment().is_some()
        {
            return Err(AuthError::InvalidConfiguration(
                "redirect URI must use HTTPS or an HTTP loopback IP, without credentials, query, or fragment",
            ));
        }
        Ok(Self {
            client_id,
            redirect_uri,
        })
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }
    pub fn redirect_uri(&self) -> &Url {
        &self.redirect_uri
    }
}

/// RFC 7636 verifier. Not Debug or serializable. Generated with the OS CSPRNG.
pub struct PkceVerifier(Zeroizing<String>);

impl PkceVerifier {
    pub fn generate() -> Result<Self, AuthError> {
        let mut bytes = [0_u8; 32];
        crate::random_bytes(&mut bytes)?;
        Ok(Self(Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes))))
    }

    /// Construct a validated verifier, also useful for deterministic RFC vectors.
    pub fn from_verifier(verifier: String) -> Result<Self, AuthError> {
        if !(43..=128).contains(&verifier.len())
            || !verifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        {
            return Err(AuthError::InvalidInput(
                "PKCE verifier must be 43–128 unreserved ASCII characters",
            ));
        }
        Ok(Self(Zeroizing::new(verifier)))
    }

    pub fn challenge(&self) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(self.0.as_bytes()))
    }
}

/// One pending authorization. Keep this in memory and consume it exactly once
/// when completing the browser flow. It owns the verifier and CSRF state.
pub struct MicrosoftAuthorization {
    authorization_url: Url,
    config: MicrosoftConfig,
    verifier: PkceVerifier,
    state: String,
    started: std::time::Instant,
}

impl MicrosoftAuthorization {
    pub fn url(&self) -> &Url {
        &self.authorization_url
    }

    /// Validate a complete redirect URL before extracting its code. Checks exact
    /// redirect scheme/host/port/path, state, and duplicate/error parameters.
    fn callback_code(&self, callback: &Url) -> Result<String, AuthError> {
        if self.started.elapsed() > std::time::Duration::from_secs(600) {
            return Err(AuthError::Expired);
        }
        let mut without_query = callback.clone();
        without_query.set_query(None);
        if without_query != self.config.redirect_uri || callback.fragment().is_some() {
            return Err(AuthError::InvalidCallback);
        }
        let mut state = None;
        let mut code = None;
        let mut denied = false;
        for (key, value) in callback.query_pairs() {
            match key.as_ref() {
                "state" if state.is_none() => state = Some(value.into_owned()),
                "code" if code.is_none() => code = Some(value.into_owned()),
                "state" | "code" => return Err(AuthError::InvalidCallback),
                "error" => denied = true,
                _ => {}
            }
        }
        if !state
            .as_deref()
            .is_some_and(|state| secrets_equal(state, &self.state))
        {
            return Err(AuthError::InvalidCallback);
        }
        if denied {
            return Err(AuthError::AuthorizationDenied);
        }
        code.filter(|code| valid_secret(code))
            .ok_or(AuthError::InvalidCallback)
    }
}

pub struct MicrosoftClient {
    http: Client,
    config: MicrosoftConfig,
    endpoints: MicrosoftEndpoints,
}

// Private: production callers cannot redirect Microsoft credentials elsewhere.
struct MicrosoftEndpoints {
    oauth_token: String,
    xbox: String,
    xsts: String,
    minecraft_login: String,
    entitlements: String,
    profile: String,
}

impl Default for MicrosoftEndpoints {
    fn default() -> Self {
        Self {
            oauth_token: TOKEN_URL.into(),
            xbox: XBOX_URL.into(),
            xsts: XSTS_URL.into(),
            minecraft_login: MINECRAFT_LOGIN.into(),
            entitlements: MINECRAFT_ENTITLEMENTS.into(),
            profile: MINECRAFT_PROFILE.into(),
        }
    }
}

impl MicrosoftClient {
    pub fn redirect_uri(&self) -> &Url {
        self.config.redirect_uri()
    }
    pub fn new(config: MicrosoftConfig) -> Result<Self, AuthError> {
        Ok(Self {
            http: http_client()?,
            config,
            endpoints: MicrosoftEndpoints::default(),
        })
    }

    /// Build a public-client authorization-code URL with S256 PKCE and random
    /// CSRF state. The parent application opens the URL and receives the redirect;
    /// this crate never starts a web server or handles a client secret.
    pub fn begin_authorization(&self) -> Result<MicrosoftAuthorization, AuthError> {
        let verifier = PkceVerifier::generate()?;
        let mut state = [0_u8; 32];
        crate::random_bytes(&mut state)?;
        self.authorization_with(verifier, URL_SAFE_NO_PAD.encode(state))
    }

    fn authorization_with(
        &self,
        verifier: PkceVerifier,
        state: String,
    ) -> Result<MicrosoftAuthorization, AuthError> {
        if state.len() < 32 || !valid_secret(&state) {
            return Err(AuthError::InvalidInput("invalid authorization state"));
        }
        let mut authorization_url = Url::parse(AUTHORIZE_URL).map_err(|_| {
            AuthError::InvalidConfiguration("invalid Microsoft authorization endpoint")
        })?;
        authorization_url
            .query_pairs_mut()
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", self.config.redirect_uri.as_str())
            .append_pair("response_type", "code")
            .append_pair("response_mode", "query")
            .append_pair("scope", SCOPE)
            .append_pair("code_challenge", &verifier.challenge())
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state);
        Ok(MicrosoftAuthorization {
            authorization_url,
            config: self.config.clone(),
            verifier,
            state,
            started: std::time::Instant::now(),
        })
    }

    /// Consume a pending authorization, check the callback and exchange the code,
    /// then authenticate with Xbox, XSTS, and Minecraft. Only accounts with Java
    /// entitlement and a valid Java profile become launchable sessions.
    pub async fn complete_authorization(
        &self,
        authorization: MicrosoftAuthorization,
        callback: &Url,
    ) -> Result<AuthenticatedAccount, AuthError> {
        if authorization.config.client_id != self.config.client_id
            || authorization.config.redirect_uri != self.config.redirect_uri
        {
            return Err(AuthError::CredentialMismatch);
        }
        let code = authorization.callback_code(callback)?;
        let response = self
            .oauth_token(&[
                ("client_id", self.config.client_id.as_str()),
                ("redirect_uri", self.config.redirect_uri.as_str()),
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("code_verifier", &authorization.verifier.0),
                ("scope", SCOPE),
            ])
            .await?;
        let refresh_token = response
            .refresh_token
            .as_deref()
            .filter(|token| valid_secret(token))
            .ok_or(AuthError::InvalidResponse("Microsoft OAuth"))?
            .to_owned();
        self.minecraft_session(&response.access_token, refresh_token)
            .await
    }

    /// Refresh the Microsoft token, repeat the full Xbox/XSTS/ownership/profile
    /// chain, and return replacement keyring credentials. Minecraft access tokens
    /// are never mistaken for Microsoft refresh tokens.
    pub async fn refresh(
        &self,
        credentials: &Credentials,
    ) -> Result<AuthenticatedAccount, AuthError> {
        let Credentials::Microsoft { refresh_token } = credentials else {
            return Err(AuthError::CredentialMismatch);
        };
        if !valid_secret(refresh_token) {
            return Err(AuthError::InvalidInput(
                "invalid stored Microsoft refresh token",
            ));
        }
        let response = self
            .oauth_token(&[
                ("client_id", self.config.client_id.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("scope", SCOPE),
            ])
            .await?;
        let replacement = response.refresh_token.as_deref().unwrap_or(refresh_token);
        if !valid_secret(replacement) {
            return Err(AuthError::InvalidResponse("Microsoft OAuth"));
        }
        self.minecraft_session(&response.access_token, replacement.to_owned())
            .await
    }

    async fn oauth_token(&self, form: &[(&str, &str)]) -> Result<OAuthToken, AuthError> {
        let response = checked_response(
            self.http
                .post(&self.endpoints.oauth_token)
                .form(form)
                .send()
                .await,
            "Microsoft OAuth",
        )
        .await?;
        let token: OAuthToken = decode(response, "Microsoft OAuth").await?;
        if !valid_secret(&token.access_token) || !token.token_type.eq_ignore_ascii_case("bearer") {
            return Err(AuthError::InvalidResponse("Microsoft OAuth"));
        }
        Ok(token)
    }

    async fn minecraft_session(
        &self,
        microsoft_access_token: &str,
        refresh_token: String,
    ) -> Result<AuthenticatedAccount, AuthError> {
        #[derive(Serialize)]
        #[serde(rename_all = "PascalCase")]
        struct XboxProperties<'a> {
            auth_method: &'static str,
            site_name: &'static str,
            rps_ticket: &'a str,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "PascalCase")]
        struct XboxRequest<'a> {
            properties: XboxProperties<'a>,
            relying_party: &'static str,
            token_type: &'static str,
        }
        let ticket = format!("d={microsoft_access_token}");
        let xbox: XboxToken = decode(
            checked_response(
                self.http
                    .post(&self.endpoints.xbox)
                    .json(&XboxRequest {
                        properties: XboxProperties {
                            auth_method: "RPS",
                            site_name: "user.auth.xboxlive.com",
                            rps_ticket: &ticket,
                        },
                        relying_party: "http://auth.xboxlive.com",
                        token_type: "JWT",
                    })
                    .send()
                    .await,
                "Xbox Live",
            )
            .await?,
            "Xbox Live",
        )
        .await?;
        let user_hash = xbox.user_hash("Xbox Live")?.to_owned();
        #[derive(Serialize)]
        #[serde(rename_all = "PascalCase")]
        struct XstsProperties<'a> {
            sandbox_id: &'static str,
            user_tokens: [&'a str; 1],
        }
        #[derive(Serialize)]
        #[serde(rename_all = "PascalCase")]
        struct XstsRequest<'a> {
            properties: XstsProperties<'a>,
            relying_party: &'static str,
            token_type: &'static str,
        }
        let xsts: XboxToken = decode(
            checked_response(
                self.http
                    .post(&self.endpoints.xsts)
                    .json(&XstsRequest {
                        properties: XstsProperties {
                            sandbox_id: "RETAIL",
                            user_tokens: [&xbox.token],
                        },
                        relying_party: "rp://api.minecraftservices.com/",
                        token_type: "JWT",
                    })
                    .send()
                    .await,
                "Xbox XSTS",
            )
            .await?,
            "Xbox XSTS",
        )
        .await?;
        if !secrets_equal(xsts.user_hash("Xbox XSTS")?, &user_hash) {
            return Err(AuthError::InvalidResponse("Xbox XSTS"));
        }
        let xuid = xsts.xuid("Xbox XSTS")?.map(str::to_owned);
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct MinecraftRequest {
            identity_token: String,
            ensure_legacy_enabled: bool,
        }
        let minecraft: MinecraftToken = decode(
            checked_response(
                self.http
                    .post(&self.endpoints.minecraft_login)
                    .json(&MinecraftRequest {
                        identity_token: format!("XBL3.0 x={user_hash};{}", xsts.token),
                        ensure_legacy_enabled: true,
                    })
                    .send()
                    .await,
                "Minecraft authentication",
            )
            .await?,
            "Minecraft authentication",
        )
        .await?;
        if !valid_secret(&minecraft.access_token)
            || !minecraft.token_type.eq_ignore_ascii_case("bearer")
            || minecraft.expires_in == 0
        {
            return Err(AuthError::InvalidResponse("Minecraft authentication"));
        }
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|now| now.as_secs().checked_add(minecraft.expires_in))
            .ok_or(AuthError::InvalidResponse("Minecraft authentication"))?;
        let entitlements: Entitlements = decode(
            checked_response(
                self.http
                    .get(&self.endpoints.entitlements)
                    .bearer_auth(&minecraft.access_token)
                    .send()
                    .await,
                "Minecraft entitlements",
            )
            .await?,
            "Minecraft entitlements",
        )
        .await?;
        entitlements.require_java()?;
        let response = self
            .http
            .get(&self.endpoints.profile)
            .bearer_auth(&minecraft.access_token)
            .send()
            .await
            .map_err(|_| AuthError::Network)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AuthError::MissingMinecraftProfile);
        }
        let profile: GameProfile = decode(
            checked_response(Ok(response), "Minecraft profile").await?,
            "Minecraft profile",
        )
        .await?;
        let profile = profile.validated()?;
        Ok(AuthenticatedAccount {
            session: AuthSession {
                profile_id: profile.id,
                profile_name: profile.name,
                access_token: Zeroizing::new(minecraft.access_token),
                xuid,
                client_id: Some(self.config.client_id.clone()),
                expires_at: Some(expires_at),
            },
            credentials: Credentials::Microsoft { refresh_token },
        })
    }
}

#[derive(Deserialize)]
struct OAuthToken {
    access_token: String,
    token_type: String,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XboxToken {
    token: String,
    display_claims: DisplayClaims,
}
#[derive(Deserialize)]
struct DisplayClaims {
    xui: Vec<UserClaim>,
}
#[derive(Deserialize)]
struct UserClaim {
    uhs: String,
    #[serde(default)]
    xid: Option<String>,
}

impl XboxToken {
    fn user_hash(&self, service: &'static str) -> Result<&str, AuthError> {
        if !valid_secret(&self.token) {
            return Err(AuthError::InvalidResponse(service));
        }
        let [claim] = self.display_claims.xui.as_slice() else {
            return Err(AuthError::InvalidResponse(service));
        };
        if claim.uhs.is_empty()
            || claim.uhs.len() > 128
            || !claim.uhs.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(AuthError::InvalidResponse(service));
        }
        Ok(&claim.uhs)
    }

    fn xuid(&self, service: &'static str) -> Result<Option<&str>, AuthError> {
        let [claim] = self.display_claims.xui.as_slice() else {
            return Err(AuthError::InvalidResponse(service));
        };
        if let Some(xuid) = claim.xid.as_deref()
            && (xuid.is_empty() || xuid.len() > 32 || !xuid.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(AuthError::InvalidResponse(service));
        }
        Ok(claim.xid.as_deref())
    }
}

#[derive(Deserialize)]
struct MinecraftToken {
    access_token: String,
    token_type: String,
    expires_in: u64,
}
#[derive(Deserialize)]
struct Entitlements {
    items: Vec<Entitlement>,
}
#[derive(Deserialize)]
struct Entitlement {
    name: String,
}

impl Entitlements {
    fn require_java(&self) -> Result<(), AuthError> {
        if self
            .items
            .iter()
            .any(|item| matches!(item.name.as_str(), "game_minecraft" | "product_minecraft"))
        {
            Ok(())
        } else {
            Err(AuthError::MissingEntitlement)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> MicrosoftClient {
        MicrosoftClient::new(
            MicrosoftConfig::new(
                "configured-client-id",
                Url::parse("http://127.0.0.1:8123/callback").unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn authorization() -> MicrosoftAuthorization {
        client()
            .authorization_with(
                PkceVerifier::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into())
                    .unwrap(),
                "01234567890123456789012345678901".into(),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn local_mock_full_chain_and_refresh_rotation() {
        for owned in [true, false] {
            let mut replies = vec![
                (
                    200,
                    serde_json::json!({"access_token":"msa", "token_type":"Bearer", "refresh_token":"rotated"}),
                ),
                (
                    200,
                    serde_json::json!({"Token":"xbox", "DisplayClaims":{"xui":[{"uhs":"123"}]}}),
                ),
                (
                    200,
                    serde_json::json!({"Token":"xsts", "DisplayClaims":{"xui":[{"uhs":"123"}]}}),
                ),
                (
                    200,
                    serde_json::json!({"access_token":"minecraft", "token_type":"Bearer", "expires_in":3600}),
                ),
                (
                    200,
                    serde_json::json!({"items":if owned {vec![serde_json::json!({"name":"game_minecraft"})]} else {vec![]}}),
                ),
            ];
            if owned {
                replies.push((
                    200,
                    serde_json::json!({"id":"123456781234423481231234567890ab", "name":"Player"}),
                ));
            }
            let (base, server) = crate::tests::mock_server(replies).await;
            let mut client = client();
            client.http = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            client.endpoints = MicrosoftEndpoints {
                oauth_token: format!("{base}/0"),
                xbox: format!("{base}/1"),
                xsts: format!("{base}/2"),
                minecraft_login: format!("{base}/3"),
                entitlements: format!("{base}/4"),
                profile: format!("{base}/5"),
            };
            let result = client
                .refresh(&Credentials::Microsoft {
                    refresh_token: "old".into(),
                })
                .await;
            if owned {
                let account = result.unwrap();
                assert_eq!(account.session.access_token.as_str(), "minecraft");
                assert!(
                    matches!(&account.credentials, Credentials::Microsoft { refresh_token } if refresh_token == "rotated")
                );
            } else {
                assert!(matches!(result, Err(AuthError::MissingEntitlement)));
            }
            let requests = server.await.unwrap();
            assert!(requests[0].contains("grant_type=refresh_token"));
            assert!(requests[1].contains("d=msa"));
            assert!(requests[3].contains("XBL3.0 x=123;xsts"));
            assert!(
                requests[4]
                    .to_ascii_lowercase()
                    .contains("authorization: bearer minecraft")
            );
        }
    }

    fn mock_client(base: &str) -> MicrosoftClient {
        let mut client = client();
        client.http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();
        client.endpoints = MicrosoftEndpoints {
            oauth_token: format!("{base}/oauth"),
            xbox: format!("{base}/xbox"),
            xsts: format!("{base}/xsts"),
            minecraft_login: format!("{base}/login"),
            entitlements: format!("{base}/entitlements"),
            profile: format!("{base}/profile"),
        };
        client
    }

    fn success_responses(rotated: bool) -> Vec<(u16, serde_json::Value)> {
        let mut oauth = serde_json::json!({"access_token": "ms-secret", "token_type": "Bearer"});
        if rotated {
            oauth["refresh_token"] = "rotated-refresh-secret".into();
        }
        vec![
            (200, oauth),
            (
                200,
                serde_json::json!({"Token": "xbox-secret", "DisplayClaims": {"xui": [{"uhs": "12345"}]}}),
            ),
            (
                200,
                serde_json::json!({"Token": "xsts-secret", "DisplayClaims": {"xui": [{"uhs": "12345"}]}}),
            ),
            (
                200,
                serde_json::json!({"access_token": "minecraft-secret", "token_type": "Bearer", "expires_in": 3600}),
            ),
            (
                200,
                serde_json::json!({"items": [{"name": "game_minecraft"}]}),
            ),
            (
                200,
                serde_json::json!({"id": "123456781234423481231234567890ab", "name": "Player_1"}),
            ),
        ]
    }

    fn request_body(request: &str) -> serde_json::Value {
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn microsoft_code_exchange_and_complete_chain() {
        let (base, server) = crate::tests::mock_server(success_responses(true)).await;
        let client = mock_client(&base);
        let result = client.complete_authorization(authorization(), &Url::parse(
            "http://127.0.0.1:8123/callback?code=code-secret&state=01234567890123456789012345678901"
        ).unwrap()).await.unwrap();
        assert_eq!(result.session.profile_name, "Player_1");
        assert_eq!(result.session.access_token.as_str(), "minecraft-secret");
        assert!(result.session.expires_at.is_some());
        assert!(
            matches!(&result.credentials, Credentials::Microsoft { refresh_token } if refresh_token == "rotated-refresh-secret")
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 6);
        let form =
            url::form_urlencoded::parse(requests[0].split("\r\n\r\n").nth(1).unwrap().as_bytes())
                .into_owned()
                .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(form["grant_type"], "authorization_code");
        assert_eq!(form["client_id"], "configured-client-id");
        assert_eq!(form["code"], "code-secret");
        assert_eq!(
            form["code_verifier"],
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        );
        assert!(!form.contains_key("client_secret"));
        assert_eq!(
            request_body(&requests[1])["Properties"]["RpsTicket"],
            "d=ms-secret"
        );
        assert_eq!(
            request_body(&requests[2])["Properties"]["UserTokens"][0],
            "xbox-secret"
        );
        assert_eq!(
            request_body(&requests[3])["identityToken"],
            "XBL3.0 x=12345;xsts-secret"
        );
        assert!(
            requests[4]
                .to_ascii_lowercase()
                .contains("authorization: bearer minecraft-secret")
        );
        assert!(
            requests[5]
                .to_ascii_lowercase()
                .contains("authorization: bearer minecraft-secret")
        );
    }

    #[tokio::test]
    async fn microsoft_refresh_rotates_or_retains_refresh_token() {
        for rotated in [false, true] {
            let (base, server) = crate::tests::mock_server(success_responses(rotated)).await;
            let client = mock_client(&base);
            let result = client
                .refresh(&Credentials::Microsoft {
                    refresh_token: "old-refresh-secret".into(),
                })
                .await
                .unwrap();
            let expected = if rotated {
                "rotated-refresh-secret"
            } else {
                "old-refresh-secret"
            };
            assert!(
                matches!(&result.credentials, Credentials::Microsoft { refresh_token } if refresh_token == expected)
            );
            let requests = server.await.unwrap();
            let form = url::form_urlencoded::parse(
                requests[0].split("\r\n\r\n").nth(1).unwrap().as_bytes(),
            )
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();
            assert_eq!(form["grant_type"], "refresh_token");
            assert_eq!(form["refresh_token"], "old-refresh-secret");
        }
    }

    #[tokio::test]
    async fn microsoft_refuses_missing_entitlement_and_profile() {
        let mut responses = success_responses(true);
        responses.truncate(5);
        responses[4].1 = serde_json::json!({"items": []});
        let (base, server) = crate::tests::mock_server(responses).await;
        let Err(error) = mock_client(&base)
            .refresh(&Credentials::Microsoft {
                refresh_token: "secret".into(),
            })
            .await
        else {
            panic!("unowned game accepted")
        };
        assert_eq!(error, AuthError::MissingEntitlement);
        assert_eq!(server.await.unwrap().len(), 5);
        let mut responses = success_responses(true);
        responses[5] = (404, serde_json::json!({"errorMessage": "secret"}));
        let (base, server) = crate::tests::mock_server(responses).await;
        let Err(error) = mock_client(&base)
            .refresh(&Credentials::Microsoft {
                refresh_token: "secret".into(),
            })
            .await
        else {
            panic!("missing profile accepted")
        };
        assert_eq!(error, AuthError::MissingMinecraftProfile);
        assert_eq!(server.await.unwrap().len(), 6);
    }

    #[tokio::test]
    async fn microsoft_rejects_redirect_without_forwarding_credentials() {
        let (base, server) =
            crate::tests::mock_server(vec![(302, serde_json::json!({"error": "secret"}))]).await;
        let Err(error) = mock_client(&base)
            .refresh(&Credentials::Microsoft {
                refresh_token: "secret".into(),
            })
            .await
        else {
            panic!("redirect accepted")
        };
        assert_eq!(
            error,
            AuthError::Http {
                service: "Microsoft OAuth",
                status: 302
            }
        );
        assert!(!format!("{error:?}: {error}").contains("secret"));
        assert_eq!(server.await.unwrap().len(), 1);
    }

    #[test]
    fn expired_callback_is_rejected_before_token_exchange() {
        let mut auth = authorization();
        auth.started = std::time::Instant::now() - std::time::Duration::from_secs(601);
        assert_eq!(auth.callback_code(&Url::parse("http://127.0.0.1:8123/callback?code=secret&state=01234567890123456789012345678901").unwrap()), Err(AuthError::Expired));
    }

    #[test]
    fn rfc7636_s256_vector() {
        let verifier =
            PkceVerifier::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into())
                .unwrap();
        assert_eq!(
            verifier.challenge(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn pkce_rejects_invalid_verifiers() {
        for value in [
            "a".repeat(42),
            "a".repeat(129),
            "!".repeat(43),
            "é".repeat(43),
        ] {
            assert!(PkceVerifier::from_verifier(value).is_err());
        }
        assert!(PkceVerifier::from_verifier("-._~".repeat(32)).is_ok());
    }

    #[test]
    fn authorization_url_has_public_client_pkce_parameters() {
        let auth = authorization();
        let params = auth
            .url()
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(params.get("client_id").unwrap(), "configured-client-id");
        assert_eq!(params.get("scope").unwrap(), SCOPE);
        assert_eq!(params.get("response_type").unwrap(), "code");
        assert_eq!(params.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(
            params.get("code_challenge").unwrap(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert!(!params.contains_key("client_secret"));
        assert!(!params.contains_key("code_verifier"));
        assert_eq!(
            params.get("redirect_uri").unwrap(),
            "http://127.0.0.1:8123/callback"
        );
    }

    #[test]
    fn callback_checks_state_uri_duplicates_and_denial() {
        let auth = authorization();
        let valid = "http://127.0.0.1:8123/callback?code=secret-code&state=01234567890123456789012345678901";
        assert_eq!(
            auth.callback_code(&Url::parse(valid).unwrap()).unwrap(),
            "secret-code"
        );
        for bad in [
            valid.replace("8123", "8124"),
            valid.replace("/callback", "/other"),
            valid.replace("127.0.0.1", "example.com"),
            valid.replace("01234567890123456789012345678901", "invalid"),
            format!("{valid}&state=duplicate"),
            format!("{valid}&code=duplicate"),
            format!("{valid}#fragment"),
            valid.replace("code=secret-code&", ""),
            valid.replace("&state=01234567890123456789012345678901", ""),
        ] {
            assert_eq!(
                auth.callback_code(&Url::parse(&bad).unwrap()),
                Err(AuthError::InvalidCallback)
            );
        }
        let denial = valid.replace(
            "code=secret-code",
            "error=access_denied&error_description=secret",
        );
        let error = auth
            .callback_code(&Url::parse(&denial).unwrap())
            .unwrap_err();
        assert_eq!(error, AuthError::AuthorizationDenied);
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn configuration_rejects_unsafe_redirects_and_missing_client_id() {
        for uri in [
            "http://example.com/cb",
            "http://localhost/cb",
            "file:///tmp/cb",
            "https://user:secret@example.com/cb",
            "https://example.com/cb?state=bad",
            "https://example.com/cb#bad",
        ] {
            assert!(MicrosoftConfig::new("configured-id", Url::parse(uri).unwrap()).is_err());
        }
        for uri in [
            "https://example.com/cb",
            "http://127.0.0.1:8080/cb",
            "http://[::1]:8080/cb",
        ] {
            assert!(MicrosoftConfig::new("configured-id", Url::parse(uri).unwrap()).is_ok());
        }
        assert!(MicrosoftConfig::new("", Url::parse("https://example.com/cb").unwrap()).is_err());
    }

    #[test]
    fn entitlements_require_java_not_arbitrary_products() {
        for name in ["game_minecraft", "product_minecraft"] {
            assert!(
                Entitlements {
                    items: vec![Entitlement { name: name.into() }]
                }
                .require_java()
                .is_ok()
            );
        }
        assert_eq!(
            Entitlements {
                items: vec![Entitlement {
                    name: "minecraft_dungeons".into()
                }]
            }
            .require_java(),
            Err(AuthError::MissingEntitlement)
        );
        assert_eq!(
            Entitlements { items: vec![] }.require_java(),
            Err(AuthError::MissingEntitlement)
        );
    }

    #[test]
    fn xbox_claims_are_strictly_validated() {
        let mut token = XboxToken {
            token: "secret".into(),
            display_claims: DisplayClaims {
                xui: vec![UserClaim {
                    uhs: "12345".into(),
                    xid: Some("987654321".into()),
                }],
            },
        };
        assert_eq!(token.user_hash("Xbox").unwrap(), "12345");
        token.display_claims.xui[0].uhs = "123;injection".into();
        assert!(token.user_hash("Xbox").is_err());
        token.display_claims.xui.clear();
        assert!(token.user_hash("Xbox").is_err());
    }
}
