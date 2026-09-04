//! Carry out one planned action against the local folder, iCloud and the
//! state store. Each action is atomic from the planner's point of view: it
//! either completes and records its result, or fails and leaves the record as
//! it was so the next pass re-plans it.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use thiserror::Error;
use wattdrive_domain::{
    DriveError, ItemKind, RelPath, RemoteDrive, RemoteFile, RemoteId, SyncAction, SyncEntry,
};

use crate::ignore::PARTIAL_PREFIX;
use crate::local;
use crate::state::{StateError, StateStore};

/// Folder inside the sync root that holds locally "deleted" items, one batch
/// folder per pass. Inside the root so a move never crosses filesystems, and
/// ignored by the scanner through its `.wattdrive` prefix.
pub const TRASH_DIR_NAME: &str = ".wattdrive-trash";

#[derive(Debug, Error)]
pub enum ExecError {
    #[error(transparent)]
    Drive(#[from] DriveError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error("local I/O: {0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Other(String),
}

impl ExecError {
    /// Errors that mean "stop the pass" rather than "skip this item".
    pub fn aborts_pass(&self) -> bool {
        matches!(
            self,
            ExecError::Drive(
                DriveError::SignInRequired(_) | DriveError::RateLimited | DriveError::Network(_)
            )
        )
    }
}

pub struct Executor<'a> {
    pub root: &'a Path,
    pub trash_batch: PathBuf,
    pub drive: &'a dyn RemoteDrive,
    pub state: &'a dyn StateStore,
    pub folder_ids: HashMap<RelPath, RemoteId>,
    pub root_id: RemoteId,
    pub host: &'a str,
}

impl Executor<'_> {
    pub async fn execute(&mut self, action: &SyncAction) -> Result<(), ExecError> {
        match action {
            SyncAction::MoveLocalAside { path } => self.move_local_aside(path).await,
            SyncAction::CreateLocalFolder { path, remote_id } => {
                tokio::fs::create_dir_all(self.abs(path)).await?;
                self.record_folder(path, remote_id).await
            }
            SyncAction::CreateRemoteFolder { path } => {
                let parent = self.parent_id(path)?;
                let id = self.drive.create_folder(&parent, path.name()).await?;
                self.record_folder(path, &id).await
            }
            SyncAction::RecordFolder { path, remote_id } => {
                self.record_folder(path, remote_id).await
            }
            SyncAction::Download { path, remote } => self.download(path, remote).await,
            SyncAction::Upload { path, replaces } => self.upload(path, replaces.as_ref()).await,
            SyncAction::Adopt { path, remote } => {
                let (_, mtime) = stamp(self.abs(path)).await?;
                Ok(self
                    .state
                    .put(path, &SyncEntry::file(remote, mtime))
                    .await?)
            }
            SyncAction::TrashLocal { path, kind } => self.trash_local(path, *kind).await,
            SyncAction::TrashRemote {
                path,
                id,
                etag,
                kind,
            } => {
                self.drive.trash(id, etag).await?;
                self.forget(path, *kind).await
            }
            SyncAction::Forget { path } => Ok(self.state.remove(path).await?),
        }
    }

    fn abs(&self, path: &RelPath) -> PathBuf {
        self.root.join(path.as_str())
    }

    fn parent_id(&self, path: &RelPath) -> Result<RemoteId, ExecError> {
        match path.parent() {
            None => Ok(self.root_id.clone()),
            Some(parent) => {
                self.folder_ids.get(&parent).cloned().ok_or_else(|| {
                    ExecError::Other(format!("no iCloud id known for folder {parent}"))
                })
            }
        }
    }

    async fn record_folder(&mut self, path: &RelPath, id: &RemoteId) -> Result<(), ExecError> {
        self.state.put(path, &SyncEntry::folder(id.clone())).await?;
        self.folder_ids.insert(path.clone(), id.clone());
        Ok(())
    }

    async fn forget(&self, path: &RelPath, kind: ItemKind) -> Result<(), ExecError> {
        match kind {
            ItemKind::Folder => self.state.remove_subtree(path).await?,
            ItemKind::File => self.state.remove(path).await?,
        }
        Ok(())
    }

    async fn move_local_aside(&self, path: &RelPath) -> Result<(), ExecError> {
        let from = self.abs(path);
        let kind = if tokio::fs::metadata(&from).await?.is_dir() {
            ItemKind::Folder
        } else {
            ItemKind::File
        };
        let mut new_name = local::conflict_name(path.name(), self.host, SystemTime::now());
        let mut to = from.with_file_name(&new_name);
        if tokio::fs::try_exists(&to).await? {
            new_name = format!("{new_name}.{}", uuid::Uuid::new_v4().simple());
            to = from.with_file_name(&new_name);
        }
        tokio::fs::rename(&from, &to).await?;
        tracing::info!("kept local {path} as conflict copy {new_name}");
        self.forget(path, kind).await
    }

    async fn download(&self, path: &RelPath, remote: &RemoteFile) -> Result<(), ExecError> {
        let final_path = self.abs(path);
        let dir = final_path
            .parent()
            .ok_or_else(|| ExecError::Other(format!("{path} has no parent directory")))?;
        tokio::fs::create_dir_all(dir).await?;
        let tmp = dir.join(format!("{PARTIAL_PREFIX}{}", uuid::Uuid::new_v4().simple()));

        let result = async {
            self.drive.download(remote, &tmp).await?;
            let mtime = remote.modified_ms;
            let t = tmp.clone();
            blocking(move || local::set_mtime_ms(&t, mtime)).await?;
            tokio::fs::rename(&tmp, &final_path).await?;
            Ok::<(), ExecError>(())
        }
        .await;
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }

        let (size, mtime) = stamp(final_path).await?;
        if size != remote.size {
            tracing::warn!(
                "{path}: downloaded {size} bytes, iCloud reported {}",
                remote.size
            );
        }
        Ok(self
            .state
            .put(path, &SyncEntry::file(remote, mtime))
            .await?)
    }

    async fn upload(&self, path: &RelPath, replaces: Option<&RemoteFile>) -> Result<(), ExecError> {
        let src = self.abs(path);
        let (size, mtime) = stamp(src.clone()).await?;
        if let Some(old) = replaces {
            self.drive.trash(&old.id, &old.etag).await?;
        }
        let parent = self.parent_id(path)?;
        let remote = self.drive.upload(&parent, path.name(), &src, mtime).await?;
        if remote.size != size {
            tracing::warn!(
                "{path}: uploaded {size} bytes, iCloud recorded {}",
                remote.size
            );
        }
        Ok(self
            .state
            .put(path, &SyncEntry::file(&remote, mtime))
            .await?)
    }

    async fn trash_local(&self, path: &RelPath, kind: ItemKind) -> Result<(), ExecError> {
        let from = self.abs(path);
        let to = self.trash_batch.join(path.as_str());
        if let Some(parent) = to.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::rename(&from, &to).await?;
        tracing::info!("moved {path} to {}", to.display());
        self.forget(path, kind).await
    }
}

async fn stamp(path: PathBuf) -> Result<(u64, i64), ExecError> {
    Ok(blocking(move || local::file_stamp(&path)).await?)
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| io::Error::other(format!("blocking task failed: {e}")))?
}
