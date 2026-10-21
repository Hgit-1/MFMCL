use crate::{
    AuthError, AuthenticatedAccount, MicrosoftAuthorization, MicrosoftClient, MicrosoftConfig,
    Result,
};
use url::Url;

/// Single-use public-client PKCE facade. Caller binds the loopback listener and
/// passes its exact registered URI; the object must stay in backend memory.
pub struct MicrosoftFlow {
    client: MicrosoftClient,
    authorization: MicrosoftAuthorization,
}
impl MicrosoftFlow {
    pub fn begin(client_id: impl Into<String>, redirect_uri: &str) -> Result<Self> {
        let uri = Url::parse(redirect_uri).map_err(|_| AuthError::InvalidCallback)?;
        if uri.scheme() != "http"
            || !matches!(uri.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
                && !matches!(uri.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback())
        {
            return Err(AuthError::InvalidConfiguration(
                "flow requires an HTTP literal-loopback redirect",
            ));
        }
        let client = MicrosoftClient::new(MicrosoftConfig::new(client_id, uri)?)?;
        let authorization = client.begin_authorization()?;
        Ok(Self {
            client,
            authorization,
        })
    }
    pub fn authorization_url(&self) -> &Url {
        self.authorization.url()
    }
    pub async fn finish(self, code: &str, state: &str) -> Result<AuthenticatedAccount> {
        let mut callback = self.client.redirect_uri().clone();
        callback
            .query_pairs_mut()
            .append_pair("code", code)
            .append_pair("state", state);
        self.client
            .complete_authorization(self.authorization, &callback)
            .await
    }
    pub async fn finish_callback(self, callback: &Url) -> Result<AuthenticatedAccount> {
        self.client
            .complete_authorization(self.authorization, callback)
            .await
    }
}
