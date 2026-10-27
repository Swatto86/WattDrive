//! Native Windows tray, using the same commands and status as the main window.
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

use crate::status::SyncState;

pub fn spawn(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open WattDrive", true, None::<&str>)?;
    let sync = MenuItem::with_id(app, "sync", "Sync now", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause / resume syncing", true, None::<&str>)?;
    let folder = MenuItem::with_id(
        app,
        "folder",
        "Open iCloud Drive folder",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &sync, &pause, &folder, &quit])?;
    TrayIconBuilder::with_id("wattdrive")
        .icon(icon_for(SyncState::SignedOut)?)
        .tooltip("WattDrive")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => crate::show_main(app),
            "sync" => crate::commands::sync_now_from_tray(app),
            "pause" => crate::commands::toggle_pause(app),
            "folder" => crate::commands::open_folder_from_tray(app),
            "quit" => crate::commands::quit_app(app.clone()),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                crate::toggle_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn icon_for(state: SyncState) -> tauri::Result<Image<'static>> {
    let bytes: &[u8] = match state {
        SyncState::Idle => include_bytes!("../icons/tray-idle.png"),
        SyncState::Syncing => include_bytes!("../icons/tray-syncing.png"),
        SyncState::Paused | SyncState::SignedOut => include_bytes!("../icons/tray-paused.png"),
        SyncState::SignInRequired | SyncState::Offline => {
            include_bytes!("../icons/tray-attention.png")
        }
        SyncState::Error => include_bytes!("../icons/tray-error.png"),
    };
    Image::from_bytes(bytes)
}

pub fn update(app: &AppHandle, state: SyncState, detail: &str) {
    if let Some(tray) = app.tray_by_id("wattdrive") {
        let result = icon_for(state)
            .and_then(|icon| tray.set_icon(Some(icon)))
            .and_then(|()| tray.set_tooltip(Some(format!("WattDrive — {detail}"))));
        if let Err(e) = result {
            tracing::warn!("could not update Windows tray: {e}");
        }
    }
}
