//! Case tables for the planner. Each test states a situation in terms of the
//! three snapshots and asserts the exact action list, including order.

use std::collections::BTreeMap;

use crate::plan::{plan, PlanInput, SyncAction};
use crate::{ItemKind, LocalNode, RelPath, RemoteFile, RemoteId, RemoteNode, SyncEntry};

fn p(s: &str) -> RelPath {
    RelPath::new(s).unwrap()
}
fn rid(s: &str) -> RemoteId {
    RemoteId(format!("FILE::z::{s}"))
}
fn rfile(id: &str, etag: &str, size: u64, mtime: i64) -> RemoteFile {
    RemoteFile {
        id: rid(id),
        etag: etag.into(),
        size,
        modified_ms: mtime,
    }
}
fn rf(id: &str, etag: &str, size: u64, mtime: i64) -> RemoteNode {
    RemoteNode::File(rfile(id, etag, size, mtime))
}
fn rfolder(id: &str) -> RemoteNode {
    RemoteNode::Folder {
        id: RemoteId(format!("FOLDER::z::{id}")),
        etag: format!("{id}-etag"),
    }
}
fn lf(size: u64, mtime: i64) -> LocalNode {
    LocalNode::File {
        size,
        mtime_ms: mtime,
    }
}
fn sfile(id: &str, etag: &str, size: u64, mtime: i64) -> SyncEntry {
    SyncEntry {
        kind: ItemKind::File,
        remote_id: rid(id),
        remote_etag: etag.into(),
        size,
        local_mtime_ms: mtime,
    }
}
fn sfolder(id: &str) -> SyncEntry {
    SyncEntry::folder(RemoteId(format!("FOLDER::z::{id}")))
}

struct World {
    remote: BTreeMap<RelPath, RemoteNode>,
    local: BTreeMap<RelPath, LocalNode>,
    state: BTreeMap<RelPath, SyncEntry>,
}

impl World {
    fn new() -> Self {
        Self {
            remote: BTreeMap::new(),
            local: BTreeMap::new(),
            state: BTreeMap::new(),
        }
    }
    fn remote(mut self, path: &str, node: RemoteNode) -> Self {
        self.remote.insert(p(path), node);
        self
    }
    fn local(mut self, path: &str, node: LocalNode) -> Self {
        self.local.insert(p(path), node);
        self
    }
    fn state(mut self, path: &str, entry: SyncEntry) -> Self {
        self.state.insert(p(path), entry);
        self
    }
    fn plan(&self) -> Vec<SyncAction> {
        plan(PlanInput {
            remote: &self.remote,
            local: &self.local,
            state: &self.state,
        })
    }
}

#[test]
fn nothing_to_do_when_everything_is_in_sync_or_empty() {
    assert!(World::new().plan().is_empty());
    let w = World::new()
        .remote("a.txt", rf("1", "e1", 10, 1000))
        .local("a.txt", lf(10, 1000))
        .state("a.txt", sfile("1", "e1", 10, 1000))
        .remote("dir", rfolder("d"))
        .local("dir", LocalNode::Folder)
        .state("dir", sfolder("d"));
    assert!(w.plan().is_empty());
}

#[test]
fn new_remote_content_downloads_folders_first() {
    let w = World::new()
        .remote("dir", rfolder("d"))
        .remote("dir/a.txt", rf("1", "e1", 10, 1000))
        .remote("top.txt", rf("2", "e2", 5, 500));
    assert_eq!(
        w.plan(),
        vec![
            SyncAction::CreateLocalFolder {
                path: p("dir"),
                remote_id: RemoteId("FOLDER::z::d".into())
            },
            // transfers run shallow-first, then by path
            SyncAction::Download {
                path: p("top.txt"),
                remote: rfile("2", "e2", 5, 500)
            },
            SyncAction::Download {
                path: p("dir/a.txt"),
                remote: rfile("1", "e1", 10, 1000)
            },
        ]
    );
}

#[test]
fn new_local_content_uploads_folders_first() {
    let w = World::new()
        .local("dir", LocalNode::Folder)
        .local("dir/a.txt", lf(10, 1000));
    assert_eq!(
        w.plan(),
        vec![
            SyncAction::CreateRemoteFolder { path: p("dir") },
            SyncAction::Upload {
                path: p("dir/a.txt"),
                replaces: None
            },
        ]
    );
}

#[test]
fn one_side_changed_copies_to_the_other() {
    let base = || {
        World::new()
            .local("a.txt", lf(10, 1000))
            .state("a.txt", sfile("1", "e1", 10, 1000))
    };
    // remote edited
    let w = base().remote("a.txt", rf("1", "e2", 12, 2000));
    assert_eq!(
        w.plan(),
        vec![SyncAction::Download {
            path: p("a.txt"),
            remote: rfile("1", "e2", 12, 2000)
        }]
    );
    // local edited
    let w = World::new()
        .remote("a.txt", rf("1", "e1", 10, 1000))
        .local("a.txt", lf(11, 5000))
        .state("a.txt", sfile("1", "e1", 10, 1000));
    assert_eq!(
        w.plan(),
        vec![SyncAction::Upload {
            path: p("a.txt"),
            replaces: Some(rfile("1", "e1", 10, 1000))
        }]
    );
}

#[test]
fn both_changed_keeps_both_with_icloud_taking_the_name() {
    let w = World::new()
        .remote("a.txt", rf("1", "e2", 12, 2000))
        .local("a.txt", lf(11, 5000))
        .state("a.txt", sfile("1", "e1", 10, 1000));
    assert_eq!(
        w.plan(),
        vec![
            SyncAction::MoveLocalAside { path: p("a.txt") },
            SyncAction::Download {
                path: p("a.txt"),
                remote: rfile("1", "e2", 12, 2000)
            },
        ]
    );
}

#[test]
fn identical_content_is_adopted_not_transferred() {
    // both new, same size and mtime within tolerance
    let w = World::new()
        .remote("a.txt", rf("1", "e1", 10, 1000))
        .local("a.txt", lf(10, 2500));
    assert_eq!(
        w.plan(),
        vec![SyncAction::Adopt {
            path: p("a.txt"),
            remote: rfile("1", "e1", 10, 1000)
        }]
    );
    // both new, different → conflict copy
    let w = World::new()
        .remote("a.txt", rf("1", "e1", 10, 1000))
        .local("a.txt", lf(10, 9000));
    assert!(matches!(w.plan()[0], SyncAction::MoveLocalAside { .. }));
    // etag drifted after our own upload, content unchanged → adopt
    let w = World::new()
        .remote("a.txt", rf("1", "e-settled", 10, 1000))
        .local("a.txt", lf(10, 1000))
        .state("a.txt", sfile("1", "e-upload", 10, 1000));
    assert!(matches!(w.plan()[0], SyncAction::Adopt { .. }));
    // local touched but identical → adopt, not upload
    let w = World::new()
        .remote("a.txt", rf("1", "e1", 10, 1000))
        .local("a.txt", lf(10, 1500))
        .state("a.txt", sfile("1", "e1", 10, 1000));
    assert!(matches!(w.plan()[0], SyncAction::Adopt { .. }));
}

#[test]
fn deletions_propagate_only_against_an_unchanged_other_side() {
    // remote deleted, local unchanged → trash local
    let w = World::new()
        .local("a.txt", lf(10, 1000))
        .state("a.txt", sfile("1", "e1", 10, 1000));
    assert_eq!(
        w.plan(),
        vec![SyncAction::TrashLocal {
            path: p("a.txt"),
            kind: ItemKind::File
        }]
    );
    // remote deleted, local edited → re-upload, never delete edits
    let w = World::new()
        .local("a.txt", lf(12, 3000))
        .state("a.txt", sfile("1", "e1", 10, 1000));
    assert_eq!(
        w.plan(),
        vec![SyncAction::Upload {
            path: p("a.txt"),
            replaces: None
        }]
    );
    // local deleted, remote unchanged → trash remote with the known etag
    let w = World::new()
        .remote("a.txt", rf("1", "e1", 10, 1000))
        .state("a.txt", sfile("1", "e1", 10, 1000));
    assert_eq!(
        w.plan(),
        vec![SyncAction::TrashRemote {
            path: p("a.txt"),
            id: rid("1"),
            etag: "e1".into(),
            kind: ItemKind::File
        }]
    );
    // local deleted, remote edited → download the edit
    let w = World::new()
        .remote("a.txt", rf("1", "e2", 10, 1000))
        .state("a.txt", sfile("1", "e1", 10, 1000));
    assert!(matches!(w.plan()[0], SyncAction::Download { .. }));
    // gone on both sides → forget
    let w = World::new().state("a.txt", sfile("1", "e1", 10, 1000));
    assert_eq!(w.plan(), vec![SyncAction::Forget { path: p("a.txt") }]);
}

#[test]
fn locally_deleted_folder_is_trashed_remotely_as_one_unit() {
    let w = World::new()
        .remote("dir", rfolder("d"))
        .remote("dir/a.txt", rf("1", "e1", 10, 1000))
        .remote("dir/sub", rfolder("s"))
        .remote("dir/sub/b.txt", rf("2", "e2", 10, 1000))
        .state("dir", sfolder("d"))
        .state("dir/a.txt", sfile("1", "e1", 10, 1000))
        .state("dir/sub", sfolder("s"))
        .state("dir/sub/b.txt", sfile("2", "e2", 10, 1000));
    assert_eq!(
        w.plan(),
        vec![SyncAction::TrashRemote {
            path: p("dir"),
            id: RemoteId("FOLDER::z::d".into()),
            etag: "d-etag".into(),
            kind: ItemKind::Folder
        }],
        "children's deletes are covered by the folder trash"
    );
}

#[test]
fn locally_deleted_folder_comes_back_when_icloud_changed_something_inside() {
    let w = World::new()
        .remote("dir", rfolder("d"))
        .remote("dir/a.txt", rf("1", "e1", 10, 1000))
        .remote("dir/new.txt", rf("9", "e9", 3, 3000))
        .state("dir", sfolder("d"))
        .state("dir/a.txt", sfile("1", "e1", 10, 1000));
    assert_eq!(
        w.plan(),
        vec![
            SyncAction::CreateLocalFolder {
                path: p("dir"),
                remote_id: RemoteId("FOLDER::z::d".into())
            },
            SyncAction::Download {
                path: p("dir/new.txt"),
                remote: rfile("9", "e9", 3, 3000)
            },
            SyncAction::TrashRemote {
                path: p("dir/a.txt"),
                id: rid("1"),
                etag: "e1".into(),
                kind: ItemKind::File
            },
        ]
    );
}

#[test]
fn remotely_deleted_folder_mirrors_the_same_rules_locally() {
    // unchanged locally → trash the local folder as one unit
    let w = World::new()
        .local("dir", LocalNode::Folder)
        .local("dir/a.txt", lf(10, 1000))
        .state("dir", sfolder("d"))
        .state("dir/a.txt", sfile("1", "e1", 10, 1000));
    assert_eq!(
        w.plan(),
        vec![SyncAction::TrashLocal {
            path: p("dir"),
            kind: ItemKind::Folder
        }]
    );
    // a local edit inside → recreate remotely and upload the edit
    let w = World::new()
        .local("dir", LocalNode::Folder)
        .local("dir/a.txt", lf(12, 4000))
        .state("dir", sfolder("d"))
        .state("dir/a.txt", sfile("1", "e1", 10, 1000));
    assert_eq!(
        w.plan(),
        vec![
            SyncAction::CreateRemoteFolder { path: p("dir") },
            SyncAction::Upload {
                path: p("dir/a.txt"),
                replaces: None
            },
        ]
    );
}

#[test]
fn kind_mismatch_moves_the_local_item_aside() {
    let w = World::new().remote("x", rfolder("d")).local("x", lf(1, 1));
    assert_eq!(
        w.plan(),
        vec![
            SyncAction::MoveLocalAside { path: p("x") },
            SyncAction::CreateLocalFolder {
                path: p("x"),
                remote_id: RemoteId("FOLDER::z::d".into())
            },
        ]
    );
    let w = World::new()
        .remote("x", rf("1", "e1", 1, 1))
        .local("x", LocalNode::Folder);
    assert_eq!(
        w.plan(),
        vec![
            SyncAction::MoveLocalAside { path: p("x") },
            SyncAction::Download {
                path: p("x"),
                remote: rfile("1", "e1", 1, 1)
            },
        ]
    );
}

#[test]
fn folders_present_on_both_sides_are_recorded_once() {
    let w = World::new()
        .remote("dir", rfolder("d"))
        .local("dir", LocalNode::Folder);
    assert_eq!(
        w.plan(),
        vec![SyncAction::RecordFolder {
            path: p("dir"),
            remote_id: RemoteId("FOLDER::z::d".into())
        }]
    );
    // stale id in the record → re-record
    let w = World::new()
        .remote("dir", rfolder("d2"))
        .local("dir", LocalNode::Folder)
        .state("dir", sfolder("d"));
    assert!(matches!(w.plan()[0], SyncAction::RecordFolder { .. }));
}

#[test]
fn folder_deletes_run_deep_first_and_after_file_work() {
    let w = World::new()
        .remote("a", rfolder("a"))
        .remote("a/b", rfolder("b"))
        .remote("c", rfolder("c"))
        .remote("new.txt", rf("n", "en", 1, 1))
        .local("a", LocalNode::Folder)
        .state("a", sfolder("a"))
        .state("a/b", sfolder("b"))
        .state("c", sfolder("c"));
    let paths: Vec<String> = w.plan().iter().map(|a| a.path().to_string()).collect();
    assert_eq!(paths, vec!["new.txt", "a/b", "c"]);
}
