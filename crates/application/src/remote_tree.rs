//! Walk iCloud Drive into a path-keyed tree, one batched listing per level.

use std::collections::{BTreeMap, HashMap, HashSet};

use futures_util::stream::{self, StreamExt};
use wattdrive_domain::{DriveError, RelPath, RemoteDrive, RemoteId, RemoteNode, SyncEntry};

pub struct RemoteTree {
    pub nodes: BTreeMap<RelPath, RemoteNode>,
    /// Folder path → id, including entries the executor adds as it creates
    /// folders. The root is `None` in path terms and lives in `root_id`.
    pub folder_ids: HashMap<RelPath, RemoteId>,
    pub root_id: RemoteId,
}

/// Apple omits package information from listings. Resolve new versions before
/// planning, retaining the representation of already-synced versions.
pub async fn prepare_files(
    drive: &dyn RemoteDrive,
    tree: &mut RemoteTree,
    state: &BTreeMap<RelPath, SyncEntry>,
    // true means replacing remote content: recheck even an adopted baseline.
    transfers: &BTreeMap<RelPath, bool>,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<(), DriveError> {
    let known: HashMap<_, _> = state.values().map(|s| (&s.remote_id, s)).collect();
    for node in tree.nodes.values_mut() {
        if let RemoteNode::File(file) = node {
            if let Some(saved) = known.get(&file.id).filter(|s| s.remote_etag == file.etag) {
                file.is_package = saved.is_package;
            }
        }
    }
    let pending: Vec<_> = tree
        .nodes
        .iter()
        .filter_map(|(path, node)| {
            let RemoteNode::File(file) = node else {
                return None;
            };
            (transfers.get(path).is_some_and(|replacing| {
                *replacing
                    || known
                        .get(&file.id)
                        .is_none_or(|s| s.remote_etag != file.etag)
            }))
            .then(|| (path.clone(), file.clone()))
        })
        .collect();
    let total = pending.len();
    let mut requests = stream::iter(pending.into_iter().map(|(path, file)| async move {
        drive.prepare_file(&file).await.map(|file| (path, file))
    }))
    .buffer_unordered(4);
    let mut done = 0;
    progress(done, total);
    while let Some(result) = requests.next().await {
        let (path, file) = result?;
        tree.nodes.insert(path, RemoteNode::File(file));
        done += 1;
        progress(done, total);
    }
    let mut nodes = BTreeMap::new();
    for (path, node) in std::mem::take(&mut tree.nodes) {
        let mut mapped = path;
        if let RemoteNode::File(file) = &node {
            if file.is_package {
                mapped = RelPath::new(&format!("{mapped}.zip"))
                    .map_err(|e| DriveError::Other(e.to_string()))?;
            }
        }
        if nodes.insert(mapped.clone(), node).is_some() {
            return Err(DriveError::Other(format!(
                "Package archive name collides with another iCloud item: {mapped}"
            )));
        }
    }
    tree.nodes = nodes;
    Ok(())
}

pub async fn walk(drive: &dyn RemoteDrive) -> Result<RemoteTree, DriveError> {
    let root_id = drive.root();
    let mut nodes = BTreeMap::new();
    let mut folder_ids = HashMap::new();
    let mut seen = HashSet::from([root_id.clone()]);
    // Folders still to list, by id; None = root.
    let mut frontier: Vec<(Option<RelPath>, RemoteId)> = vec![(None, root_id.clone())];

    while !frontier.is_empty() {
        let ids: Vec<RemoteId> = frontier.iter().map(|(_, id)| id.clone()).collect();
        let paths: HashMap<RemoteId, Option<RelPath>> =
            frontier.drain(..).map(|(p, id)| (id, p)).collect();
        let listings = drive.list_children_many(&ids).await?;
        // A folder we asked about but got no listing for must stop the pass:
        // treating its children as absent would read as "deleted on iCloud"
        // and trash their local copies.
        let listed: HashSet<&RemoteId> = listings.iter().map(|(id, _)| id).collect();
        if let Some(missing) = ids.iter().find(|id| !listed.contains(id)) {
            let path = paths
                .get(missing)
                .and_then(|p| p.as_ref().map(ToString::to_string))
                .unwrap_or_else(|| "the root folder".to_string());
            return Err(DriveError::Api {
                status: 200,
                message: format!(
                    "iCloud returned {} of {} folder listings; no listing for {path}",
                    listings.len(),
                    ids.len()
                ),
            });
        }

        for (folder_id, children) in listings {
            let Some(parent) = paths.get(&folder_id) else {
                tracing::warn!("listing for unrequested folder {}", folder_id.as_str());
                continue;
            };
            for child in children {
                let path = match RelPath::child(parent.as_ref(), &child.name) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("skipping remote item {:?}: {e}", child.name);
                        continue;
                    }
                };
                if nodes.contains_key(&path) {
                    tracing::warn!("duplicate remote name {path}, keeping the first");
                    continue;
                }
                if let RemoteNode::Folder { id, .. } = &child.node {
                    if !seen.insert(id.clone()) {
                        tracing::warn!("skipping cyclic remote folder {path}");
                        continue;
                    }
                    folder_ids.insert(path.clone(), id.clone());
                    frontier.push((Some(path.clone()), id.clone()));
                }
                nodes.insert(path, child.node);
            }
        }
    }

    Ok(RemoteTree {
        nodes,
        folder_ids,
        root_id,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use wattdrive_domain::{
        DriveError, RemoteChild, RemoteDrive, RemoteFile, RemoteId, RemoteNode,
    };

    use super::*;

    /// Root lists a folder whose only child is the root again.
    struct CycleDrive {
        listed: Mutex<u32>,
    }

    #[async_trait]
    impl RemoteDrive for CycleDrive {
        fn root(&self) -> RemoteId {
            RemoteId("root".into())
        }
        async fn list_children(&self, folder: &RemoteId) -> Result<Vec<RemoteChild>, DriveError> {
            *self.listed.lock().expect("lock") += 1;
            if folder.as_str() == "root" {
                Ok(vec![RemoteChild {
                    name: "loop".into(),
                    node: RemoteNode::Folder {
                        id: RemoteId("root".into()),
                        etag: "e".into(),
                    },
                }])
            } else {
                Ok(vec![])
            }
        }
        async fn download(&self, _: &RemoteFile, _: &Path) -> Result<(), DriveError> {
            Ok(())
        }
        async fn upload(
            &self,
            _: &RemoteId,
            _: &str,
            _: &Path,
            _: i64,
        ) -> Result<RemoteFile, DriveError> {
            Err(DriveError::Other("unused".into()))
        }
        async fn create_folder(&self, _: &RemoteId, _: &str) -> Result<RemoteId, DriveError> {
            Err(DriveError::Other("unused".into()))
        }
        async fn trash(&self, _: &RemoteId, _: &str) -> Result<(), DriveError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_cyclic_folder_listing_does_not_walk_forever() {
        let drive = CycleDrive {
            listed: Mutex::new(0),
        };
        let tree = walk(&drive).await.expect("walk finishes");
        assert!(tree.nodes.is_empty(), "the cycle is not a second folder");
        assert_eq!(*drive.listed.lock().expect("lock"), 1);
    }
}
