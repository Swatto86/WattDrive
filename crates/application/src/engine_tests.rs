//! End-to-end passes against a fake drive and a temp folder: every planner
//! outcome is exercised through the real executor, so the wiring (paths, temp
//! files, mtimes, state records) is what gets tested, not the classifier.

use std::path::Path;
use std::sync::Arc;

use crate::executor::TRASH_DIR_NAME;
use crate::fake_drive::FakeDrive;
use crate::local::set_mtime_ms;
use crate::test_drives::{DropsFolder, Vanishing};
use crate::{MemoryStateStore, StateStore, SyncEngine, SyncReport};

struct Rig {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    drive: Arc<FakeDrive>,
    state: Arc<MemoryStateStore>,
    engine: SyncEngine,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("iCloud Drive");
    let drive = Arc::new(FakeDrive::default());
    let state = Arc::new(MemoryStateStore::default());
    let engine = SyncEngine::new(root.clone(), drive.clone(), state.clone(), "testbox".into());
    Rig {
        _dir: dir,
        root,
        drive,
        state,
        engine,
    }
}

async fn run(engine: &SyncEngine) -> SyncReport {
    let report = engine.run_once(&|_| {}).await.unwrap();
    assert!(
        report.failures.is_empty(),
        "failures: {:?}",
        report.failures
    );
    assert!(report.aborted.is_none());
    report
}

fn write_local(root: &Path, rel: &str, content: &[u8], mtime_ms: i64) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
    set_mtime_ms(&p, mtime_ms).unwrap();
}

fn local_names(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(".wattdrive") {
                continue;
            }
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if e.file_type().unwrap().is_dir() {
                walk(&e.path(), &rel, out);
            } else {
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, "", &mut out);
    out.sort();
    out
}

const T0: i64 = 1_700_000_000_000;

#[tokio::test]
async fn first_pass_mirrors_icloud_locally_and_second_pass_is_quiet() {
    let r = rig();
    r.drive.add_file("Docs/notes.md", b"# notes", T0);
    r.drive.add_file("top.txt", b"top", T0 + 10_000);

    let report = run(&r.engine).await;
    assert_eq!(report.downloaded, 2);
    assert_eq!(report.folders_created, 1);
    assert_eq!(local_names(&r.root), vec!["Docs/notes.md", "top.txt"]);
    assert_eq!(
        std::fs::read(r.root.join("Docs/notes.md")).unwrap(),
        b"# notes"
    );
    let meta = std::fs::metadata(r.root.join("top.txt")).unwrap();
    assert_eq!(
        crate::local::mtime_ms(&meta),
        T0 + 10_000,
        "mtime mirrors iCloud"
    );

    let again = run(&r.engine).await;
    assert_eq!(again.planned, 0, "nothing to do after a clean sync");
}

#[tokio::test]
async fn new_local_files_upload_with_their_folders() {
    let r = rig();
    write_local(&r.root, "Work/plan.txt", b"plan", T0);
    let report = run(&r.engine).await;
    assert_eq!(report.uploaded, 1);
    assert_eq!(report.folders_created, 1);
    assert_eq!(r.drive.read("Work/plan.txt").unwrap(), b"plan");
    assert_eq!(run(&r.engine).await.planned, 0);
}

#[tokio::test]
async fn edits_flow_in_both_directions() {
    let r = rig();
    r.drive.add_file("a.txt", b"v1", T0);
    run(&r.engine).await;

    // iCloud edit → local updated
    r.drive.edit_file("a.txt", b"v2 from icloud", T0 + 60_000);
    let report = run(&r.engine).await;
    assert_eq!(report.downloaded, 1);
    assert_eq!(
        std::fs::read(r.root.join("a.txt")).unwrap(),
        b"v2 from icloud"
    );

    // local edit → iCloud updated, old version trashed first
    write_local(&r.root, "a.txt", b"v3 from linux", T0 + 120_000);
    let report = run(&r.engine).await;
    assert_eq!(report.uploaded, 1);
    assert_eq!(r.drive.read("a.txt").unwrap(), b"v3 from linux");
    assert_eq!(
        r.drive.trashed_count(),
        1,
        "superseded version went to iCloud trash"
    );
    assert_eq!(run(&r.engine).await.planned, 0);
}

#[tokio::test]
async fn deletions_go_to_a_trash_on_the_other_side() {
    let r = rig();
    r.drive.add_file("keep.txt", b"k", T0);
    r.drive.add_file("gone-remote.txt", b"g", T0);
    r.drive.add_file("gone-local.txt", b"l", T0);
    run(&r.engine).await;

    r.drive.remove("gone-remote.txt");
    std::fs::remove_file(r.root.join("gone-local.txt")).unwrap();
    let report = run(&r.engine).await;
    assert_eq!(report.trashed_local, 1);
    assert_eq!(report.trashed_remote, 1);
    assert_eq!(local_names(&r.root), vec!["keep.txt"]);
    assert_eq!(r.drive.paths(), vec!["keep.txt"]);
    // the locally "deleted" file is recoverable from the in-root trash
    let trash = r.root.join(TRASH_DIR_NAME);
    let batch = std::fs::read_dir(&trash)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(std::fs::read(batch.join("gone-remote.txt")).unwrap(), b"g");
    assert_eq!(run(&r.engine).await.planned, 0);
}

#[tokio::test]
async fn a_locally_edited_file_survives_a_remote_delete() {
    let r = rig();
    r.drive.add_file("a.txt", b"v1", T0);
    run(&r.engine).await;
    r.drive.remove("a.txt");
    write_local(&r.root, "a.txt", b"edited offline", T0 + 60_000);
    let report = run(&r.engine).await;
    assert_eq!(report.uploaded, 1);
    assert_eq!(report.trashed_local, 0);
    assert_eq!(r.drive.read("a.txt").unwrap(), b"edited offline");
}

#[tokio::test]
async fn a_conflict_keeps_both_versions_and_uploads_the_copy() {
    let r = rig();
    r.drive.add_file("a.txt", b"v1", T0);
    run(&r.engine).await;
    r.drive.edit_file("a.txt", b"icloud edit", T0 + 60_000);
    write_local(&r.root, "a.txt", b"linux edit", T0 + 90_000);

    let report = run(&r.engine).await;
    assert_eq!(report.conflicts, 1);
    assert_eq!(report.downloaded, 1);
    let names = local_names(&r.root);
    assert_eq!(names.len(), 2, "{names:?}");
    assert_eq!(std::fs::read(r.root.join("a.txt")).unwrap(), b"icloud edit");
    let copy = names
        .iter()
        .find(|n| n.contains("(conflict testbox"))
        .unwrap();
    assert!(copy.ends_with(".txt"));
    assert_eq!(std::fs::read(r.root.join(copy)).unwrap(), b"linux edit");

    // next pass: the conflict copy is a new local file → uploaded, then quiet
    let report = run(&r.engine).await;
    assert_eq!(report.uploaded, 1);
    assert!(r.drive.exists(copy));
    assert_eq!(run(&r.engine).await.planned, 0);
}

#[tokio::test]
async fn deleting_a_folder_locally_trashes_it_on_icloud_as_one_unit() {
    let r = rig();
    r.drive.add_file("Old/a.txt", b"a", T0);
    r.drive.add_file("Old/Sub/b.txt", b"b", T0);
    r.drive.add_file("keep.txt", b"k", T0);
    run(&r.engine).await;
    std::fs::remove_dir_all(r.root.join("Old")).unwrap();
    let report = run(&r.engine).await;
    assert_eq!(report.trashed_remote, 1);
    assert_eq!(r.drive.paths(), vec!["keep.txt"]);
    assert_eq!(
        run(&r.engine).await.planned,
        0,
        "subtree records were dropped"
    );
}

#[tokio::test]
async fn a_failed_download_is_retried_next_pass_and_leaves_no_partial_file() {
    let r = rig();
    r.drive.add_file("a.txt", b"a", T0);
    r.drive.add_file("b.txt", b"b", T0);
    run(&r.engine).await;
    // Simulate iCloud reporting a file it can no longer serve.
    r.drive.edit_file("b.txt", b"new", T0 + 5_000);
    let listing_id = {
        let children = r.drive.list_children(&r.drive.root()).await.unwrap();
        children
            .into_iter()
            .find(|c| c.name == "b.txt")
            .unwrap()
            .node
            .id()
            .clone()
    };
    use wattdrive_domain::RemoteDrive;
    // Same records as the first engine: this is the same install seeing a
    // remote edit it cannot fetch.
    let flaky = SyncEngine::new(
        r.root.clone(),
        Arc::new(Vanishing(r.drive.clone(), listing_id)),
        r.state.clone(),
        "testbox".into(),
    );
    let report = flaky.run_once(&|_| {}).await.unwrap();
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].path, "b.txt");
    assert!(
        report.aborted.is_none(),
        "a per-file error does not stop the pass"
    );
    let leftovers: Vec<_> = std::fs::read_dir(&r.root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(".wattdrive-part-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "partial download cleaned up: {leftovers:?}"
    );
    assert_eq!(
        std::fs::read(r.root.join("b.txt")).unwrap(),
        b"b",
        "old content untouched"
    );
}

#[tokio::test]
async fn a_folder_missing_from_the_listing_stops_the_pass_instead_of_trashing() {
    let r = rig();
    r.drive.add_file("Docs/notes.md", b"# notes", T0);
    r.drive.add_file("Docs/plan.md", b"plan", T0);
    run(&r.engine).await;
    let docs_id = r.drive.id_of("Docs").unwrap();

    let partial = SyncEngine::new(
        r.root.clone(),
        Arc::new(DropsFolder(r.drive.clone(), docs_id)),
        r.state.clone(),
        "testbox".into(),
    );
    let Err(err) = partial.run_once(&|_| {}).await else {
        panic!("pass must fail");
    };
    assert!(err.to_string().contains("Docs"), "{err}");
    assert_eq!(
        local_names(&r.root),
        vec!["Docs/notes.md", "Docs/plan.md"],
        "nothing was moved to the local trash"
    );
    assert!(!r.root.join(TRASH_DIR_NAME).exists());
    assert_eq!(run(&r.engine).await.planned, 0, "state untouched");
}

#[tokio::test]
async fn truncated_download_keeps_original_and_retries() {
    let r = rig();
    r.drive.add_file("a.txt", b"original", T0);
    run(&r.engine).await;
    let before = r.state.load_all().await.unwrap();
    r.drive.edit_file("a.txt", b"replacement", T0 + 5000);
    r.drive
        .truncate_download
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let report = r.engine.run_once(&|_| {}).await.unwrap();
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.downloaded, 0);
    assert_eq!(std::fs::read(r.root.join("a.txt")).unwrap(), b"original");
    assert_eq!(r.state.load_all().await.unwrap(), before);
    assert_eq!(std::fs::read_dir(&r.root).unwrap().count(), 1);
    r.drive
        .truncate_download
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(run(&r.engine).await.downloaded, 1);
    assert_eq!(
        std::fs::read(r.root.join("a.txt")).unwrap(),
        b"replacement"
    );
}

#[tokio::test]
async fn failed_conflict_preservation_stops_before_overwrite() {
    let r = rig();
    r.drive.add_file("a.txt", b"original", T0);
    run(&r.engine).await;
    write_local(&r.root, "a.txt", b"local edit", T0 + 5000);
    r.drive.edit_file("a.txt", b"remote edit", T0 + 9000);
    // The invalid conflict destination deterministically makes rename fail.
    let engine = SyncEngine::new(
        r.root.clone(),
        r.drive.clone(),
        r.state.clone(),
        "missing/parent".into(),
    );
    let report = engine.run_once(&|_| {}).await.unwrap();
    assert!(report.aborted.is_some());
    assert_eq!(report.downloaded, 0);
    assert_eq!(std::fs::read(r.root.join("a.txt")).unwrap(), b"local edit");
}

#[tokio::test]
async fn folder_replaced_by_remote_file_preserves_its_entire_subtree() {
    let r = rig();
    r.drive.add_file("Docs/sub/a.txt", b"original", T0);
    run(&r.engine).await;
    write_local(&r.root, "Docs/sub/a.txt", b"local edit", T0 + 5000);
    r.drive.remove("Docs");
    r.drive.add_file("Docs", b"remote file", T0 + 9000);
    let report = run(&r.engine).await;
    assert_eq!(report.conflicts, 1);
    assert_eq!(report.downloaded, 1);
    let names = local_names(&r.root);
    let copy = names
        .iter()
        .find(|name| name.ends_with("/sub/a.txt"))
        .unwrap();
    assert_eq!(std::fs::read(r.root.join(copy)).unwrap(), b"local edit");
    assert_eq!(std::fs::read(r.root.join("Docs")).unwrap(), b"remote file");
    assert_eq!(run(&r.engine).await.uploaded, 1);
    assert_eq!(run(&r.engine).await.planned, 0);
}
