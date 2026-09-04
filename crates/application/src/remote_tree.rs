//! Walk iCloud Drive into a path-keyed tree, listing sibling folders
//! concurrently level by level.

use std::collections::{BTreeMap, HashMap};

use futures_util::stream::{self, StreamExt};
use wattdrive_domain::{DriveError, RelPath, RemoteDrive, RemoteId, RemoteNode};

/// How many folder listings are in flight at once.
const LIST_CONCURRENCY: usize = 6;

pub struct RemoteTree {
    pub nodes: BTreeMap<RelPath, RemoteNode>,
    /// Folder path → id, including entries the executor adds as it creates
    /// folders. The root is `None` in path terms and lives in `root_id`.
    pub folder_ids: HashMap<RelPath, RemoteId>,
    pub root_id: RemoteId,
}

pub async fn walk(drive: &dyn RemoteDrive) -> Result<RemoteTree, DriveError> {
    let root_id = drive.root();
    let mut nodes = BTreeMap::new();
    let mut folder_ids = HashMap::new();
    // (path of the folder, id) pairs still to list; None = root.
    let mut frontier: Vec<(Option<RelPath>, RemoteId)> = vec![(None, root_id.clone())];

    while !frontier.is_empty() {
        let listings: Vec<_> = stream::iter(frontier.drain(..))
            .map(|(path, id)| async move {
                let children = drive.list_children(&id).await;
                (path, children)
            })
            .buffer_unordered(LIST_CONCURRENCY)
            .collect()
            .await;

        for (parent, children) in listings {
            let children = children?;
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
