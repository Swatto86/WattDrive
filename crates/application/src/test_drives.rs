//! Wrappers around [`FakeDrive`] that misbehave in one specific way, for the
//! engine tests that exercise failure paths.

use std::path::Path;
use std::sync::Arc;

use crate::fake_drive::FakeDrive;

/// A drive that fails to serve one file's content (iCloud reporting a file it
/// can no longer download).
pub struct Vanishing(pub Arc<FakeDrive>, pub wattdrive_domain::RemoteId);
#[async_trait::async_trait]
impl wattdrive_domain::RemoteDrive for Vanishing {
    fn root(&self) -> wattdrive_domain::RemoteId {
        self.0.root()
    }
    async fn list_children(
        &self,
        f: &wattdrive_domain::RemoteId,
    ) -> Result<Vec<wattdrive_domain::RemoteChild>, wattdrive_domain::DriveError> {
        self.0.list_children(f).await
    }
    async fn download(
        &self,
        file: &wattdrive_domain::RemoteFile,
        dest: &Path,
    ) -> Result<(), wattdrive_domain::DriveError> {
        if file.id == self.1 {
            return Err(wattdrive_domain::DriveError::Api {
                status: 500,
                message: "boom".into(),
            });
        }
        self.0.download(file, dest).await
    }
    async fn upload(
        &self,
        p: &wattdrive_domain::RemoteId,
        n: &str,
        s: &Path,
        m: i64,
    ) -> Result<wattdrive_domain::RemoteFile, wattdrive_domain::DriveError> {
        self.0.upload(p, n, s, m).await
    }
    async fn create_folder(
        &self,
        p: &wattdrive_domain::RemoteId,
        n: &str,
    ) -> Result<wattdrive_domain::RemoteId, wattdrive_domain::DriveError> {
        self.0.create_folder(p, n).await
    }
    async fn trash(
        &self,
        id: &wattdrive_domain::RemoteId,
        e: &str,
    ) -> Result<(), wattdrive_domain::DriveError> {
        self.0.trash(id, e).await
    }
}

/// A drive whose batched listing silently omits one folder, the way a partial
/// `retrieveItemDetailsInFolders` response would.
pub struct DropsFolder(pub Arc<FakeDrive>, pub wattdrive_domain::RemoteId);

#[async_trait::async_trait]
impl wattdrive_domain::RemoteDrive for DropsFolder {
    fn root(&self) -> wattdrive_domain::RemoteId {
        self.0.root()
    }
    async fn list_children(
        &self,
        f: &wattdrive_domain::RemoteId,
    ) -> Result<Vec<wattdrive_domain::RemoteChild>, wattdrive_domain::DriveError> {
        self.0.list_children(f).await
    }
    async fn list_children_many(
        &self,
        folders: &[wattdrive_domain::RemoteId],
    ) -> Result<
        Vec<(
            wattdrive_domain::RemoteId,
            Vec<wattdrive_domain::RemoteChild>,
        )>,
        wattdrive_domain::DriveError,
    > {
        let mut out = Vec::new();
        for id in folders.iter().filter(|id| **id != self.1) {
            out.push((id.clone(), self.0.list_children(id).await?));
        }
        Ok(out)
    }
    async fn download(
        &self,
        file: &wattdrive_domain::RemoteFile,
        dest: &Path,
    ) -> Result<(), wattdrive_domain::DriveError> {
        self.0.download(file, dest).await
    }
    async fn upload(
        &self,
        p: &wattdrive_domain::RemoteId,
        n: &str,
        s: &Path,
        m: i64,
    ) -> Result<wattdrive_domain::RemoteFile, wattdrive_domain::DriveError> {
        self.0.upload(p, n, s, m).await
    }
    async fn create_folder(
        &self,
        p: &wattdrive_domain::RemoteId,
        n: &str,
    ) -> Result<wattdrive_domain::RemoteId, wattdrive_domain::DriveError> {
        self.0.create_folder(p, n).await
    }
    async fn trash(
        &self,
        id: &wattdrive_domain::RemoteId,
        e: &str,
    ) -> Result<(), wattdrive_domain::DriveError> {
        self.0.trash(id, e).await
    }
}
