//! The contract a cloud-drive adapter fulfils for the sync engine.

use std::path::Path;

use async_trait::async_trait;

use crate::{DriveError, RemoteChild, RemoteFile, RemoteId};

/// Everything the engine needs from iCloud Drive (or a fake of it in tests).
#[async_trait]
pub trait RemoteDrive: Send + Sync {
    /// The drive's root folder id.
    fn root(&self) -> RemoteId;

    /// Direct children of a folder.
    async fn list_children(&self, folder: &RemoteId) -> Result<Vec<RemoteChild>, DriveError>;

    /// Fetch a file's content into `dest` (an already-chosen temp path).
    async fn download(&self, file: &RemoteFile, dest: &Path) -> Result<(), DriveError>;

    /// Upload `src` as `name` inside `parent`, stamping it with `mtime_ms`.
    /// Returns the created file as iCloud now reports it.
    async fn upload(
        &self,
        parent: &RemoteId,
        name: &str,
        src: &Path,
        mtime_ms: i64,
    ) -> Result<RemoteFile, DriveError>;

    /// Create a folder inside `parent`.
    async fn create_folder(&self, parent: &RemoteId, name: &str) -> Result<RemoteId, DriveError>;

    /// Move a file or folder to iCloud's Recently Deleted. `etag` guards
    /// against trashing a version we have not seen (files only; folders pass
    /// their current etag or an empty string).
    async fn trash(&self, id: &RemoteId, etag: &str) -> Result<(), DriveError>;
}
