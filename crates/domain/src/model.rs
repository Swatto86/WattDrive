//! Snapshot types the planner compares.

use serde::{Deserialize, Serialize};

/// Opaque iCloud identifier for a file or folder (`drivewsid`, e.g.
/// `FOLDER::com.apple.CloudDocs::root` or `FILE::com.apple.CloudDocs::<uuid>`).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct RemoteId(pub String);

impl RemoteId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ItemKind {
    File,
    Folder,
}

/// A file as iCloud reports it in a folder listing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RemoteFile {
    pub id: RemoteId,
    /// Version stamp; changes whenever the content or metadata changes.
    pub etag: String,
    pub size: u64,
    /// Modification time, Unix milliseconds.
    pub modified_ms: i64,
}

/// One entry in the remote tree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RemoteNode {
    Folder { id: RemoteId, etag: String },
    File(RemoteFile),
}

impl RemoteNode {
    pub fn id(&self) -> &RemoteId {
        match self {
            RemoteNode::Folder { id, .. } => id,
            RemoteNode::File(f) => &f.id,
        }
    }

    pub fn kind(&self) -> ItemKind {
        match self {
            RemoteNode::Folder { .. } => ItemKind::Folder,
            RemoteNode::File(_) => ItemKind::File,
        }
    }
}

/// A named child returned by listing one remote folder.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RemoteChild {
    pub name: String,
    pub node: RemoteNode,
}

/// One entry in the local tree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LocalNode {
    Folder,
    File {
        size: u64,
        /// Modification time, Unix milliseconds.
        mtime_ms: i64,
    },
}

impl LocalNode {
    pub fn kind(&self) -> ItemKind {
        match self {
            LocalNode::Folder => ItemKind::Folder,
            LocalNode::File { .. } => ItemKind::File,
        }
    }
}

/// What the last successful sync recorded for one path: the remote version we
/// mirrored and the local file stamp we left behind. Change detection on each
/// side is a comparison against this record.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SyncEntry {
    pub kind: ItemKind,
    pub remote_id: RemoteId,
    /// Empty for folders (iCloud folder etags churn with their contents).
    pub remote_etag: String,
    pub size: u64,
    pub local_mtime_ms: i64,
}

impl SyncEntry {
    pub fn folder(remote_id: RemoteId) -> Self {
        Self {
            kind: ItemKind::Folder,
            remote_id,
            remote_etag: String::new(),
            size: 0,
            local_mtime_ms: 0,
        }
    }

    pub fn file(remote: &RemoteFile, local_mtime_ms: i64) -> Self {
        Self {
            kind: ItemKind::File,
            remote_id: remote.id.clone(),
            remote_etag: remote.etag.clone(),
            size: remote.size,
            local_mtime_ms,
        }
    }
}
