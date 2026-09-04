# WattDrive — AI Context

## System overview

Two-way iCloud Drive ↔ local folder sync for Linux (Omarchy). Tauri v2 shell,
Rust workspace, private icloud.com web API. See ARCHITECTURE.md for layers and
CONTEXT.md for decisions/state.

## Where things are

- `crates/domain/src/plan.rs` — the sync rules; `plan_tests.rs` is the spec.
- `crates/application/src/engine.rs` + `executor.rs` — one pass end to end;
  `engine_tests.rs` runs real passes against `fake_drive.rs` in a temp dir.
- `crates/infrastructure/src/icloud/auth.rs` — sign-in/2FA/trust/reauth;
  `srp.rs` has reference vectors; `wire.rs` has decode tests per payload.
- `src-tauri/src/sync_runner.rs` — the background loop and status.
- `src-tauri/src/commands.rs` — IPC; every argument validated.
- `scripts/verify.sh` — full gate; `scripts/fastcheck.sh [-p crate]` — inner loop.

## Rules of the road

- Never `.unwrap()`/`.expect()` outside tests (workspace lints + `-D warnings`).
- No lock guard across an `.await` (bind headers first; see auth.rs).
- Keyring, notify-rust and ksni calls only on plain `std::thread`s.
- Any change to a decode/encode path needs a test in `wire.rs`.
- Inner loop: `npx tauri dev`; handoff: `npx tauri build --debug --no-bundle`
  → copy `src-tauri/target/debug/wattdrive-desktop` to `~/Downloads`.
- Release: push `v*` tag after CI is green on that SHA (release.yml).
