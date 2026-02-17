use std::sync::Arc;

use crate::error::TrackerError;

const SERVICE_NAME: &str = "claude-tracker";

/// Trait for keyring operations, allowing injection of mocks in tests.
pub trait KeyringBackend: Send + Sync {
    fn get_session_key(&self, account_name: &str) -> Result<String, TrackerError>;
    fn set_session_key(&self, account_name: &str, session_key: &str) -> Result<(), TrackerError>;
    fn delete_session_key(&self, account_name: &str) -> Result<(), TrackerError>;
}

/// Real keyring backend that uses macOS Keychain.
pub struct SystemKeyring;

impl KeyringBackend for SystemKeyring {
    fn get_session_key(&self, account_name: &str) -> Result<String, TrackerError> {
        let entry = keyring::Entry::new(SERVICE_NAME, account_name)
            .map_err(|e| TrackerError::Keyring(format!("Failed to create keyring entry: {e}")))?;
        entry
            .get_password()
            .map_err(|e| TrackerError::Keyring(format!("Failed to get session key for '{account_name}': {e}")))
    }

    fn set_session_key(&self, account_name: &str, session_key: &str) -> Result<(), TrackerError> {
        let entry = keyring::Entry::new(SERVICE_NAME, account_name)
            .map_err(|e| TrackerError::Keyring(format!("Failed to create keyring entry: {e}")))?;
        entry
            .set_password(session_key)
            .map_err(|e| TrackerError::Keyring(format!("Failed to store session key for '{account_name}': {e}")))
    }

    fn delete_session_key(&self, account_name: &str) -> Result<(), TrackerError> {
        let entry = keyring::Entry::new(SERVICE_NAME, account_name)
            .map_err(|e| TrackerError::Keyring(format!("Failed to create keyring entry: {e}")))?;
        entry
            .delete_credential()
            .map_err(|e| TrackerError::Keyring(format!("Failed to delete session key for '{account_name}': {e}")))
    }
}

pub fn system_keyring() -> Arc<dyn KeyringBackend> {
    Arc::new(SystemKeyring)
}
