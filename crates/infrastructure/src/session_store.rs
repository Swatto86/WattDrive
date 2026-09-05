//! OS keyring persistence for the iCloud session and the Apple ID credentials.
//!
//! Linux Secret Service (GNOME Keyring / KWallet through libsecret) has no
//! practical size limit, so each item is one JSON entry. Calls are blocking
//! D-Bus round-trips: callers wrap them in `spawn_blocking`.
//!
//! Every call runs under one process-wide lock. The keyring crate opens a
//! fresh D-Bus connection per operation, and gnome-keyring-daemon 50.0 aborted
//! (`gkd_secret_service_get_pkcs11_session: assertion 'client' failed`) when
//! two of those connections negotiated sessions at the same instant during
//! WattDrive's first sign-in. Serialising our side removes that trigger.

use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use thiserror::Error;

static KEYRING_LOCK: Mutex<()> = Mutex::new(());

fn serialised() -> MutexGuard<'static, ()> {
    KEYRING_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

use crate::icloud::SavedSession;

const SERVICE: &str = "WattDrive";
const SESSION_ENTRY: &str = "icloud-session";
const CREDENTIALS_ENTRY: &str = "icloud-credentials";

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("keyring: {0}")]
    Keyring(#[from] keyring::Error),
    #[error("stored value is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// The Apple ID and password, kept so the app can renew its session without
/// asking again (Apple's web session needs the password for SRP; the trust
/// token only skips the second factor).
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredCredentials {
    pub apple_id: String,
    pub password: String,
    /// Apple's 30-day two-factor trust token, duplicated here from the session
    /// item: if the session item is lost, this still lets the next sign-in
    /// skip the second factor.
    #[serde(default)]
    pub trust_token: String,
}

impl std::fmt::Debug for StoredCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredCredentials")
            .field("apple_id", &self.apple_id)
            .field("password", &"<redacted>")
            .finish()
    }
}

pub struct SessionStore;

impl SessionStore {
    fn entry(name: &str) -> Result<keyring::Entry, keyring::Error> {
        keyring::Entry::new(SERVICE, name)
    }

    fn load<T: for<'de> Deserialize<'de>>(name: &str) -> Result<Option<T>, StoreError> {
        let _one_at_a_time = serialised();
        match Self::entry(name)?.get_password() {
            // An item whose secret came back blank (seen once after a keyring
            // daemon crash) is treated as absent, not as corrupt.
            Ok(json) if json.trim().is_empty() => Ok(None),
            Ok(json) => Ok(Some(serde_json::from_str(&json)?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save<T: Serialize>(name: &str, value: &T) -> Result<(), StoreError> {
        let _one_at_a_time = serialised();
        Ok(Self::entry(name)?.set_password(&serde_json::to_string(value)?)?)
    }

    fn delete(name: &str) -> Result<(), StoreError> {
        let _one_at_a_time = serialised();
        match Self::entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn load_session() -> Result<Option<SavedSession>, StoreError> {
        Self::load(SESSION_ENTRY)
    }

    pub fn save_session(session: &SavedSession) -> Result<(), StoreError> {
        Self::save(SESSION_ENTRY, session)
    }

    pub fn load_credentials() -> Result<Option<StoredCredentials>, StoreError> {
        Self::load(CREDENTIALS_ENTRY)
    }

    pub fn save_credentials(creds: &StoredCredentials) -> Result<(), StoreError> {
        Self::save(CREDENTIALS_ENTRY, creds)
    }

    /// Sign out: remove both entries.
    pub fn clear() -> Result<(), StoreError> {
        Self::delete(SESSION_ENTRY)?;
        Self::delete(CREDENTIALS_ENTRY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_without_a_trust_token_still_load() {
        let c: StoredCredentials =
            serde_json::from_str(r#"{"apple_id":"a@b.c","password":"p"}"#).unwrap();
        assert_eq!(c.trust_token, "");
    }

    #[test]
    fn credentials_debug_never_prints_the_password() {
        let c = StoredCredentials {
            apple_id: "a@b.c".into(),
            password: "hunter2".into(),
            trust_token: "tt".into(),
        };
        let dbg = format!("{c:?}");
        assert!(dbg.contains("a@b.c"));
        assert!(!dbg.contains("hunter2"));
    }
}
