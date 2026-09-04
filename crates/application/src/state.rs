//! Port for the per-path sync records, plus an in-memory implementation used
//! by tests (and as a fallback when the database cannot be opened).

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use thiserror::Error;
use wattdrive_domain::{RelPath, SyncEntry};

#[derive(Debug, Error)]
pub enum StateError {
    #[error("sync state store: {0}")]
    Store(String),
}

/// What the last sync recorded, keyed by relative path.
#[async_trait]
pub trait StateStore: Send + Sync {
    async fn load_all(&self) -> Result<BTreeMap<RelPath, SyncEntry>, StateError>;
    async fn put(&self, path: &RelPath, entry: &SyncEntry) -> Result<(), StateError>;
    async fn remove(&self, path: &RelPath) -> Result<(), StateError>;
    /// Remove `path` and everything inside it.
    async fn remove_subtree(&self, path: &RelPath) -> Result<(), StateError>;
    async fn clear(&self) -> Result<(), StateError>;
}

/// Records held in memory only. Every restart is a "first sync" with it, so
/// it is for tests; the app uses the SQLite store.
#[derive(Default)]
pub struct MemoryStateStore {
    entries: Mutex<BTreeMap<RelPath, SyncEntry>>,
}

impl MemoryStateStore {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<RelPath, SyncEntry>>, StateError> {
        self.entries
            .lock()
            .map_err(|_| StateError::Store("state mutex poisoned".into()))
    }
}

#[async_trait]
impl StateStore for MemoryStateStore {
    async fn load_all(&self) -> Result<BTreeMap<RelPath, SyncEntry>, StateError> {
        Ok(self.lock()?.clone())
    }

    async fn put(&self, path: &RelPath, entry: &SyncEntry) -> Result<(), StateError> {
        self.lock()?.insert(path.clone(), entry.clone());
        Ok(())
    }

    async fn remove(&self, path: &RelPath) -> Result<(), StateError> {
        self.lock()?.remove(path);
        Ok(())
    }

    async fn remove_subtree(&self, path: &RelPath) -> Result<(), StateError> {
        self.lock()?.retain(|p, _| p != path && !p.is_inside(path));
        Ok(())
    }

    async fn clear(&self) -> Result<(), StateError> {
        self.lock()?.clear();
        Ok(())
    }
}
