//! Encrypted on-disk store for the app's secrets, keyed by one random key held
//! in the OS keyring.
//!
//! Why not put the secrets in the keyring directly: the keyring crate's
//! Secret Service backend opens a new D-Bus connection per operation, and two
//! operations in quick succession reliably crashed gnome-keyring-daemon 50.0
//! (`gkd_secret_service_get_pkcs11_session: assertion 'client' failed`) on
//! WattMail's and WattDrive's start-ups. With this design the keyring sees one
//! read per process lifetime, one write on first run, and one delete on sign-out.
//!
//! File format: 12-byte AES-GCM nonce followed by the ciphertext of the JSON
//! payload. Written to a temp file and renamed, mode 0600.

use std::path::{Path, PathBuf};
use std::time::Duration;

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, Nonce};
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;

const KEYRING_SERVICE: &str = "WattDrive";
const KEYRING_ENTRY: &str = "vault-key";
const NONCE_LEN: usize = 12;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("keyring: {0}")]
    Keyring(#[from] keyring::Error),
    #[error("vault file: {0}")]
    Io(#[from] std::io::Error),
    #[error("vault payload is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("vault key in keyring is malformed")]
    BadKey,
    #[error("vault file could not be decrypted (wrong key or corrupt file)")]
    Decrypt,
}

pub struct Vault {
    path: PathBuf,
    key: Key<Aes256Gcm>,
}

impl Vault {
    /// Open the vault at `path`, fetching (or on first run creating) its key in
    /// the keyring. Exactly one keyring read, plus one write on first run. A
    /// transient keyring failure is retried once after a short pause, because
    /// the daemon is restarted by systemd within a fraction of a second.
    pub fn open(path: PathBuf) -> Result<Self, VaultError> {
        let key = match load_or_create_key() {
            Ok(k) => k,
            Err(VaultError::Keyring(e)) if !matches!(e, keyring::Error::NoEntry) => {
                tracing::warn!("keyring unavailable ({e}); retrying once");
                std::thread::sleep(Duration::from_millis(1200));
                load_or_create_key()?
            }
            Err(e) => return Err(e),
        };
        Ok(Self { path, key })
    }

    /// A vault with a caller-supplied key: tests, and any future export path.
    pub fn with_key(path: PathBuf, key: [u8; 32]) -> Self {
        Self {
            path,
            key: Key::<Aes256Gcm>::from(key),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Decrypt and decode the payload; `None` when no file exists.
    pub fn load<T: DeserializeOwned>(&self) -> Result<Option<T>, VaultError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if bytes.len() < NONCE_LEN + 16 {
            return Err(VaultError::Decrypt);
        }
        let (nonce, ciphertext) = bytes.split_at(NONCE_LEN);
        let plain = Aes256Gcm::new(&self.key)
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| VaultError::Decrypt)?;
        Ok(Some(serde_json::from_slice(&plain)?))
    }

    /// Encrypt and write atomically.
    pub fn save<T: Serialize>(&self, value: &T) -> Result<(), VaultError> {
        let plain = serde_json::to_vec(value)?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = Aes256Gcm::new(&self.key)
            .encrypt(&nonce, plain.as_ref())
            .map_err(|_| VaultError::Decrypt)?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ciphertext);

        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            std::io::Write::write_all(&mut file, &out)?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Remove the file. The key stays: deleting it would be one more keyring
    /// operation, and a key without a file protects nothing.
    pub fn clear(&self) -> Result<(), VaultError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

fn load_or_create_key() -> Result<Key<Aes256Gcm>, VaultError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ENTRY)?;
    let b64 = base64::engine::general_purpose::STANDARD;
    match entry.get_password() {
        Ok(encoded) => {
            let bytes = b64.decode(encoded.trim()).map_err(|_| VaultError::BadKey)?;
            let arr: [u8; 32] = bytes.try_into().map_err(|_| VaultError::BadKey)?;
            Ok(Key::<Aes256Gcm>::from(arr))
        }
        Err(keyring::Error::NoEntry) => {
            let key = Aes256Gcm::generate_key(&mut OsRng);
            entry.set_password(&b64.encode(key))?;
            tracing::info!("created a new vault key in the keyring");
            Ok(key)
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Payload {
        secret: String,
        n: u32,
    }

    #[test]
    fn roundtrip_atomic_and_mode_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep/vault.bin");
        let v = Vault::with_key(path.clone(), [7u8; 32]);
        assert_eq!(v.load::<Payload>().unwrap(), None, "no file yet");
        let p = Payload {
            secret: "hunter2".into(),
            n: 3,
        };
        v.save(&p).unwrap();
        assert_eq!(v.load::<Payload>().unwrap(), Some(p));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            !path.with_extension("bin.tmp").exists(),
            "temp file renamed away"
        );
        let raw = std::fs::read(&path).unwrap();
        assert!(
            !raw.windows(7).any(|w| w == b"hunter2"),
            "plaintext never on disk"
        );
    }

    #[test]
    fn wrong_key_or_tampering_is_an_error_not_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.bin");
        Vault::with_key(path.clone(), [1u8; 32])
            .save(&Payload {
                secret: "s".into(),
                n: 1,
            })
            .unwrap();
        let other = Vault::with_key(path.clone(), [2u8; 32]);
        assert!(matches!(other.load::<Payload>(), Err(VaultError::Decrypt)));
        let mut raw = std::fs::read(&path).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        std::fs::write(&path, raw).unwrap();
        let same = Vault::with_key(path.clone(), [1u8; 32]);
        assert!(matches!(same.load::<Payload>(), Err(VaultError::Decrypt)));
        same.clear().unwrap();
        assert_eq!(same.load::<Payload>().unwrap(), None);
        same.clear().unwrap();
    }
}
