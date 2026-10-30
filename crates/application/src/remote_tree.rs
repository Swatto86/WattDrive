//! Walk iCloud Drive with bounded, rolling batches across folder depths.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use futures_util::stream::{self, FuturesUnordered, StreamExt};
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
) -> Result<BTreeMap<RelPath, RelPath>, DriveError> {
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
    // Reserve source names even for packages, and old state paths so an archive
    // cannot take over another item's baseline. Windows aliases share a slot.
    let source_names: HashSet<_> = tree.nodes.keys().map(path_key).collect();
    let mut reserved = source_names.clone();
    reserved.extend(state.keys().map(path_key));
    let saved_paths: HashMap<_, _> = state
        .iter()
        .filter(|(_, entry)| entry.is_package)
        .map(|(path, entry)| (&entry.remote_id, path))
        .collect();
    let mut nodes = BTreeMap::new();
    let mut originals = BTreeMap::new();
    for (path, node) in std::mem::take(&mut tree.nodes) {
        let mut mapped = path.clone();
        if let RemoteNode::File(file) = &node {
            if file.is_package {
                mapped = match saved_paths.get(&file.id).filter(|saved| {
                    is_archive_name(&path, saved) && !source_names.contains(&path_key(saved))
                }) {
                    Some(saved) => (*saved).clone(),
                    None => {
                        let mut index = 0u32;
                        loop {
                            let name = if index == 0 {
                                format!("{path}.zip")
                            } else {
                                format!("{path} (iCloud package {index}).zip")
                            };
                            let candidate = RelPath::new(&name)
                                .map_err(|e| DriveError::Other(e.to_string()))?;
                            if !reserved.contains(&path_key(&candidate)) {
                                break candidate;
                            }
                            index = index.checked_add(1).ok_or_else(|| {
                                DriveError::Other("No available package archive name".into())
                            })?;
                        }
                    }
                };
                reserved.insert(path_key(&mapped));
                originals.insert(mapped.clone(), path);
            }
        }
        if nodes.insert(mapped.clone(), node).is_some() {
            return Err(DriveError::Other(format!(
                "Package archive name collides with another iCloud item: {mapped}"
            )));
        }
    }
    tree.nodes = nodes;
    Ok(originals)
}

fn path_key(path: &RelPath) -> String {
    if cfg!(windows) {
        path.as_str().to_lowercase()
    } else {
        path.to_string()
    }
}

fn is_archive_name(original: &RelPath, saved: &RelPath) -> bool {
    saved.as_str() == format!("{original}.zip")
        || saved
            .as_str()
            .strip_prefix(&format!("{original} (iCloud package "))
            .and_then(|rest| rest.strip_suffix(").zip"))
            .and_then(|index| index.parse::<u32>().ok())
            .is_some_and(|index| index > 0)
}

#[cfg(test)]
pub async fn walk(drive: &dyn RemoteDrive) -> Result<RemoteTree, DriveError> {
    walk_with_progress(drive, &|_, _| {}).await
}

pub async fn walk_with_progress(
    drive: &dyn RemoteDrive,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<RemoteTree, DriveError> {
    let started = std::time::Instant::now();
    let mut completed = 0;
    let mut batches = 0;
    let root_id = drive.root();
    let mut nodes = BTreeMap::new();
    let mut folder_ids = HashMap::new();
    let mut seen = HashSet::from([root_id.clone()]);
    // Folders still to list, by id; None = root.
    let mut frontier = VecDeque::from([(None, root_id.clone())]);
    let mut pending = FuturesUnordered::new();
    progress(0, 0);
    loop {
        // Smaller batches avoid Apple's long serial response times, while a
        // rolling queue lets fast branches descend without waiting for a level.
        while !frontier.is_empty() && pending.len() < 4 {
            let paths: HashMap<RemoteId, Option<RelPath>> = frontier
                .drain(..frontier.len().min(10))
                .map(|(path, id)| (id, path))
                .collect();
            pending.push(async move {
                let ids: Vec<_> = paths.keys().cloned().collect();
                let result = drive.list_children_many(&ids).await;
                (paths, ids, result)
            });
        }
        let Some((paths, ids, result)) = pending.next().await else {
            break;
        };
        let listings = result?;
        batches += 1;
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
                    frontier.push_back((Some(path.clone()), id.clone()));
                }
                nodes.insert(path, child.node);
            }
        }
        completed += ids.len();
        progress(completed, nodes.len());
    }
    tracing::info!(
        folders = completed,
        items = nodes.len(),
        batches,
        elapsed_ms = started.elapsed().as_millis(),
        "iCloud listing complete"
    );

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
