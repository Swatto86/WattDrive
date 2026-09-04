//! The two-way sync planner.
//!
//! Compares three snapshots keyed by [`RelPath`] and emits an ordered action
//! list. Rules, in plain words:
//!
//! * A side "changed" when it differs from what the last sync recorded. With
//!   no record, both sides are new.
//! * One side changed → copy it to the other. Both changed → keep both: the
//!   local file is moved aside as a conflict copy and the iCloud version takes
//!   the original name (the copy then uploads as a new file next pass).
//! * A deletion only propagates when the other side is unchanged. Edited
//!   files are never deleted because the other side removed them.
//! * A folder deleted on one side is deleted on the other only if nothing
//!   inside it needs to travel the other way; otherwise it is recreated.
//! * Deletions go to a trash (iCloud's Recently Deleted, or WattDrive's own
//!   trash folder locally) — never a hard delete.

use std::collections::{BTreeMap, BTreeSet};

use crate::{ItemKind, LocalNode, RelPath, RemoteFile, RemoteId, RemoteNode, SyncEntry};

/// Files whose size matches and whose mtimes agree within this window are
/// treated as the same content (iCloud reports ms; local filesystems and
/// upload round-trips can shift stamps by a second or so).
const SAME_MTIME_TOLERANCE_MS: i64 = 2_000;

pub struct PlanInput<'a> {
    pub remote: &'a BTreeMap<RelPath, RemoteNode>,
    pub local: &'a BTreeMap<RelPath, LocalNode>,
    pub state: &'a BTreeMap<RelPath, SyncEntry>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SyncAction {
    /// Rename the local item to a conflict-copy name before the remote version
    /// takes its place. Always precedes a `Download` / `CreateLocalFolder` of
    /// the same path.
    MoveLocalAside {
        path: RelPath,
    },
    CreateLocalFolder {
        path: RelPath,
        remote_id: RemoteId,
    },
    CreateRemoteFolder {
        path: RelPath,
    },
    /// Both sides have the folder; make sure the state knows its id.
    RecordFolder {
        path: RelPath,
        remote_id: RemoteId,
    },
    Download {
        path: RelPath,
        remote: RemoteFile,
    },
    /// Send the local file. `replaces` is the remote version being superseded
    /// (iCloud needs the old one trashed before the new upload).
    Upload {
        path: RelPath,
        replaces: Option<RemoteFile>,
    },
    /// Both sides hold the same content but no record exists: record it.
    Adopt {
        path: RelPath,
        remote: RemoteFile,
    },
    TrashLocal {
        path: RelPath,
        kind: ItemKind,
    },
    TrashRemote {
        path: RelPath,
        id: RemoteId,
        etag: String,
        kind: ItemKind,
    },
    /// Gone on both sides: drop the record.
    Forget {
        path: RelPath,
    },
}

impl SyncAction {
    pub fn path(&self) -> &RelPath {
        match self {
            SyncAction::MoveLocalAside { path }
            | SyncAction::CreateLocalFolder { path, .. }
            | SyncAction::CreateRemoteFolder { path }
            | SyncAction::RecordFolder { path, .. }
            | SyncAction::Download { path, .. }
            | SyncAction::Upload { path, .. }
            | SyncAction::Adopt { path, .. }
            | SyncAction::TrashLocal { path, .. }
            | SyncAction::TrashRemote { path, .. }
            | SyncAction::Forget { path } => path,
        }
    }

    /// Does this action bring content from iCloud to the local folder?
    fn pulls_remote(&self) -> bool {
        matches!(
            self,
            SyncAction::Download { .. }
                | SyncAction::CreateLocalFolder { .. }
                | SyncAction::Adopt { .. }
        )
    }

    /// Does this action send local content to iCloud?
    fn pushes_local(&self) -> bool {
        matches!(
            self,
            SyncAction::Upload { .. } | SyncAction::CreateRemoteFolder { .. }
        )
    }

    /// Execution phase; lower runs first. Within a phase, creates run shallow
    /// first and deletes deep first (see [`plan`]).
    fn phase(&self) -> u8 {
        match self {
            SyncAction::MoveLocalAside { .. } => 0,
            SyncAction::CreateLocalFolder { .. }
            | SyncAction::CreateRemoteFolder { .. }
            | SyncAction::RecordFolder { .. } => 1,
            SyncAction::Download { .. } | SyncAction::Upload { .. } | SyncAction::Adopt { .. } => 2,
            SyncAction::TrashLocal {
                kind: ItemKind::File,
                ..
            }
            | SyncAction::TrashRemote {
                kind: ItemKind::File,
                ..
            } => 3,
            SyncAction::TrashLocal { .. } | SyncAction::TrashRemote { .. } => 4,
            SyncAction::Forget { .. } => 5,
        }
    }
}

fn same_content(remote: &RemoteFile, size: u64, mtime_ms: i64) -> bool {
    remote.size == size && (remote.modified_ms - mtime_ms).abs() <= SAME_MTIME_TOLERANCE_MS
}

fn local_changed(entry: &SyncEntry, size: u64, mtime_ms: i64) -> bool {
    entry.size != size || entry.local_mtime_ms != mtime_ms
}

/// A folder present on one side only, with a sync record: its fate depends on
/// what its descendants need, so it is decided after the files.
enum Deferred {
    LocalGone {
        path: RelPath,
        id: RemoteId,
        etag: String,
    },
    RemoteGone {
        path: RelPath,
    },
}

/// Compute the ordered action list for one sync pass.
pub fn plan(input: PlanInput<'_>) -> Vec<SyncAction> {
    let mut actions = Vec::new();
    let mut deferred = Vec::new();

    let paths: BTreeSet<&RelPath> = input
        .remote
        .keys()
        .chain(input.local.keys())
        .chain(input.state.keys())
        .collect();

    for path in paths {
        let remote = input.remote.get(path);
        let local = input.local.get(path);
        let state = input.state.get(path);
        plan_path(path, remote, local, state, &mut actions, &mut deferred);
    }

    for d in deferred {
        match d {
            Deferred::LocalGone { path, id, etag } => {
                if actions
                    .iter()
                    .any(|a| a.path().is_inside(&path) && a.pulls_remote())
                {
                    actions.push(SyncAction::CreateLocalFolder {
                        path,
                        remote_id: id,
                    });
                } else {
                    actions.push(SyncAction::TrashRemote {
                        path,
                        id,
                        etag,
                        kind: ItemKind::Folder,
                    });
                }
            }
            Deferred::RemoteGone { path } => {
                if actions
                    .iter()
                    .any(|a| a.path().is_inside(&path) && a.pushes_local())
                {
                    actions.push(SyncAction::CreateRemoteFolder { path });
                } else {
                    actions.push(SyncAction::TrashLocal {
                        path,
                        kind: ItemKind::Folder,
                    });
                }
            }
        }
    }

    prune_covered_deletes(&mut actions);
    sort_for_execution(&mut actions);
    actions
}

fn plan_path(
    path: &RelPath,
    remote: Option<&RemoteNode>,
    local: Option<&LocalNode>,
    state: Option<&SyncEntry>,
    out: &mut Vec<SyncAction>,
    deferred: &mut Vec<Deferred>,
) {
    let p = || path.clone();
    match (remote, local) {
        // ---- folders on both sides ----
        (Some(RemoteNode::Folder { id, .. }), Some(LocalNode::Folder)) => {
            if state.is_none_or(|s| s.kind != ItemKind::Folder || &s.remote_id != id) {
                out.push(SyncAction::RecordFolder {
                    path: p(),
                    remote_id: id.clone(),
                });
            }
        }
        // ---- kind mismatch: the local item steps aside, iCloud wins the name ----
        (Some(RemoteNode::Folder { id, .. }), Some(LocalNode::File { .. })) => {
            out.push(SyncAction::MoveLocalAside { path: p() });
            out.push(SyncAction::CreateLocalFolder {
                path: p(),
                remote_id: id.clone(),
            });
        }
        (Some(RemoteNode::File(rf)), Some(LocalNode::Folder)) => {
            out.push(SyncAction::MoveLocalAside { path: p() });
            out.push(SyncAction::Download {
                path: p(),
                remote: rf.clone(),
            });
        }
        // ---- files on both sides ----
        (Some(RemoteNode::File(rf)), Some(LocalNode::File { size, mtime_ms })) => {
            let (remote_changed, local_changed) = match state {
                Some(s) if s.kind == ItemKind::File => {
                    (s.remote_etag != rf.etag, local_changed(s, *size, *mtime_ms))
                }
                // No record (or a folder record for what is now a file): both new.
                _ => (true, true),
            };
            let identical = same_content(rf, *size, *mtime_ms);
            match (remote_changed, local_changed) {
                (false, false) => {}
                // Something moved, but the content is the same on both sides
                // (our own upload's etag settling, a local `touch`): re-record.
                _ if identical => out.push(SyncAction::Adopt {
                    path: p(),
                    remote: rf.clone(),
                }),
                (true, false) => out.push(SyncAction::Download {
                    path: p(),
                    remote: rf.clone(),
                }),
                (false, true) => out.push(SyncAction::Upload {
                    path: p(),
                    replaces: Some(rf.clone()),
                }),
                (true, true) => {
                    out.push(SyncAction::MoveLocalAside { path: p() });
                    out.push(SyncAction::Download {
                        path: p(),
                        remote: rf.clone(),
                    });
                }
            }
        }
        // ---- remote only ----
        (Some(RemoteNode::File(rf)), None) => match state {
            Some(s) if s.kind == ItemKind::File && s.remote_etag == rf.etag => {
                out.push(SyncAction::TrashRemote {
                    path: p(),
                    id: rf.id.clone(),
                    etag: rf.etag.clone(),
                    kind: ItemKind::File,
                })
            }
            _ => out.push(SyncAction::Download {
                path: p(),
                remote: rf.clone(),
            }),
        },
        (Some(RemoteNode::Folder { id, etag }), None) => match state {
            Some(s) if s.kind == ItemKind::Folder => deferred.push(Deferred::LocalGone {
                path: p(),
                id: id.clone(),
                etag: etag.clone(),
            }),
            _ => out.push(SyncAction::CreateLocalFolder {
                path: p(),
                remote_id: id.clone(),
            }),
        },
        // ---- local only ----
        (None, Some(LocalNode::File { size, mtime_ms })) => match state {
            Some(s) if s.kind == ItemKind::File && !local_changed(s, *size, *mtime_ms) => {
                out.push(SyncAction::TrashLocal {
                    path: p(),
                    kind: ItemKind::File,
                })
            }
            _ => out.push(SyncAction::Upload {
                path: p(),
                replaces: None,
            }),
        },
        (None, Some(LocalNode::Folder)) => match state {
            Some(s) if s.kind == ItemKind::Folder => {
                deferred.push(Deferred::RemoteGone { path: p() })
            }
            _ => out.push(SyncAction::CreateRemoteFolder { path: p() }),
        },
        // ---- gone everywhere ----
        (None, None) => {
            if state.is_some() {
                out.push(SyncAction::Forget { path: p() });
            }
        }
    }
}

/// Trashing a folder takes its contents with it, so drop the deletes (and
/// forgets) of everything inside a folder that is itself being trashed on the
/// same side. The executor clears the subtree's records when it trashes the
/// folder.
fn prune_covered_deletes(actions: &mut Vec<SyncAction>) {
    let trashed_local: Vec<RelPath> = actions
        .iter()
        .filter_map(|a| match a {
            SyncAction::TrashLocal {
                path,
                kind: ItemKind::Folder,
            } => Some(path.clone()),
            _ => None,
        })
        .collect();
    let trashed_remote: Vec<RelPath> = actions
        .iter()
        .filter_map(|a| match a {
            SyncAction::TrashRemote {
                path,
                kind: ItemKind::Folder,
                ..
            } => Some(path.clone()),
            _ => None,
        })
        .collect();
    actions.retain(|a| match a {
        SyncAction::TrashLocal { path, .. } => !trashed_local.iter().any(|f| path.is_inside(f)),
        SyncAction::TrashRemote { path, .. } => !trashed_remote.iter().any(|f| path.is_inside(f)),
        SyncAction::Forget { path } => !trashed_local
            .iter()
            .chain(trashed_remote.iter())
            .any(|f| path.is_inside(f)),
        _ => true,
    });
}

/// Phase order, then shallow-first for creates/transfers and deep-first for
/// folder deletes, then path for determinism.
fn sort_for_execution(actions: &mut [SyncAction]) {
    actions.sort_by(|a, b| {
        let (pa, pb) = (a.phase(), b.phase());
        pa.cmp(&pb).then_with(|| {
            let (da, db) = (a.path().depth(), b.path().depth());
            let depth_order = if pa == 4 { db.cmp(&da) } else { da.cmp(&db) };
            depth_order.then_with(|| a.path().cmp(b.path()))
        })
    });
}
