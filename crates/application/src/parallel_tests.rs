//! Real passes with delayed transfers: verify overlap, draining and restart.
use crate::{fake_drive::FakeDrive, MemoryStateStore, Progress, StateStore, SyncEngine};
use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use wattdrive_domain::{DriveError, RemoteChild, RemoteDrive, RemoteFile, RemoteId};

struct SlowDrive {
    inner: FakeDrive,
    active: AtomicUsize,
    peak: AtomicUsize,
    started: AtomicUsize,
    failure: AtomicUsize,
}

impl SlowDrive {
    async fn transfer(&self) -> Result<(), DriveError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        let failure = self.failure.swap(0, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(if failure > 0 { 20 } else { 80 })).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        match failure {
            1 => Err(DriveError::Network("connection lost".into())),
            2 => Err(DriveError::RateLimited),
            3 => Err(DriveError::SignInRequired("expired".into())),
            4 => Err(DriveError::Api {
                status: 404,
                message: "missing".into(),
            }),
            _ => Ok(()),
        }
    }
}

#[async_trait::async_trait]
impl RemoteDrive for SlowDrive {
    fn root(&self) -> RemoteId {
        self.inner.root()
    }
    async fn list_children(&self, id: &RemoteId) -> Result<Vec<RemoteChild>, DriveError> {
        self.inner.list_children(id).await
    }
    async fn download(&self, file: &RemoteFile, dest: &Path) -> Result<(), DriveError> {
        self.transfer().await?;
        self.inner.download(file, dest).await
    }
    async fn upload(
        &self,
        parent: &RemoteId,
        name: &str,
        src: &Path,
        mtime: i64,
    ) -> Result<RemoteFile, DriveError> {
        self.transfer().await?;
        self.inner.upload(parent, name, src, mtime).await
    }
    async fn create_folder(&self, parent: &RemoteId, name: &str) -> Result<RemoteId, DriveError> {
        self.inner.create_folder(parent, name).await
    }
    async fn trash(&self, id: &RemoteId, etag: &str) -> Result<(), DriveError> {
        self.inner.trash(id, etag).await
    }
}

#[tokio::test]
async fn transfers_overlap_with_a_limit_and_settle() {
    exercise(0).await;
}

#[tokio::test]
async fn failures_drain_active_work_and_retry_only_unfinished_files() {
    for failure in 1..=4 {
        exercise(failure).await;
    }
}

async fn exercise(failure: usize) {
    let dir = tempfile::tempdir().unwrap();
    let drive = Arc::new(SlowDrive {
        inner: FakeDrive::default(),
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        started: AtomicUsize::new(0),
        failure: AtomicUsize::new(failure),
    });
    for i in 0..6 {
        drive
            .inner
            .add_file(&format!("remote/{i}.txt"), b"remote", 1_700_000_000_000);
        std::fs::create_dir_all(dir.path().join("local")).unwrap();
        std::fs::write(dir.path().join(format!("local/{i}.txt")), b"local").unwrap();
    }
    let state = Arc::new(MemoryStateStore::default());
    let engine = SyncEngine::new(
        dir.path().into(),
        drive.clone(),
        state.clone(),
        "test".into(),
    );
    let progress = Mutex::new(Vec::new());
    let result = engine
        .run_once(&|p| {
            if let Progress::Executing { done, total, .. } = p {
                progress.lock().unwrap().push((done, total));
            }
        })
        .await;
    assert_eq!(
        drive.peak.load(Ordering::SeqCst),
        4,
        "four transfers overlap"
    );
    assert_eq!(
        drive.active.load(Ordering::SeqCst),
        0,
        "no abandoned in-flight transfers"
    );
    if (1..=3).contains(&failure) {
        assert_eq!(
            drive.started.load(Ordering::SeqCst),
            4,
            "stop scheduling after fatal error"
        );
        assert_eq!(
            state.load_all().await.unwrap().len(),
            5,
            "two folders and three completed transfers recorded"
        );
        let events = progress.lock().unwrap();
        assert!(
            events.last().unwrap().0 < events.last().unwrap().1,
            "aborted pass must not claim 100%"
        );
    }
    if failure == 3 {
        assert!(matches!(result, Err(DriveError::SignInRequired(_))));
    } else {
        let report = result.unwrap();
        assert_eq!(report.failures.len(), usize::from(failure != 0));
        assert_eq!(report.aborted.is_some(), failure == 1 || failure == 2);
    }
    let retry = engine.run_once(&|_| {}).await.unwrap();
    assert!(retry.failures.is_empty());
    assert_eq!(
        retry.uploaded + retry.downloaded,
        match failure {
            0 => 0,
            4 => 1,
            _ => 9,
        }
    );
    for i in 0..6 {
        assert_eq!(
            std::fs::read(dir.path().join(format!("remote/{i}.txt"))).unwrap(),
            b"remote"
        );
        assert!(drive.inner.exists(&format!("local/{i}.txt")));
    }
    assert_eq!(engine.run_once(&|_| {}).await.unwrap().planned, 0);
}
