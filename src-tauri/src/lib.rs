//! WattDrive desktop — Tauri presentation layer and composition root.
//!
//! Wires the iCloud client, the SQLite state store and the sync loop into
//! Tauri commands, the window and the tray. No sync logic lives here.

pub mod commands;
#[cfg(target_os = "linux")]
pub mod linux_webkit;
mod notify;
mod paths;
mod settings;
mod status;
mod sync_runner;
#[cfg(target_os = "linux")]
mod tray_linux;
mod window_ops;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use tauri::{Emitter, Manager, WindowEvent};
use wattdrive_infrastructure::icloud::{Credentials, IcloudClient};
use wattdrive_infrastructure::session_store::SessionStore;
use wattdrive_infrastructure::state_db::SqliteStateStore;

use commands::AppState;
use settings::SettingsState;
pub(crate) use window_ops::{show_main, toggle_main};

/// Passed by the autostart entry so a login-launched instance stays in the tray.
const HIDDEN_FLAG: &str = "--hidden";
/// Set once the user closes the window to the tray, so the startup safety net
/// does not pop it back up.
pub(crate) static USER_HID_WINDOW: AtomicBool = AtomicBool::new(false);

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wattdrive=debug"));
    let path = paths::log_path();
    let _ = std::fs::create_dir_all(paths::data_dir());
    // Start fresh once the log grows past a few MB; one file, no rotation daemon.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 5 * 1024 * 1024) {
        let _ = std::fs::remove_file(&path);
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .init(),
        Err(_) => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("WattDrive: {msg}");
    tracing::error!("{msg}");
    std::process::exit(1)
}

pub fn run() {
    #[cfg(target_os = "linux")]
    linux_webkit::apply_session_quirks();
    init_logging();
    tracing::info!("WattDrive {} starting", env!("CARGO_PKG_VERSION"));

    let loaded = settings::load();
    let start_hidden = std::env::args().any(|arg| arg == HIDDEN_FLAG);

    // Keyring reads are blocking D-Bus calls; fine here, before the runtime.
    let saved = SessionStore::load_session().unwrap_or_else(|e| {
        tracing::warn!("could not read saved session: {e}");
        None
    });
    let creds = SessionStore::load_credentials()
        .unwrap_or_else(|e| {
            tracing::warn!("could not read saved credentials: {e}");
            None
        })
        .map(|c| Credentials {
            apple_id: c.apple_id,
            password: c.password,
        });
    let client = match IcloudClient::new(saved, creds) {
        Ok(c) => Arc::new(c),
        Err(e) => fail(&format!("cannot create HTTP client: {e}")),
    };
    client.set_session_hook(Arc::new(|session| {
        let session = session.clone();
        std::thread::spawn(move || {
            if let Err(e) = SessionStore::save_session(&session) {
                tracing::warn!("could not save session to keyring: {e}");
            }
        });
    }));
    let state_db = match SqliteStateStore::open(&paths::state_db_path()) {
        Ok(db) => Arc::new(db),
        Err(e) => fail(&format!("cannot open sync database: {e}")),
    };

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_FLAG]),
        ))
        .setup(move |app| {
            let handle = app.handle().clone();
            let progress_handle = handle.clone();
            client.set_progress_hook(Arc::new(move |msg| {
                let _ = progress_handle.emit("auth-progress", msg);
            }));
            let runner = sync_runner::start(
                handle.clone(),
                client.clone(),
                state_db.clone(),
                loaded.clone(),
            );
            app.manage(AppState {
                client: client.clone(),
                runner,
                settings: SettingsState(RwLock::new(loaded.clone())),
                state_db: state_db.clone(),
                start_hidden,
            });
            #[cfg(target_os = "linux")]
            tray_linux::spawn(handle.clone());
            // Safety net: if the frontend never reveals the window, show it —
            // unless this instance was autostarted into the tray.
            if !start_hidden {
                if let Some(window) = app.get_webview_window("main") {
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(3000));
                        if !USER_HID_WINDOW.load(Ordering::SeqCst) {
                            let _ = window.show();
                        }
                    });
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" {
                    return;
                }
                let close_to_tray = window
                    .app_handle()
                    .try_state::<AppState>()
                    .and_then(|s| s.settings.0.read().ok().map(|s| s.close_to_tray))
                    .unwrap_or(true);
                if close_to_tray {
                    USER_HID_WINDOW.store(true, Ordering::SeqCst);
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::sign_in,
            commands::submit_code,
            commands::request_sms,
            commands::submit_sms_code,
            commands::sign_out,
            commands::sync_now,
            commands::get_settings,
            commands::set_settings,
            commands::open_sync_folder,
            commands::open_trash_folder,
            commands::started_hidden,
            commands::app_info,
            commands::quit_app,
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        fail(&format!("error while running WattDrive: {e}"));
    }
}
