//! Local folder scan and the small filesystem helpers the executor needs.
//! Blocking work runs on the blocking pool; nothing here touches the runtime.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wattdrive_domain::{LocalNode, RelPath};

use crate::ignore::is_ignored_name;

/// Snapshot the sync folder. Symlinks are not followed, unreadable entries and
/// non-UTF-8 names are skipped with a warning, ignored names are dropped.
pub async fn scan(root: PathBuf) -> io::Result<BTreeMap<RelPath, LocalNode>> {
    tokio::task::spawn_blocking(move || {
        let mut out = BTreeMap::new();
        scan_dir(&root, None, &mut out)?;
        Ok(out)
    })
    .await
    .map_err(|e| io::Error::other(format!("scan task failed: {e}")))?
}

fn scan_dir(
    dir: &Path,
    rel: Option<&RelPath>,
    out: &mut BTreeMap<RelPath, LocalNode>,
) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("skipping unreadable entry in {}: {e}", dir.display());
                continue;
            }
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            tracing::warn!("skipping non-UTF-8 name in {}", dir.display());
            continue;
        };
        if is_ignored_name(&name) {
            continue;
        }
        let Ok(path) = RelPath::child(rel, &name) else {
            continue;
        };
        // symlink_metadata so a link is seen as a link, never followed.
        let meta = match std::fs::symlink_metadata(entry.path()) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("skipping {}: {e}", entry.path().display());
                continue;
            }
        };
        if meta.file_type().is_symlink() {
            tracing::debug!("skipping symlink {}", path);
            continue;
        }
        if meta.is_dir() {
            out.insert(path.clone(), LocalNode::Folder);
            scan_dir(&entry.path(), Some(&path), out)?;
        } else if meta.is_file() {
            out.insert(
                path,
                LocalNode::File {
                    size: meta.len(),
                    mtime_ms: mtime_ms(&meta),
                },
            );
        }
    }
    Ok(())
}

/// Modification time as Unix milliseconds (0 when the filesystem has none).
pub fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Stamp a file with the given modification time.
pub fn set_mtime_ms(path: &Path, ms: i64) -> io::Result<()> {
    let when = UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).unwrap_or(0));
    std::fs::File::options()
        .write(true)
        .open(path)?
        .set_modified(when)
}

/// Read back the (size, mtime) stamp the scanner would report for `path`.
pub fn file_stamp(path: &Path) -> io::Result<(u64, i64)> {
    let meta = std::fs::metadata(path)?;
    Ok((meta.len(), mtime_ms(&meta)))
}

/// `name` with a conflict marker before its extension, e.g.
/// `report.docx` → `report (conflict swatarch 2026-09-05 1412).docx`.
pub fn conflict_name(name: &str, host: &str, now: SystemTime) -> String {
    let stamp = time::OffsetDateTime::from(now)
        .format(&time::macros::format_description!(
            "[year]-[month]-[day] [hour][minute]"
        ))
        .unwrap_or_else(|_| "undated".to_string());
    let marker = format!(" (conflict {host} {stamp})");
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{}{marker}{}", &name[..dot], &name[dot..]),
        _ => format!("{name}{marker}"),
    }
}

/// A folder name for one trash batch: `2026-09-05T141233`.
pub fn trash_batch_name(now: SystemTime) -> String {
    time::OffsetDateTime::from(now)
        .format(&time::macros::format_description!(
            "[year]-[month]-[day]T[hour][minute][second]"
        ))
        .unwrap_or_else(|_| "undated".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scan_reports_files_folders_and_skips_ignored_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Docs/Sub")).unwrap();
        std::fs::write(root.join("Docs/a.txt"), b"hello").unwrap();
        std::fs::write(root.join("Docs/Sub/b.md"), b"# hi").unwrap();
        std::fs::write(root.join(".DS_Store"), b"").unwrap();
        std::fs::write(root.join("Docs/.wattdrive-part-x"), b"partial").unwrap();
        std::os::unix::fs::symlink(root.join("Docs/a.txt"), root.join("link.txt")).unwrap();

        let tree = scan(root.to_path_buf()).await.unwrap();
        let keys: Vec<&str> = tree.keys().map(|k| k.as_str()).collect();
        assert_eq!(
            keys,
            vec!["Docs", "Docs/Sub", "Docs/Sub/b.md", "Docs/a.txt"]
        );
        match &tree[&RelPath::new("Docs/a.txt").unwrap()] {
            LocalNode::File { size, mtime_ms } => {
                assert_eq!(*size, 5);
                assert!(
                    *mtime_ms > 1_600_000_000_000,
                    "mtime looks like ms since epoch"
                );
            }
            other => panic!("expected file, got {other:?}"),
        }
    }

    #[test]
    fn set_and_read_mtime_roundtrip_at_ms_precision() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x");
        std::fs::write(&f, b"x").unwrap();
        set_mtime_ms(&f, 1_700_000_000_123).unwrap();
        assert_eq!(file_stamp(&f).unwrap(), (1, 1_700_000_000_123));
    }

    #[test]
    fn conflict_name_keeps_extension_and_handles_dotfiles() {
        let t = UNIX_EPOCH + Duration::from_secs(1_757_000_000); // 2025-09-04 15:33:20 UTC
        assert_eq!(
            conflict_name("report.docx", "box", t),
            "report (conflict box 2025-09-04 1533).docx"
        );
        assert_eq!(
            conflict_name(".bashrc", "box", t),
            ".bashrc (conflict box 2025-09-04 1533)"
        );
        assert_eq!(
            conflict_name("README", "box", t),
            "README (conflict box 2025-09-04 1533)"
        );
        assert_eq!(trash_batch_name(t), "2025-09-04T153320");
    }
}
