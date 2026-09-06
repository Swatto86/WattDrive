//! SQLite implementation of the application's `StateStore`: one row per synced
//! path. Opened once; every call hops to the blocking pool.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use wattdrive_application::{StateError, StateStore};
use wattdrive_domain::{ItemKind, RelPath, RemoteId, SyncEntry};

pub struct SqliteStateStore {
    conn: Arc<Mutex<Connection>>,
}

fn db_err(e: impl std::fmt::Display) -> StateError {
    StateError::Store(e.to_string())
}

impl SqliteStateStore {
    pub fn open(path: &Path) -> Result<Self, StateError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(db_err)?;
        }
        let conn = Connection::open(path).map_err(db_err)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS entries (
               path TEXT PRIMARY KEY,
               kind TEXT NOT NULL,
               remote_id TEXT NOT NULL,
               remote_etag TEXT NOT NULL,
               size INTEGER NOT NULL,
               local_mtime_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .map_err(db_err)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// In-memory database (tests).
    pub fn in_memory() -> Result<Self, StateError> {
        let conn = Connection::open_in_memory().map_err(db_err)?;
        conn.execute_batch(
            "CREATE TABLE entries (path TEXT PRIMARY KEY, kind TEXT NOT NULL, remote_id TEXT NOT NULL,
             remote_etag TEXT NOT NULL, size INTEGER NOT NULL, local_mtime_ms INTEGER NOT NULL);
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .map_err(db_err)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    async fn with_conn<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T, StateError> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_| StateError::Store("db mutex poisoned".into()))?;
            f(&guard).map_err(db_err)
        })
        .await
        .map_err(db_err)?
    }

    /// Free-form key/value (e.g. last sync time), for the app's status screen.
    pub async fn get_meta(&self, key: &str) -> Result<Option<String>, StateError> {
        let key = key.to_string();
        self.with_conn(move |c| {
            c.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()
        })
        .await
    }

    pub async fn set_meta(&self, key: &str, value: &str) -> Result<(), StateError> {
        let (key, value) = (key.to_string(), value.to_string());
        self.with_conn(move |c| {
            c.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
        })
        .await
    }

    /// Records belong to one account and local root. A new scope starts a
    /// fresh comparison, never interprets the old root's absence as deletion.
    pub async fn bind_scope(&self, folder: &Path, account: &str) -> Result<(), StateError> {
        let scope = serde_json::to_string(&(folder, account)).map_err(db_err)?;
        self.with_conn(move |c| {
            let tx = c.unchecked_transaction()?;
            let previous: Option<String> = tx
                .query_row("SELECT value FROM meta WHERE key = 'sync_scope'", [], |r| {
                    r.get(0)
                })
                .optional()?;
            if previous.as_deref() != Some(scope.as_str()) {
                tx.execute("DELETE FROM entries", [])?;
                tx.execute("DELETE FROM meta WHERE key = 'last_sync'", [])?;
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES ('sync_scope', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![scope],
                )?;
            }
            tx.commit()
        })
        .await
    }
}

fn kind_str(k: ItemKind) -> &'static str {
    match k {
        ItemKind::File => "file",
        ItemKind::Folder => "folder",
    }
}

#[async_trait]
impl StateStore for SqliteStateStore {
    async fn load_all(&self) -> Result<BTreeMap<RelPath, SyncEntry>, StateError> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT path, kind, remote_id, remote_etag, size, local_mtime_ms FROM entries",
            )?;
            let rows = stmt.query_map([], |r| {
                let path: String = r.get(0)?;
                let kind: String = r.get(1)?;
                let size: i64 = r.get(4)?;
                Ok((
                    path,
                    SyncEntry {
                        kind: if kind == "folder" {
                            ItemKind::Folder
                        } else {
                            ItemKind::File
                        },
                        remote_id: RemoteId(r.get(2)?),
                        remote_etag: r.get(3)?,
                        size: u64::try_from(size).unwrap_or(0),
                        local_mtime_ms: r.get(5)?,
                    },
                ))
            })?;
            let mut out = BTreeMap::new();
            for row in rows {
                let (path, entry) = row?;
                match RelPath::new(&path) {
                    Ok(p) => {
                        out.insert(p, entry);
                    }
                    Err(e) => tracing::warn!("dropping bad path {path:?} from state db: {e}"),
                }
            }
            Ok(out)
        })
        .await
    }

    async fn put(&self, path: &RelPath, entry: &SyncEntry) -> Result<(), StateError> {
        let path = path.to_string();
        let entry = entry.clone();
        self.with_conn(move |c| {
            c.execute(
                "INSERT INTO entries (path, kind, remote_id, remote_etag, size, local_mtime_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(path) DO UPDATE SET kind = excluded.kind, remote_id = excluded.remote_id,
                   remote_etag = excluded.remote_etag, size = excluded.size, local_mtime_ms = excluded.local_mtime_ms",
                params![
                    path,
                    kind_str(entry.kind),
                    entry.remote_id.0,
                    entry.remote_etag,
                    i64::try_from(entry.size).unwrap_or(i64::MAX),
                    entry.local_mtime_ms
                ],
            )
            .map(|_| ())
        })
        .await
    }

    async fn remove(&self, path: &RelPath) -> Result<(), StateError> {
        let path = path.to_string();
        self.with_conn(move |c| {
            c.execute("DELETE FROM entries WHERE path = ?1", params![path])
                .map(|_| ())
        })
        .await
    }

    async fn remove_subtree(&self, path: &RelPath) -> Result<(), StateError> {
        let path = path.to_string();
        let prefix = format!("{path}/");
        self.with_conn(move |c| {
            // `substr` beats LIKE here: a path with `%` or `_` in it must not
            // act as a wildcard.
            c.execute(
                "DELETE FROM entries WHERE path = ?1 OR substr(path, 1, length(?2)) = ?2",
                params![path, prefix],
            )
            .map(|_| ())
        })
        .await
    }

    async fn clear(&self) -> Result<(), StateError> {
        self.with_conn(|c| c.execute_batch("DELETE FROM entries; DELETE FROM meta;"))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn changing_scope_discards_old_deletion_baseline() {
        let store = SqliteStateStore::in_memory().unwrap();
        let folder = std::path::Path::new("/sync/a");
        store.bind_scope(folder, "a@example.com").await.unwrap();
        let path = p("important");
        let entry = entry("file");
        store.put(&path, &entry).await.unwrap();
        store.bind_scope(folder, "a@example.com").await.unwrap();
        assert_eq!(store.load_all().await.unwrap().len(), 1);
        store
            .bind_scope(std::path::Path::new("/sync/b"), "a@example.com")
            .await
            .unwrap();
        assert!(store.load_all().await.unwrap().is_empty());
        store.put(&path, &entry).await.unwrap();
        store
            .bind_scope(std::path::Path::new("/sync/b"), "b@example.com")
            .await
            .unwrap();
        assert!(store.load_all().await.unwrap().is_empty());
    }

    fn p(s: &str) -> RelPath {
        RelPath::new(s).unwrap()
    }
    fn entry(id: &str) -> SyncEntry {
        SyncEntry {
            kind: ItemKind::File,
            remote_id: RemoteId(id.into()),
            remote_etag: "e".into(),
            size: 3,
            local_mtime_ms: 99,
        }
    }

    #[tokio::test]
    async fn put_load_remove_and_subtree_delete_without_wildcard_surprises() {
        let db = SqliteStateStore::in_memory().unwrap();
        db.put(&p("a"), &SyncEntry::folder(RemoteId("F".into())))
            .await
            .unwrap();
        db.put(&p("a/x.txt"), &entry("1")).await.unwrap();
        db.put(&p("a/sub/y.txt"), &entry("2")).await.unwrap();
        db.put(&p("ab/z.txt"), &entry("3")).await.unwrap();
        db.put(&p("a%/w.txt"), &entry("4")).await.unwrap();
        // overwrite keeps one row
        db.put(&p("a/x.txt"), &entry("1b")).await.unwrap();

        let all = db.load_all().await.unwrap();
        assert_eq!(all.len(), 5);
        assert_eq!(all[&p("a/x.txt")].remote_id.0, "1b");
        assert_eq!(all[&p("a")].kind, ItemKind::Folder);

        db.remove_subtree(&p("a")).await.unwrap();
        let left: Vec<String> = db
            .load_all()
            .await
            .unwrap()
            .keys()
            .map(|k| k.to_string())
            .collect();
        assert_eq!(
            left,
            vec!["a%/w.txt", "ab/z.txt"],
            "siblings sharing a prefix survive"
        );

        db.remove(&p("ab/z.txt")).await.unwrap();
        assert_eq!(db.load_all().await.unwrap().len(), 1);
        db.clear().await.unwrap();
        assert!(db.load_all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn meta_roundtrip_and_reopen_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/sync.db");
        {
            let db = SqliteStateStore::open(&path).unwrap();
            db.set_meta("last_sync", "2026-09-05T10:00:00Z")
                .await
                .unwrap();
            db.put(&p("k.txt"), &entry("9")).await.unwrap();
        }
        let db = SqliteStateStore::open(&path).unwrap();
        assert_eq!(
            db.get_meta("last_sync").await.unwrap().as_deref(),
            Some("2026-09-05T10:00:00Z")
        );
        assert_eq!(db.get_meta("missing").await.unwrap(), None);
        assert_eq!(db.load_all().await.unwrap().len(), 1);
    }
}
