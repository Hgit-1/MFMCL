use crate::{AuthError, Result, Secret};

/// Namespaced OS credential storage. Store refresh tokens/client tokens under
/// opaque account keys, and keep non-secret profile/provider metadata separately.
/// These calls may block: invoke them using the host's blocking worker facility.
pub struct CredentialStore {
    service: String,
}
impl CredentialStore {
    pub fn new(service: impl Into<String>) -> Result<Self> {
        let service = service.into();
        if service.trim().is_empty() {
            return Err(AuthError::InvalidConfiguration(
                "credential service and account keys must be nonempty",
            ));
        }
        Ok(Self { service })
    }
    fn entry(&self, account: &str) -> Result<keyring::Entry> {
        if account.trim().is_empty() {
            return Err(AuthError::InvalidConfiguration(
                "credential service and account keys must be nonempty",
            ));
        }
        keyring::Entry::new(&self.service, account).map_err(|_| AuthError::CredentialStore)
    }
    pub fn put(&self, account: &str, secret: &Secret) -> Result<()> {
        self.entry(account)?
            .set_password(secret.expose())
            .map_err(|_| AuthError::CredentialStore)
    }
    pub fn get(&self, account: &str) -> Result<Option<Secret>> {
        match self.entry(account)?.get_password() {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(AuthError::CredentialStore),
        }
    }
    pub fn delete(&self, account: &str) -> Result<()> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(AuthError::CredentialStore),
        }
    }
}
