# WattDrive

Two-way iCloud Drive sync for Linux, built for Omarchy. Sign in with your Apple
Account once; `~/iCloud Drive` then mirrors your iCloud Drive both ways, like the
Mac client does.

Apple publishes no iCloud Drive API for Linux. WattDrive speaks to the same
private web endpoints icloud.com itself uses (the ones rclone and pyicloud rely
on), so Apple can change them without notice. When that happens WattDrive stops
until it is patched; it never guesses.

## Install (Omarchy / Arch)

Download `WattDrive_<version>_amd64.AppImage` from the latest release, make it
executable and run it. It registers a tray icon on Omarchy's bar, starts hidden
at login if you enable that in Settings, and updates itself in place.

## How syncing works

- Local changes sync within a couple of seconds (inotify). Remote changes are
  picked up on a timer (2 minutes by default) because Apple offers no push feed.
- A file edited on both sides keeps both versions: the local copy is renamed
  `name (conflict <host> <date>).ext` and iCloud's version takes the original
  name. Both end up in iCloud.
- Deletions never propagate over edits, and never hard-delete: files removed on
  iCloud move to `.wattdrive-trash/` inside the sync folder; files removed
  locally move to iCloud's Recently Deleted.
- Renames are seen as a delete plus an add (v1 limitation).

## Sign-in and security

Sign-in uses Apple's SRP handshake plus two-factor verification. The resulting
session, the 30-day trust token and the Apple ID + password (needed to renew
the session silently) are stored in your system keyring (Secret Service).
Nothing is written to disk in the clear. With Advanced Data Protection on,
enable "Access iCloud Data on the Web" on your iPhone and approve WattDrive
when prompted.

## Build

```bash
npm ci
npx tauri dev            # run locally with hot reload
bash scripts/verify.sh   # fmt, clippy, tests, version agreement
```

Releases are cut by pushing a `v*` tag; GitHub Actions builds and signs the
AppImage and publishes `latest.json` for the updater.
