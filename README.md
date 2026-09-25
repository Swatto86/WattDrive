# WattDrive

Two-way iCloud Drive sync for Linux. Sign in with your Apple Account once; a
local folder (by default `~/iCloud Drive`) then mirrors your iCloud Drive both
ways, the way the Mac client does.

Apple publishes no iCloud Drive API for Linux. WattDrive speaks to the same
private web endpoints icloud.com itself uses, so Apple can change them without
notice. When that happens WattDrive stops until it is patched; it never guesses.

Built for Omarchy. The file you install is the same x86_64 AppImage on every
other Linux desktop.

## Requirements

- Linux on `x86_64`, with a desktop session and D-Bus. The published build is
  that AppImage.
- FUSE 2, so the AppImage can start. The package is `fuse2` on Arch and
  Omarchy, `libfuse2` on Debian and Ubuntu 22.04, and `libfuse2t64` on Ubuntu
  24.04. Where FUSE is missing, run the file with `--appimage-extract-and-run`.
- A Secret Service provider unlocked in the session: GNOME Keyring, or KWallet
  with its Secret Service interface. WattDrive keeps one encryption key there.
  Sign-in cannot be saved without it.
- A StatusNotifier watcher, if you want the tray icon. Omarchy's bar and KDE
  Plasma have one. On GNOME, an AppIndicator extension provides it. The window
  works either way.
- An Apple Account with [two-factor authentication](https://support.apple.com/en-us/102660)
  (Apple's usual setting). WattDrive asks for the account password, then the
  six-digit code.

## Install

Download `WattDrive_<version>_amd64.AppImage` and `SHA256SUMS.txt` from the
[latest release](https://github.com/Swatto86/WattDrive/releases/latest). Check
the AppImage, make it executable, and move it out of Downloads. Start at login
refuses a copy that still sits in Downloads, in a temporary directory, or in a
development build.

```bash
cd ~/Downloads
sha256sum -c SHA256SUMS.txt --ignore-missing
chmod +x WattDrive_*_amd64.AppImage
mkdir -p ~/.local/bin
mv WattDrive_*_amd64.AppImage ~/.local/bin/WattDrive.AppImage
~/.local/bin/WattDrive.AppImage
```

`~/.local/bin/WattDrive.AppImage` is an example. Any stable path outside
Downloads and temporary directories is fine.

A second launch opens the window of the copy that is already running. Closing
the window leaves that copy in the tray (the default). Quit is on the tray menu.

The app checks for a newer signed release on its own, downloads it, waits until
the current sync pass has finished, then replaces this AppImage and restarts.
About → Check for updates does the same when you ask. `latest.json` and the
`.AppImage.sig` on the release page are for that updater.

## First-run and sign-in

The window opens on **Sign in to iCloud**. A copy started at login stays in the
tray until you open it.

1. **Apple Account email.** The email address of your Apple Account, such as
   `you@example.com`.
2. **Password.** The password for that Apple Account.
3. **Verification code.** Apple sends a six-digit code to your trusted devices.
   Approve the sign-in there, then type the code. If no trusted device is to
   hand, choose **Text me** and enter the SMS code. Delivery of the code is
   covered in Apple's [two-factor authentication](https://support.apple.com/en-us/102660)
   article.

The Password field takes that Apple Account password. An
[app-specific password](https://support.apple.com/en-us/102654) is a different
credential: Apple issues one for apps that reach Mail, Contacts, or Calendar
and never complete two-factor. WattDrive completes two-factor itself, and an
app-specific password is rejected with "Incorrect Apple ID or password."

After a successful sign-in the email, password, session, and trust token are
written to `$XDG_DATA_HOME/WattDrive/secrets.bin`
(`~/.local/share/WattDrive/secrets.bin`). The file is AES-256-GCM, mode `0600`.
The keyring holds only the key for that file (`WattDrive` / `vault-key`).
Nothing is written in the clear. The password is kept because Apple's web
session needs it again when the session expires; the trust token (about 30
days) is what lets that renewal skip the six-digit code. When the token is
gone, the window asks for the code again. **Continue with saved password**
retries with what is already stored.

**Sign out** in Settings forgets the account and the sync database. The folder
on disk is left as it is.

With [Advanced Data Protection](https://support.apple.com/en-us/108756) on,
access to iCloud data on the web is off until you turn it back on. On your
iPhone, iPad, or Mac, enable **Access iCloud Data on the Web**, then approve
WattDrive when the device asks. The window says so while it waits.

The first pass creates the sync folder if needed and mirrors iCloud Drive into
it.

## Configuration

Open **Settings**.

- **Sync folder.** An absolute path. The default is `~/iCloud Drive`. It cannot
  be `/` or your home directory itself. Saving a new path starts a fresh mirror
  there. The previous folder stays on disk, untouched.
- **Check iCloud every.** 1 minute, 2 minutes (the default), 5 minutes, 15
  minutes, or 1 hour. This is the remote poll. Local changes sync within a
  couple of seconds.
- **Start at login.** Hidden, in the tray, via
  `~/.config/autostart/WattDrive.desktop`. Enable it from the installed
  AppImage (see Install).
- **Closing the window keeps WattDrive running in the tray.** On by default.
- **Desktop notifications** for conflicts and sign-in problems. On by default.
- **Theme.** Follow system, dark, or light.

**Pause** stops syncing until you resume, including across restarts. **Sync
now** runs a pass immediately. That includes while paused, and while the app is
waiting for you to sign in again.

### How syncing works

- A file edited on both sides keeps both versions: the local copy is renamed
  `name (conflict <hostname> <YYYY-MM-DD HHMM>).ext` and iCloud's version takes
  the original name. Both end up in iCloud. `<hostname>` is this machine's
  hostname.
- Deletions never propagate over edits, and never hard-delete. Files removed
  on iCloud move under `.wattdrive-trash/` inside the sync folder. Files
  removed locally move to iCloud's Recently Deleted.
- Renames are seen as a delete plus an add.
- Symlinks are not followed. Names WattDrive skips include `.DS_Store`,
  `Thumbs.db`, `desktop.ini`, editor lock and swap files, partial downloads
  (`.crdownload`, `.part`, `.tmp`), and anything starting with `.wattdrive`
  (its own trash and in-progress downloads).

## Troubleshooting

- **"Incorrect Apple ID or password."** Use the Apple Account email and the
  account password. An app-specific password is rejected here. See
  [First-run and sign-in](#first-run-and-sign-in).
- **The code never arrives.** Check every trusted device signed in to that
  Apple Account, or use **Text me**. Apple's steps are in
  [two-factor authentication](https://support.apple.com/en-us/102660).
- **"Approve iCloud web access"** or a timeout waiting for a trusted device.
  Advanced Data Protection is on. Turn on **Access iCloud Data on the Web** and
  approve the prompt. See Apple's
  [Advanced Data Protection](https://support.apple.com/en-us/108756) article.
- **"Could not reach Apple."** Retry when this machine can reach Apple.
- **"Apple asked us to slow down."** Wait a minute, then Sync now.
- **Sign-in will not stick, or a keyring error.** A Secret Service daemon has
  to be running and unlocked in this session (GNOME Keyring or KWallet).
- **Start at login is refused.** Move the AppImage out of Downloads and out of
  any temporary or `target/` directory, run that copy, and turn the setting on
  again.
- **No tray icon.** This desktop has no StatusNotifier watcher. Launch the
  AppImage again to raise the existing window.
- **The AppImage will not start.** Install FUSE 2 (see Requirements), or run
  `WattDrive.AppImage --appimage-extract-and-run`. An extracted copy does not
  get in-place updates.
- **Sync stopped after an Apple-side change.** The private endpoints moved.
  WattDrive will not invent a fallback. Install the next release, or build a
  patched version.
- **Log.** `$XDG_DATA_HOME/WattDrive/wattdrive.log`
  (`~/.local/share/WattDrive/wattdrive.log`). About shows the path this
  install is using. Settings live in `settings.json` beside that log; the
  sync database is `sync.db`.

## Build from source

Same platform as the release: Linux, `x86_64`. Rust comes from
`rust-toolchain.toml` (1.96.0; rustup selects it). Node.js 22 and npm are what
CI uses.

Debian or Ubuntu, the packages the release job installs, plus a C toolchain:

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libxdo-dev \
  libsoup-3.0-dev \
  libdbus-1-dev \
  patchelf \
  build-essential \
  curl \
  wget \
  file \
  libssl-dev
```

Arch or Omarchy:

```bash
sudo pacman -S --needed \
  webkit2gtk-4.1 \
  base-devel \
  curl \
  wget \
  file \
  openssl \
  appmenu-gtk-module \
  libappindicator-gtk3 \
  librsvg \
  xdotool \
  libsoup3 \
  patchelf
```

On another distribution, install the same pieces: WebKitGTK 4.1, GTK 3,
OpenSSL, D-Bus, and a C toolchain. Tauri's
[Linux prerequisites](https://v2.tauri.app/start/prerequisites/) list the
package names.

```bash
npm ci
npx tauri dev            # run locally with hot reload
bash scripts/verify.sh   # fmt, clippy, tests, version agreement
npx tauri build          # AppImage under src-tauri/target/release/bundle/appimage/
```

Releases are cut by pushing a `v*` tag. GitHub Actions builds and signs the
AppImage and publishes `latest.json` for the updater.
