//! An in-memory iCloud Drive for engine tests: real ids, etags that bump on
//! every write, and the same trash-then-replace semantics as the adapter.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use wattdrive_domain::{DriveError, RemoteChild, RemoteDrive, RemoteFile, RemoteId, RemoteNode};

#[derive(Clone, Debug)]
struct Node {
    parent: String,
    name: String,
    folder: bool,
    etag: u64,
    content: Vec<u8>,
    mtime_ms: i64,
}

#[derive(Default)]
struct Inner {
    nodes: HashMap<String, Node>,
    next_id: u64,
    next_etag: u64,
    trashed: Vec<String>,
}

#[derive(Default)]
pub struct FakeDrive {
    inner: Mutex<Inner>,
}

const ROOT: &str = "FOLDER::fake::root";

impl FakeDrive {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    fn find(inner: &Inner, path: &str) -> Option<String> {
        let mut parent = ROOT.to_string();
        let mut found = None;
        for part in path.split('/') {
            let (id, _) = inner
                .nodes
                .iter()
                .find(|(_, n)| n.parent == parent && n.name == part)?;
            found = Some(id.clone());
            parent = id.clone();
        }
        found
    }

    fn insert(
        inner: &mut Inner,
        parent: &str,
        name: &str,
        folder: bool,
        content: Vec<u8>,
        mtime_ms: i64,
    ) -> (String, u64) {
        inner.next_id += 1;
        inner.next_etag += 1;
        let id = format!(
            "{}::fake::{}",
            if folder { "FOLDER" } else { "FILE" },
            inner.next_id
        );
        inner.nodes.insert(
            id.clone(),
            Node {
                parent: parent.to_string(),
                name: name.to_string(),
                folder,
                etag: inner.next_etag,
                content,
                mtime_ms,
            },
        );
        (id, inner.next_etag)
    }

    /// Create every folder along `path` and the file at its end.
    pub fn add_file(&self, path: &str, content: &[u8], mtime_ms: i64) {
        let mut inner = self.lock();
        let mut parent = ROOT.to_string();
        let parts: Vec<&str> = path.split('/').collect();
        for dir in &parts[..parts.len() - 1] {
            parent = match inner
                .nodes
                .iter()
                .find(|(_, n)| n.parent == parent && n.name == *dir)
                .map(|(id, _)| id.clone())
            {
                Some(id) => id,
                None => Self::insert(&mut inner, &parent, dir, true, vec![], 0).0,
            };
        }
        Self::insert(
            &mut inner,
            &parent,
            parts[parts.len() - 1],
            false,
            content.to_vec(),
            mtime_ms,
        );
    }

    pub fn edit_file(&self, path: &str, content: &[u8], mtime_ms: i64) {
        let mut inner = self.lock();
        let id = Self::find(&inner, path).expect("file exists");
        inner.next_etag += 1;
        let etag = inner.next_etag;
        let node = inner.nodes.get_mut(&id).unwrap();
        node.content = content.to_vec();
        node.mtime_ms = mtime_ms;
        node.etag = etag;
    }

    pub fn remove(&self, path: &str) {
        let mut inner = self.lock();
        let id = Self::find(&inner, path).expect("item exists");
        Self::remove_subtree(&mut inner, &id);
    }

    fn remove_subtree(inner: &mut Inner, id: &str) {
        let children: Vec<String> = inner
            .nodes
            .iter()
            .filter(|(_, n)| n.parent == id)
            .map(|(cid, _)| cid.clone())
            .collect();
        for c in children {
            Self::remove_subtree(inner, &c);
        }
        inner.nodes.remove(id);
    }

    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        let inner = self.lock();
        let id = Self::find(&inner, path)?;
        Some(inner.nodes[&id].content.clone())
    }

    pub fn exists(&self, path: &str) -> bool {
        let inner = self.lock();
        Self::find(&inner, path).is_some()
    }

    pub fn trashed_count(&self) -> usize {
        self.lock().trashed.len()
    }

    pub fn paths(&self) -> Vec<String> {
        let inner = self.lock();
        let mut out = Vec::new();
        for (id, n) in &inner.nodes {
            let mut parts = vec![n.name.clone()];
            let mut p = n.parent.clone();
            while p != ROOT {
                let pn = &inner.nodes[&p];
                parts.push(pn.name.clone());
                p = pn.parent.clone();
            }
            parts.reverse();
            let _ = id;
            out.push(parts.join("/"));
        }
        out.sort();
        out
    }
}

#[async_trait]
impl RemoteDrive for FakeDrive {
    fn root(&self) -> RemoteId {
        RemoteId(ROOT.to_string())
    }

    async fn list_children(&self, folder: &RemoteId) -> Result<Vec<RemoteChild>, DriveError> {
        let inner = self.lock();
        if folder.as_str() != ROOT && !inner.nodes.contains_key(folder.as_str()) {
            return Err(DriveError::Api {
                status: 404,
                message: "no such folder".into(),
            });
        }
        Ok(inner
            .nodes
            .iter()
            .filter(|(_, n)| n.parent == folder.as_str())
            .map(|(id, n)| RemoteChild {
                name: n.name.clone(),
                node: if n.folder {
                    RemoteNode::Folder {
                        id: RemoteId(id.clone()),
                        etag: n.etag.to_string(),
                    }
                } else {
                    RemoteNode::File(RemoteFile {
                        id: RemoteId(id.clone()),
                        etag: n.etag.to_string(),
                        size: n.content.len() as u64,
                        modified_ms: n.mtime_ms,
                    })
                },
            })
            .collect())
    }

    async fn download(&self, file: &RemoteFile, dest: &Path) -> Result<(), DriveError> {
        let content = {
            let inner = self.lock();
            inner
                .nodes
                .get(file.id.as_str())
                .map(|n| n.content.clone())
                .ok_or(DriveError::Api {
                    status: 404,
                    message: "no such file".into(),
                })?
        };
        std::fs::write(dest, content)?;
        Ok(())
    }

    async fn upload(
        &self,
        parent: &RemoteId,
        name: &str,
        src: &Path,
        mtime_ms: i64,
    ) -> Result<RemoteFile, DriveError> {
        let content = std::fs::read(src)?;
        let mut inner = self.lock();
        let size = content.len() as u64;
        let (id, etag) = Self::insert(&mut inner, parent.as_str(), name, false, content, mtime_ms);
        Ok(RemoteFile {
            id: RemoteId(id),
            etag: etag.to_string(),
            size,
            modified_ms: mtime_ms,
        })
    }

    async fn create_folder(&self, parent: &RemoteId, name: &str) -> Result<RemoteId, DriveError> {
        let mut inner = self.lock();
        let (id, _) = Self::insert(&mut inner, parent.as_str(), name, true, vec![], 0);
        Ok(RemoteId(id))
    }

    async fn trash(&self, id: &RemoteId, etag: &str) -> Result<(), DriveError> {
        let mut inner = self.lock();
        let node = inner.nodes.get(id.as_str()).ok_or(DriveError::Api {
            status: 404,
            message: "no such item".into(),
        })?;
        if !node.folder && node.etag.to_string() != etag {
            return Err(DriveError::Conflict);
        }
        Self::remove_subtree(&mut inner, id.as_str());
        inner.trashed.push(id.as_str().to_string());
        Ok(())
    }
}
