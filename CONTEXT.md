# WattDrive — Living Context

> Progress log, decisions, open questions. Update at the end of any session
> with meaningful changes; newest entries first.
>
> **Last updated:** 2026-10-30 (listing and parallel transfers)

## Overview

Personal tool: two-way iCloud Drive sync for Swatto's Omarchy box. Mirrors
WattMail's stack and standards (Rust workspace, Tauri v2, Vite/TS/DaisyUI,
keyring, SQLite, AppImage + updater, verify/fastcheck gates). Windows x64 is
also supported; macOS remains outside the current scope.

## Decisions

- **2026-10-30 iCloud write serialization.** Live concurrent upload registration
  returned `ZONE_BUSY` (409), `SERVER_OVERLOADED` (502), and `ATOMIC_FAILURE`
  (400). Keep content transfers parallel but serialize register/create/trash
  requests through a single-permit semaphore on the account client. A local HTTP service
  reproduces the rejected overlap and verifies parallel content plus serial
  mutations. Recognized busy codes use the existing rate-limit abort path;
  automatic polling waits after pass completion and local file events cannot
  bypass an offline wait. Do not blindly replay ambiguous upload requests.

- **2026-10-30 Listing pipeline.** Use a rolling queue of up to eight batches
  of ten folders, rather than waiting for each complete depth of the tree.
  The initial forty-folder benchmark favored smaller batches, but a full run
  with four workers still took 838.7 s for 3,245 folders. A larger read-only
  sample of 160 saved folders took 46.0 s with four batches of forty, 67.5 s
  with four batches of ten, and 20.1 s with eight batches of ten; no request
  failed. Sample timing is not a whole-drive speed guarantee. Preserve
  the missing-folder abort before any sync mutations. Listing progress shows
  folders checked/items found; file failures reach the popout during the pass.

- **2026-10-30 Parallel transfers and sync messages.** File transfers and
  adoption records share a rolling four-operation queue. Conflict preservation
  and folder setup finish first; deletions run after transfers drain. Fatal
  connection, rate-limit, authentication, state-store or prerequisite failures
  stop new work; active transfers finish and persist before returning. Failed
  work is re-planned on the next pass, without blindly replaying upload writes.
  Progress counts completed actions and does not jump to completion on abort.
  Detailed sync messages live in a keyboard-accessible dialog, with only a
  short status on the main screen. Transport errors strip signed URLs before
  display/logging. Synthetic delayed-provider tests cover concurrency, draining,
  retry and settling; WebDriver covers the message dialog and keyboard focus.

- **2026-10-29 Package archive collisions (0.1.7).** Reserve remote and persisted
  paths before assigning archive names, including case aliases on Windows.
  Prefer `.zip`; collisions use ` (iCloud package N).zip`. Reuse the persisted
  path by remote ID while it still belongs to the same source name and is not
  occupied by a real remote item. The existing state schema stores the mapping.
  Keep the original source path separately for the extracted-bundle guard.
  Regression passes cover first sync, later collisions, local edits, folder and
  case collisions, removal of the competing item, and settling on repeat syncs.

- **2026-10-28 Apple package downloads.** Apple's listing labels packages FILE
  and reports the unpacked size, but docws returns a package_token pointing to a
  ZIP. Resolve new remote versions before planning and map packages to `.zip`
  names. Validate archive structure and entry CRCs without extracting: this
  preserves Mac metadata and symlinks on Windows/Linux. Package ZIPs are download
  mirrors; local deletion restores the archive and local edits keep a conflict
  copy, never replace the source bundle. Extracted originals stop the pass before
  mutations; archive name collisions are resolved as described above. Persist is_package with an additive SQLite
  column (old rows default false), and record the actual local archive size.
  Unchanged remote versions reuse persisted classification. No schema removal
  or version bump is involved; installation rollback retains a pre-change DB copy.

- **2026-10-27 Windows support requested.** Keep the sync engine and iCloud
  authentication shared. Use Tauri's Windows tray and Windows Credential Manager
  for the existing encrypted vault. State lives in `%LOCALAPPDATA%\WattDrive`.
  Refuse incompatible names, case aliases and reparse points before sync actions
  rather than silently dropping items or overwriting an alias. Windows packaging
  is a per-user NSIS installer plus portable EXE using Evergreen WebView2.
  Apple Passwords integration is explicitly out of scope. WebDriver covers the
  desktop shell and settings; synthetic provider passes cover sync in Rust.

- **2026-10-27 Dependency audit and release approval.** Updated compatible Rust
  dependencies and WebdriverIO 10 clear security/unsoundness findings and npm
  advisories (including dev tools). Disable the updater's optional `zip` feature:
  published updates are raw AppImage/NSIS files, never legacy ZIP/tar archives.
  Signed verification and system-proxy support remain enabled. Cargo-deny checks
  licences and advisories in CI. Swatto explicitly approved publication with four
  Linux maintenance notices documented and visible as warnings: proc-macro-error
  (RUSTSEC-2024-0370), zvariant (2026-0370), zbus_names (2026-0371), and
  zvariant_derive (2026-0372). Upstream GTK/D-Bus dependents have no compatible
  published replacements. No advisory is ignored; vulnerabilities remain errors.
  Recheck by 2026-11-10. GitHub Actions is enabled for exact-commit CI and releases;
  releases stay draft until both platforms and checksums are uploaded.

- **2026-09-05 Secrets in an encrypted file, one key in the keyring.**
  gnome-keyring-daemon 50.0 on Omarchy aborts (`gkd_secret_service_get_pkcs11_session:
  assertion 'client' failed`) when two Secret Service operations from
  short-lived connections follow each other — the keyring crate's
  `sync-secret-service` backend does exactly that. Three crashes correlated:
  WattMail start-up (Sept 3), WattDrive sign-in and WattDrive start-up (Sept 5).
  Now `~/.local/share/WattDrive/secrets.bin` (AES-256-GCM, mode 0600) holds
  credentials + trust token + session; the keyring holds only `vault-key`, read
  once per process. A one-off migration moves the old two items across.
  WattMail has the same exposure and is not yet changed.
- **2026-09-05 Trust token duplicated with the credentials.** A lost session
  never costs a second factor; startup seeds a bare session from it.

- **2026-09-05 Native client, not rclone.** Own Rust port of the icloud.com
  web protocol (SRP, 2FA, drivews/docws) rather than bundling rclone: one
  binary, no sidecar, full control of sync semantics. Cost: we own the
  breakage whenever Apple changes the private API.
- **2026-09-05 Sync folder, not FUSE.** `~/iCloud Drive` mirrored both ways.
  Plain files, offline access, no fuse dependency, works with every app.
- **2026-09-05 Conservative sync rules.** Conflicts keep both; deletes never
  beat edits; nothing hard-deletes (in-root `.wattdrive-trash/`, iCloud
  Recently Deleted). Renames = delete + add in v1.
- **2026-09-05 Password stored in the keyring.** Apple's web session needs the
  password for SRP; the trust token only skips 2FA (about 30 days). Without it
  the user would re-type the password on every session expiry.
- **2026-09-05 Local trash inside the sync root.** Guarantees same-filesystem
  renames; ignored by the scanner via the `.wattdrive` prefix.

## State (2026-09-25)

## 2026-09-25 — README for a new install

- User-facing install, sign-in, settings, and build steps live in `README.md`.
  The release artifact is the x86_64 AppImage only. Sign-in stays Apple Account
  password plus the six-digit code; an app-specific password is the wrong
  credential for this SRP flow. No auth code changed.

## 2026-09-24 — Bug sweep (v0.1.4)

- A replace upload that trashes the remote file and then fails drops the sync
  record. The next pass uploads the local file again instead of treating the
  missing remote copy as a delete and moving the local file to trash.
- A download does not rename over a local file that appeared or changed while
  the bytes were in flight; the next pass keeps both.
- A remote folder that lists itself, and a local directory that is the same
  inode as an ancestor, are skipped instead of walking forever.
- A local-change trigger that does not start a pass (paused, signed out, or
  waiting for sign-in) is forgotten. Leaving it due spun the sync loop.

## 2026-09-06 — Sync data preservation (v0.1.3)

- Reject a download whose byte count differs from the listing before renaming
  it over the local file. Preserve the previous file/state and retry next pass.
- Stop a pass when preserving a conflict copy fails; never follow that failed
  rename with a download over the unpreserved local edit.
- When a local folder conflicts with a remote file, move its whole subtree
  aside and do not execute actions against the now-stale child paths. The
  preserved subtree uploads from its conflict name on the following pass.
- Persist the account/folder scope of sync records transactionally. Switching
  either starts a fresh baseline rather than interpreting missing old paths as
  deletions. The first upgraded pass also re-baselines legacy unscoped records.
- Added three full-engine regressions and a SQLite scope regression. Includes
  the upstream v0.1.2 incomplete-listing guard; that fix was already on main.
- Local frontend build passed. Rust toolchain unavailable here; native fmt,
  clippy, tests and desktop compilation must pass CI before tagging. No live
  iCloud account or desktop acceptance test was performed in this environment.

- **v0.1.2 (bug sweep, 2026-09-05 evening):** a folder missing from a batched
  iCloud listing now aborts the pass (`remote_tree::walk`) instead of reading as
  "deleted on iCloud" and moving its local files to the trash; local file
  changes no longer re-run a failed sign-in (and its notification) while the
  app waits for the user — only "Sync now" does; "Start at login" goes through
  Rust (`autostart.rs`, ported from WattMail) and is refused unless this
  process is the installed AppImage, with a start-up repair of a stale entry;
  updates download first and install only once no sync pass is running.

- **v0.1.0 tagged 2026-09-05** on 11e5cf5 after Swatto accepted the debug
  handoff (sign-in, first mirror, two-way uploads/deletes, new icon, batched
  listing ~9 s/pass). Release workflow builds the signed AppImage + latest.json.

- Workspace, all crates, Tauri shell and frontend written; unit + engine tests
  green against a fake drive (see `crates/application/src/engine_tests.rs`).
- **Live-verified 2026-09-05:** SRP sign-in + 2FA + trust, `accountLogin`,
  folder listing, download URLs and downloads all worked against Swatto's real
  account; first mirror of 420 items completed and a follow-up pass planned 0.
  Two zero-byte hidden files returned HTTP 400 on download — now created
  locally without a request (fix unverified live at time of writing). Uploads,
  replaces and deletes in both directions were then exercised live by Swatto
  on 2026-09-05 and synced correctly.
- Client id is icloud.com's public widget key (same as pyicloud/rclone).

## Open questions / known gaps

- Renames as delete+add: acceptable for v1, revisit with size+mtime matching.
- Remote walk is a full tree listing every pass (rolling batches, eight in flight).
  Large drives may want change-detection via folder etags.
- Etag format from `update/documents` vs folder listings unconfirmed; the
  planner's adopt-on-identical rule absorbs a mismatch without a transfer.
