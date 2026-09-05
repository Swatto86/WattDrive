//! Persistence for the iCloud session and the Apple ID credentials: one
//! encrypted file (see [`crate::vault`]) whose key lives in the OS keyring.
//! Writes are serialised so a session save and a credentials save cannot
//! interleave.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::icloud::SavedSession;
use crate::vault::{Vault, VaultError};

pub type StoreError = VaultError;

/// The Apple ID and password, kept so the app can renew its session without
/// asking again (Apple's web session needs the password for SRP; the trust
/// token only skips the second factor).
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredCredentials {
    pub apple_id: String,
    pub password: String,
    /// Apple's 30-day two-factor trust token, duplicated from the session so a
    /// lost or reset session never costs a second factor.
    #[serde(default)]
    pub trust_token: String,
}

impl std::fmt::Debug for StoredCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredCredentials")
            .field("apple_id", &self.apple_id)
            .field("password", &"<redacted>")
            .field(
                "trust_token",
                &format!("<{} chars>", self.trust_token.len()),
            )
            .finish()
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Secrets {
    credentials: Option<StoredCredentials>,
    session: Option<SavedSession>,
}

pub struct SessionStore {
    vault: Vault,
    lock: Mutex<()>,
}

impl SessionStore {
    /// Open the store whose file is `path`. Performs the vault's single
    /// keyring read (plus a write on first run).
    pub fn open(path: PathBuf) -> Result<Self, StoreError> {
        Ok(Self {
            vault: Vault::open(path)?,
            lock: Mutex::new(()),
        })
    }

    /// A store with a caller-supplied key (tests).
    pub fn with_key(path: PathBuf, key: [u8; 32]) -> Self {
        Self {
            vault: Vault::with_key(path, key),
            lock: Mutex::new(()),
        }
    }

    fn read(&self) -> Result<Secrets, StoreError> {
        Ok(self.vault.load()?.unwrap_or_default())
    }

    fn update(&self, f: impl FnOnce(&mut Secrets)) -> Result<(), StoreError> {
        let _serialised = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        // A corrupt file must not block signing in again: start from empty.
        let mut secrets = self.read().unwrap_or_else(|e| {
            tracing::warn!("secrets file unreadable, starting afresh: {e}");
            Secrets::default()
        });
        f(&mut secrets);
        self.vault.save(&secrets)
    }

    pub fn load_session(&self) -> Result<Option<SavedSession>, StoreError> {
        let _serialised = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        Ok(self.read()?.session)
    }

    pub fn save_session(&self, session: &SavedSession) -> Result<(), StoreError> {
        self.update(|s| s.session = Some(session.clone()))
    }

    pub fn load_credentials(&self) -> Result<Option<StoredCredentials>, StoreError> {
        let _serialised = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        Ok(self.read()?.credentials)
    }

    pub fn save_credentials(&self, creds: &StoredCredentials) -> Result<(), StoreError> {
        self.update(|s| s.credentials = Some(creds.clone()))
    }

    /// Sign out: remove the whole file.
    pub fn clear(&self) -> Result<(), StoreError> {
        let _serialised = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        self.vault.clear()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_and_session_live_side_by_side() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::with_key(dir.path().join("secrets.bin"), [9u8; 32]);
        assert!(store.load_session().unwrap().is_none());
        store
            .save_credentials(&StoredCredentials {
                apple_id: "a@b.c".into(),
                password: "p".into(),
                trust_token: "tt".into(),
            })
            .unwrap();
        let mut session = SavedSession::default();
        session.trust_token = "tt".into();
        session.scnt = "s".into();
        store.save_session(&session).unwrap();
        // neither save clobbers the other
        assert_eq!(store.load_credentials().unwrap().unwrap().apple_id, "a@b.c");
        assert_eq!(store.load_session().unwrap().unwrap().scnt, "s");
        store.clear().unwrap();
        assert!(store.load_credentials().unwrap().is_none());
    }

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
        assert!(!dbg.contains("hunter2") && !dbg.contains("\"tt\""));
    }
}
