//! Linux system tray via ksni (StatusNotifierItem).
//!
//! Tauri's tray click events are unsupported on Linux, so WattDrive registers
//! its own StatusNotifierItem: a primary click (SNI `Activate`) toggles the
//! window, the menu offers sync/pause/open/quit, and the icon reflects the sync
//! state. All ksni blocking calls run on one dedicated `std::thread` — its
//! internal `block_on` aborts the process if called from a Tokio worker.

use ksni::blocking::TrayMethods;
use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, ToolTip, Tray};
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::thread;

use tauri::AppHandle;

use crate::status::SyncState;
use crate::{show_main, toggle_main};

enum TrayCmd {
    Update { state: SyncState, detail: String },
}

static CMD_TX: OnceLock<Sender<TrayCmd>> = OnceLock::new();

/// Decode a bundled 8-bit RGBA PNG into one ARGB32 ksni icon; `None` for any
/// unexpected format so a bad asset degrades to "no pixmap", not a panic.
fn decode_icon(bytes: &[u8]) -> Option<Icon> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let mut data = buf[..info.buffer_size()].to_vec();
    for px in data.chunks_exact_mut(4) {
        px.rotate_right(1); // RGBA → ARGB
    }
    Some(Icon {
        width: i32::try_from(info.width).ok()?,
        height: i32::try_from(info.height).ok()?,
        data,
    })
}

fn icon_for(state: SyncState) -> Vec<Icon> {
    let bytes: &[u8] = match state {
        SyncState::Idle => include_bytes!("../icons/tray-idle.png"),
        SyncState::Syncing => include_bytes!("../icons/tray-syncing.png"),
        SyncState::Paused | SyncState::SignedOut => include_bytes!("../icons/tray-paused.png"),
        SyncState::SignInRequired | SyncState::Offline => {
            include_bytes!("../icons/tray-attention.png")
        }
        SyncState::Error => include_bytes!("../icons/tray-error.png"),
    };
    decode_icon(bytes).into_iter().collect()
}

struct WattDriveTray {
    app: AppHandle,
    state: SyncState,
    detail: String,
}

impl Tray for WattDriveTray {
    fn id(&self) -> String {
        "co.swatto.wattdrive".into()
    }

    fn title(&self) -> String {
        "WattDrive".into()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        toggle_main(&self.app);
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        icon_for(self.state)
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: format!("WattDrive — {}", self.detail),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let pause_label = if self.state == SyncState::Paused {
            "Resume syncing"
        } else {
            "Pause syncing"
        };
        vec![
            StandardItem {
                label: "Open WattDrive".into(),
                activate: Box::new(|t: &mut Self| show_main(&t.app)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Sync now".into(),
                enabled: self.state != SyncState::SignedOut,
                activate: Box::new(|t: &mut Self| crate::commands::sync_now_from_tray(&t.app)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: pause_label.into(),
                enabled: self.state != SyncState::SignedOut,
                activate: Box::new(|t: &mut Self| crate::commands::toggle_pause(&t.app)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open iCloud Drive folder".into(),
                activate: Box::new(|t: &mut Self| crate::commands::open_folder_from_tray(&t.app)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut Self| t.app.exit(0)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Register the item on a dedicated thread. `assume_sni_available(true)` lets
/// an autostarted instance register before the bar's watcher is up.
pub fn spawn(app: AppHandle) {
    let (tx, rx) = mpsc::channel();
    if CMD_TX.set(tx).is_err() {
        return;
    }
    if let Err(e) = thread::Builder::new()
        .name("wattdrive-tray".into())
        .spawn(move || run_tray_thread(app, rx))
    {
        tracing::error!("failed to start Linux tray thread: {e}");
    }
}

fn run_tray_thread(app: AppHandle, rx: mpsc::Receiver<TrayCmd>) {
    let tray = WattDriveTray {
        app,
        state: SyncState::SignedOut,
        detail: "Starting".into(),
    };
    let handle = match tray.assume_sni_available(true).spawn() {
        Ok(handle) => handle,
        Err(e) => {
            tracing::error!("failed to register Linux tray: {e}");
            return;
        }
    };
    while let Ok(cmd) = rx.recv() {
        match cmd {
            TrayCmd::Update { state, detail } => {
                handle.update(move |t: &mut WattDriveTray| {
                    t.state = state;
                    t.detail = detail;
                });
            }
        }
    }
}

/// Refresh icon + tooltip. Safe from any thread; forwarded to the tray thread.
pub fn update(state: SyncState, detail: String) {
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.send(TrayCmd::Update { state, detail });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_has_a_decodable_tray_icon() {
        for state in [
            SyncState::SignedOut,
            SyncState::Idle,
            SyncState::Syncing,
            SyncState::Paused,
            SyncState::SignInRequired,
            SyncState::Offline,
            SyncState::Error,
        ] {
            let icons = icon_for(state);
            assert_eq!(icons.len(), 1, "{state:?} icon must decode as 8-bit RGBA");
            assert_eq!((icons[0].width, icons[0].height), (32, 32));
        }
    }

    #[test]
    fn update_before_spawn_is_a_noop() {
        update(SyncState::Idle, "x".into());
    }
}
