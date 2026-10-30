use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};
use wattdrive_domain::{DriveError, RemoteChild, RemoteDrive, RemoteFile, RemoteId, RemoteNode};

#[derive(Default)]
struct BranchingDrive {
    active: AtomicUsize,
    peak: AtomicUsize,
    max_batch: AtomicUsize,
    calls: Mutex<Vec<String>>,
    descendant: tokio::sync::Notify,
}

fn folder(name: &str) -> RemoteChild {
    RemoteChild {
        name: name.into(),
        node: RemoteNode::Folder {
            id: RemoteId(name.into()),
            etag: "e".into(),
        },
    }
}

#[async_trait::async_trait]
impl RemoteDrive for BranchingDrive {
    fn root(&self) -> RemoteId {
        RemoteId("root".into())
    }
    async fn list_children(&self, _: &RemoteId) -> Result<Vec<RemoteChild>, DriveError> {
        unreachable!()
    }
    async fn list_children_many(
        &self,
        ids: &[RemoteId],
    ) -> Result<Vec<(RemoteId, Vec<RemoteChild>)>, DriveError> {
        self.max_batch.fetch_max(ids.len(), Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(10)).await;
        if ids.iter().any(|id| id.as_str() == "p0") {
            // A fast sibling's descendant must run while this batch is waiting.
            self.descendant.notified().await;
        }
        let mut out = Vec::new();
        for id in ids {
            self.calls.lock().unwrap().push(id.as_str().into());
            let children = match id.as_str() {
                "root" => (0..41).map(|i| folder(&format!("p{i}"))).collect(),
                "p10" => vec![folder("child")],
                "child" => {
                    self.descendant.notify_one();
                    vec![]
                }
                _ => vec![],
            };
            out.push((id.clone(), children));
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(out)
    }
    async fn download(&self, _: &RemoteFile, _: &Path) -> Result<(), DriveError> {
        unreachable!()
    }
    async fn upload(
        &self,
        _: &RemoteId,
        _: &str,
        _: &Path,
        _: i64,
    ) -> Result<RemoteFile, DriveError> {
        unreachable!()
    }
    async fn create_folder(&self, _: &RemoteId, _: &str) -> Result<RemoteId, DriveError> {
        unreachable!()
    }
    async fn trash(&self, _: &RemoteId, _: &str) -> Result<(), DriveError> {
        unreachable!()
    }
}

#[tokio::test]
async fn listing_keeps_workers_busy_across_depths_with_small_batches() {
    let drive = BranchingDrive::default();
    let tree = tokio::time::timeout(Duration::from_secs(1), crate::remote_tree::walk(&drive))
        .await
        .expect("a slow sibling must not block deeper folders")
        .unwrap();
    assert_eq!(tree.nodes.len(), 42);
    assert_eq!(drive.peak.load(Ordering::SeqCst), 4);
    assert_eq!(drive.max_batch.load(Ordering::SeqCst), 10);
    let calls = drive.calls.lock().unwrap();
    assert_eq!(calls.len(), 43);
    assert_eq!(
        calls.iter().collect::<std::collections::HashSet<_>>().len(),
        43
    );
}
