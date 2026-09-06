# WattDrive — Living Context

> Progress log, decisions, open questions. Update at the end of any session
> with meaningful changes; newest entries first.
>
> **Last updated:** 2026-09-05 (bug sweep, v0.1.2)

## Overview

Personal tool: two-way iCloud Drive sync for Swatto's Omarchy box. Mirrors
WattMail's stack and standards (Rust workspace, Tauri v2, Vite/TS/DaisyUI,
keyring, SQLite, AppImage + updater, verify/fastcheck gates). Linux only by
decision — no Windows/macOS builds.

## Decisions

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

## State (2026-09-05)

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

- WebDriver end-to-end suite (tauri.md hard requirement) not yet added: this
  host has no `WebKitWebDriver`/`tauri-driver`; plan is to add the suite and
  run it in CI (ubuntu-22.04 has `webkit2gtk-driver`).
- Renames as delete+add: acceptable for v1, revisit with size+mtime matching.
- Remote walk is a full tree listing every pass (level-parallel, 6 in flight).
  Large drives may want change-detection via folder etags.
- Etag format from `update/documents` vs folder listings unconfirmed; the
  planner's adopt-on-identical rule absorbs a mismatch without a transfer.
