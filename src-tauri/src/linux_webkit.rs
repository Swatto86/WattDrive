//! Linux WebKitGTK + NVIDIA session quirks.
//!
//! On Omarchy/Hyprland with the proprietary NVIDIA driver, showing a window
//! (including tray Activate after `--hidden` autostart) triggers WebKit's
//! DMA-BUF renderer, which omits a Wayland acquire point; Hyprland answers with
//! protocol error 71 and kills the client. Apply before any GTK/WebKit init.

use webkit2gtk_nvidia_quirk::{apply_workaround_with_options, ApplyWorkaroundOptions};

/// Set the env vars WebKit/NVIDIA need for this session, if any.
pub fn apply_session_quirks() {
    apply_workaround_with_options(ApplyWorkaroundOptions::default());
}
