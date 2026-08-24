use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};

use crate::config::Config;

/// Ponduin-specific credential store that uses the Config system
///
/// This implementation stores OAuth credentials in the ponduin configuration
/// system, which handles secure storage (e.g., keychain integration).

#[derive(Clone)]
pub struct PonduinCredentialStore {
    name: String,
    allow_keyring_access: bool,
}

impl PonduinCredentialStore {
    pub fn new(name: String) -> Self {
        Self {
            name,
            allow_keyring_access: true,
        }
    }

    pub fn noninteractive(name: String) -> Self {
        Self {
            name,
            allow_keyring_access: false,
        }
    }

    fn secret_key(&self) -> String {
        format!("oauth_creds_{}", self.name)
    }
}

#[async_trait::async_trait]
impl CredentialStore for PonduinCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let config = Config::global();
        let key = self.secret_key();

        let credentials = if self.allow_keyring_access {
            config.get_secret::<StoredCredentials>(&key)
        } else {
            config.get_secret_without_keyring_access::<StoredCredentials>(&key)
        };

        match credentials {
            Ok(credentials) => Ok(Some(credentials)),
            Err(_) => Ok(None), // No credentials found
        }
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let config = Config::global();
        let key = self.secret_key();

        config
            .set_secret(&key, &credentials)
            .map_err(|e| AuthError::InternalError(format!("Failed to save credentials: {}", e)))
    }

    async fn clear(&self) -> Result<(), AuthError> {
        let config = Config::global();
        let key = self.secret_key();

        config
            .delete_secret(&key)
            .map_err(|e| AuthError::InternalError(format!("Failed to clear credentials: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noninteractive_store_defers_keyring_access() {
        assert!(!PonduinCredentialStore::noninteractive("test".to_string()).allow_keyring_access);
        assert!(PonduinCredentialStore::new("test".to_string()).allow_keyring_access);
    }
}
