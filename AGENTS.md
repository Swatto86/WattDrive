# WattDrive

Two-way iCloud Drive ↔ local folder sync for Linux, built for Swatto's Omarchy box. A Tauri v2
shell over a Rust workspace (`domain` ← `application` ← `infrastructure`, composed in
`src-tauri`) that talks to icloud.com's private web API, ported from rclone's iclouddrive backend
(SRP sign-in, 2FA, drivews/docws). It mirrors WattMail's stack and standards. Linux only by
decision; the one shipped artifact is `WattDrive_<version>_amd64.AppImage` (x86_64). Public
repository `Swatto86/WattDrive`, single branch `main`.

## Build, test, verify

- Inner loop: `scripts/fastcheck.sh [-p <crate>]`; the app itself with `npx tauri dev`.
- Full gate: `bash scripts/verify.sh`: frontend build, `cargo fmt --check`, clippy with
  `-D warnings`, tests, and agreement of the version in `src-tauri/Cargo.toml`,
  `src-tauri/tauri.conf.json` and `package.json`.
- Handoff build: `npx tauri build --debug --no-bundle`, then copy `target/debug/wattdrive-desktop`
  to `~/Downloads`. The binary is in the workspace root's `target/`, not `src-tauri/target/`,
  because `src-tauri` is a workspace member.
- Release: push a `v*` tag once CI is green on that exact SHA. `release.yml` builds the AppImage
  and signs the updater bundle with the `TAURI_SIGNING_PRIVATE_KEY` repository secret.

## Where things are

- `crates/domain/src/plan.rs`: the sync rules; `plan_tests.rs` is the spec.
- `crates/application/src/engine.rs` and `executor.rs`: one pass end to end; `engine_tests.rs`
  runs real passes against `fake_drive.rs` in a temp dir.
- `crates/infrastructure/src/icloud/auth.rs`: sign-in, 2FA, trust, re-auth; `srp.rs` has
  reference vectors; `wire.rs` has a decode test per payload.
- `src-tauri/src/sync_runner.rs`: the background loop and status. `commands.rs`: IPC, every
  argument validated.

## Rules of the road

- Never `.unwrap()` or `.expect()` outside tests (workspace lints and `-D warnings`).
- No lock guard across an `.await` (bind headers first; see `auth.rs`).
- Keyring, notify-rust and ksni calls only on plain `std::thread`s, never Tokio workers.
- Any change to a decode or encode path needs a test in `wire.rs`.
- Sign-in is the Apple Account password plus the six-digit code; an app-specific password is the
  wrong credential for this SRP flow.
- Secrets live in `~/.local/share/WattDrive/secrets.bin` (AES-256-GCM). The keyring holds only
  `vault-key`, read once per process, because gnome-keyring-daemon 50.0 aborts when two Secret
  Service calls from short-lived connections follow each other (`CONTEXT.md`, 2026-09-05).
- Sync is conservative: conflicts keep both copies, deletes never beat edits, nothing is
  hard-deleted (`.wattdrive-trash/` in the sync root, iCloud's Recently Deleted).

## Where longer material lives

- `README.md`: the install and sign-in guide, the only user-facing document.
- `ARCHITECTURE.md`: layers, the sync model, auth and the runtime.
- `CONTEXT.md`: decisions, state and the progress log, newest first; read the entries a task needs.
