# WattDrive — Living Context

> Progress log, decisions, open questions. Update at the end of any session
> with meaningful changes; newest entries first.
>
> **Last updated:** 2026-09-05

## Overview

Personal tool: two-way iCloud Drive sync for Swatto's Omarchy box. Mirrors
WattMail's stack and standards (Rust workspace, Tauri v2, Vite/TS/DaisyUI,
keyring, SQLite, AppImage + updater, verify/fastcheck gates). Linux only by
decision — no Windows/macOS builds.

## Decisions

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

- Workspace, all crates, Tauri shell and frontend written; unit + engine tests
  green against a fake drive (see `crates/application/src/engine_tests.rs`).
- **Not yet live-verified against Apple.** Nothing in the auth or drive
  modules has been exercised against real iCloud in this repo; the first
  sign-in from the debug build is the milestone-1 acceptance step.
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
