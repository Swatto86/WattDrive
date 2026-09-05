//! Walk iCloud Drive into a path-keyed tree, one batched listing per level.

use std::collections::{BTreeMap, HashMap};

use wattdrive_domain::{DriveError, RelPath, RemoteDrive, RemoteId, RemoteNode};

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
    // Folders still to list, by id; None = root.
    let mut frontier: Vec<(Option<RelPath>, RemoteId)> = vec![(None, root_id.clone())];

    while !frontier.is_empty() {
        let ids: Vec<RemoteId> = frontier.iter().map(|(_, id)| id.clone()).collect();
        let paths: HashMap<RemoteId, Option<RelPath>> =
            frontier.drain(..).map(|(p, id)| (id, p)).collect();
        let listings = drive.list_children_many(&ids).await?;
        if listings.len() != ids.len() {
            tracing::warn!(
                "iCloud returned {} folder listings for {} requested",
                listings.len(),
                ids.len()
            );
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
