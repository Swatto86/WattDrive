# WattDrive — Architecture

Rust workspace + Tauri v2 shell, Linux only. Dependencies point inward:
`src-tauri (presentation + composition) → application → domain`, with
`infrastructure` implementing the domain/application ports.

```
crates/domain          RelPath, RemoteNode/LocalNode/SyncEntry, RemoteDrive port,
                       plan() — the pure two-way planner (no I/O)
crates/application     SyncEngine::run_once: scan local → walk remote → plan →
                       execute; StateStore port; local fs helpers; ignore rules
crates/infrastructure  icloud/{srp,session,auth,drive,adapter,wire}: Apple web
                       API client; session_store (keyring); state_db (SQLite)
src-tauri              lib.rs composition root, commands (IPC), sync_runner
                       (timer + inotify loop), tray_linux (ksni), settings,
                       status DTOs, notify, linux_webkit quirks
src/                   Vite + TypeScript + Tailwind/DaisyUI, vanilla TS
```

## Sync model

One pass = three snapshots keyed by relative path (remote listing, local scan,
last-sync records) → `domain::plan` → ordered `SyncAction`s → `Executor`.
Change on a side = differs from the record. One side changed → copy across.
Both changed → local moved aside as a conflict copy, remote wins the name.
Deletes propagate only against an unchanged other side and always go to a
trash (in-root `.wattdrive-trash/` locally, Recently Deleted on iCloud).
Folders deleted on one side are trashed on the other only when nothing inside
must travel the other way. Identical content (size + mtime within 2 s) is
adopted, never transferred — this also absorbs etag drift after our own uploads.

Downloads land in `.wattdrive-part-*` temp files, get the remote mtime, then
rename into place. Uploads follow icloud.com's three steps (upload slot → raw
POST → `update/documents`), trashing the superseded version first.

## Auth

`icloud::auth::IcloudClient`: SRP-6a (Apple variant, `srp.rs`) against
idmsa.apple.com → 409 means 2FA (push requested explicitly; SMS fallback) →
`/2sv/trust` → `setup/ws/1/accountLogin` gives cookies + per-account service
hosts (`drivews`, `docws`). ADP accounts additionally need PCS cookies via
`requestPCS`, approved on a trusted device. Rejected sessions (401/421/423/450)
trigger one silent re-auth using the stored password + trust token; a needed
second factor surfaces as `SignInRequired` and pauses syncing.

## Runtime

`sync_runner` owns the loop: startup delay, poll timer, debounced inotify
trigger (with a grace window so our own downloads do not retrigger), manual
sync, reconfigure, sign-in/out. Status snapshots are pushed to the window
(`sync-status` event) and the ksni tray (icon per state). All keyring and
notify-rust calls run on plain threads, never on Tokio workers.
