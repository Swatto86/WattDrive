//! One-off migration from the first debug builds, which stored the session and
//! credentials as two Secret Service items. Runs only when the secrets file
//! does not exist yet; reads the old items, writes the vault, deletes them.
//! Operations are spaced out because back-to-back Secret Service calls are
//! what crashed gnome-keyring-daemon 50.0 on this machine.

use std::path::Path;
use std::time::Duration;

use wattdrive_infrastructure::icloud::SavedSession;
use wattdrive_infrastructure::session_store::{SessionStore, StoredCredentials};

const OLD_SERVICE: &str = "WattDrive";
const OLD_SESSION: &str = "icloud-session";
const OLD_CREDENTIALS: &str = "icloud-credentials";
const PACING: Duration = Duration::from_millis(400);

fn read_old(name: &str) -> Option<String> {
    let entry = keyring::Entry::new(OLD_SERVICE, name).ok()?;
    match entry.get_password() {
        Ok(v) if !v.trim().is_empty() => Some(v),
        Ok(_) => None,
        Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            tracing::warn!("migration: could not read old {name}: {e}");
            None
        }
    }
}

fn delete_old(name: &str) {
    if let Ok(entry) = keyring::Entry::new(OLD_SERVICE, name) {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => tracing::warn!("migration: could not delete old {name}: {e}"),
        }
    }
}

/// Move any pre-vault keyring items into `store`. No-op once the secrets
/// file exists.
pub fn run(store: &SessionStore, secrets_path: &Path) {
    if secrets_path.exists() {
        return;
    }
    std::thread::sleep(PACING);
    let Some(creds_json) = read_old(OLD_CREDENTIALS) else {
        return;
    };
    std::thread::sleep(PACING);
    let session_json = read_old(OLD_SESSION);

    let mut creds: StoredCredentials = match serde_json::from_str(&creds_json) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("migration: old credentials unreadable: {e}");
            return;
        }
    };
    let session: Option<SavedSession> = session_json.and_then(|j| serde_json::from_str(&j).ok());
    if creds.trust_token.is_empty() {
        if let Some(s) = &session {
            creds.trust_token = s.trust_token.clone();
        }
    }
    if let Err(e) = store.save_credentials(&creds) {
        tracing::warn!("migration: could not write credentials: {e}");
        return;
    }
    if let Some(s) = &session {
        if let Err(e) = store.save_session(s) {
            tracing::warn!("migration: could not write session: {e}");
        }
    }
    tracing::info!("migrated sign-in from keyring items to the secrets file");
    std::thread::sleep(PACING);
    delete_old(OLD_CREDENTIALS);
    std::thread::sleep(PACING);
    delete_old(OLD_SESSION);
}
