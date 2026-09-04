//! IPC surface for the window, plus the few actions the tray shares with it.
//! Every argument is untrusted frontend input: validated here, never trusted.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use wattdrive_application::StateStore;
use wattdrive_domain::DriveError;
use wattdrive_infrastructure::icloud::{Credentials, IcloudClient, SignInStep};
use wattdrive_infrastructure::session_store::{SessionStore, StoredCredentials};
use wattdrive_infrastructure::state_db::SqliteStateStore;

use crate::settings::{self, Settings, SettingsState};
use crate::status::Status;
use crate::sync_runner::{Command, SyncRunner};

pub struct AppState {
    pub client: Arc<IcloudClient>,
    pub runner: SyncRunner,
    pub settings: SettingsState,
    pub state_db: Arc<SqliteStateStore>,
    pub start_hidden: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneDto {
    pub id: i64,
    pub number: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "step")]
pub enum SignInResult {
    SignedIn,
    NeedsCode { phones: Vec<PhoneDto> },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub log_path: String,
    pub data_dir: String,
}

fn user_message(e: DriveError) -> String {
    match e {
        DriveError::SignInRequired(m) => m,
        DriveError::Network(_) => {
            "Could not reach Apple. Check your connection and try again.".into()
        }
        DriveError::RateLimited => {
            "Apple asked us to slow down. Wait a minute and try again.".into()
        }
        other => other.to_string(),
    }
}

/// Persist what a completed sign-in produced (credentials + session) and wake
/// the sync loop. Keyring calls are blocking D-Bus round-trips.
async fn persist_sign_in(state: &AppState) -> Result<(), String> {
    let creds = state
        .client
        .credentials()
        .ok_or_else(|| "sign-in finished without credentials".to_string())?;
    let session = state.client.saved_session();
    tokio::task::spawn_blocking(move || {
        SessionStore::save_credentials(&StoredCredentials {
            apple_id: creds.apple_id,
            password: creds.password,
        })?;
        SessionStore::save_session(&session)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("could not store the session in the keyring: {e}"))?;
    state.runner.send(Command::SignedIn);
    Ok(())
}

#[tauri::command]
pub fn get_status(state: State<'_, AppState>) -> Status {
    state.runner.status()
}

#[tauri::command]
pub async fn sign_in(
    state: State<'_, AppState>,
    apple_id: String,
    password: String,
) -> Result<SignInResult, String> {
    let apple_id = apple_id.trim().to_string();
    if apple_id.is_empty() || !apple_id.contains('@') {
        return Err("Enter the email address of your Apple Account.".into());
    }
    if password.is_empty() {
        return Err("Enter your password.".into());
    }
    let step = state
        .client
        .sign_in(Credentials { apple_id, password })
        .await
        .map_err(user_message)?;
    match step {
        SignInStep::SignedIn => {
            persist_sign_in(&state).await?;
            Ok(SignInResult::SignedIn)
        }
        SignInStep::NeedsTwoFactor { phones } => Ok(SignInResult::NeedsCode {
            phones: phones
                .into_iter()
                .map(|p| PhoneDto {
                    id: p.id,
                    number: p.number,
                })
                .collect(),
        }),
    }
}

fn clean_code(code: &str) -> Result<String, String> {
    let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 6 {
        return Err("The verification code is six digits.".into());
    }
    Ok(digits)
}

#[tauri::command]
pub async fn submit_code(state: State<'_, AppState>, code: String) -> Result<(), String> {
    let code = clean_code(&code)?;
    state
        .client
        .submit_code(&code)
        .await
        .map_err(user_message)?;
    persist_sign_in(&state).await
}

#[tauri::command]
pub async fn request_sms(state: State<'_, AppState>, phone_id: i64) -> Result<(), String> {
    state
        .client
        .request_sms(phone_id)
        .await
        .map_err(user_message)
}

#[tauri::command]
pub async fn submit_sms_code(
    state: State<'_, AppState>,
    code: String,
    phone_id: i64,
) -> Result<(), String> {
    let code = clean_code(&code)?;
    state
        .client
        .submit_sms_code(&code, phone_id)
        .await
        .map_err(user_message)?;
    persist_sign_in(&state).await
}

/// Forget the account: keyring entries, sync records, in-memory session. The
/// local folder is left untouched.
#[tauri::command]
pub async fn sign_out(state: State<'_, AppState>) -> Result<(), String> {
    state.client.sign_out();
    state.runner.send(Command::SignedOut);
    tokio::task::spawn_blocking(SessionStore::clear)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    state.state_db.clear().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sync_now(state: State<'_, AppState>) {
    state.runner.send(Command::SyncNow);
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    state
        .settings
        .0
        .read()
        .map(|s| s.clone())
        .map_err(|_| "settings lock poisoned".to_string())
}

#[tauri::command]
pub async fn set_settings(state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    apply_settings(&state, settings)
}

fn apply_settings(state: &AppState, settings: Settings) -> Result<(), String> {
    settings.validate()?;
    settings::save(&settings).map_err(|e| format!("could not save settings: {e}"))?;
    let mut guard = state
        .settings
        .0
        .write()
        .map_err(|_| "settings lock poisoned".to_string())?;
    *guard = settings.clone();
    drop(guard);
    state.runner.send(Command::Reconfigure(settings));
    Ok(())
}

fn open_path(app: &AppHandle, path: std::path::PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_sync_folder(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let folder = get_settings(state).await?.sync_folder;
    open_path(&app, folder)
}

#[tauri::command]
pub async fn open_trash_folder(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let folder = get_settings(state).await?.sync_folder;
    open_path(&app, folder.join(".wattdrive-trash"))
}

#[tauri::command]
pub fn started_hidden(state: State<'_, AppState>) -> bool {
    state.start_hidden
}

#[tauri::command]
pub fn app_info(app: AppHandle) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        log_path: crate::paths::log_path().display().to_string(),
        data_dir: crate::paths::data_dir().display().to_string(),
    }
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

// ---- shared with the tray menu ----

pub fn sync_now_from_tray(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.runner.send(Command::SyncNow);
    }
}

pub fn toggle_pause(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let current = match state.settings.0.read() {
        Ok(s) => s.clone(),
        Err(_) => return,
    };
    let toggled = Settings {
        paused: !current.paused,
        ..current
    };
    if let Err(e) = apply_settings(&state, toggled) {
        tracing::warn!("toggle pause failed: {e}");
    }
}

pub fn open_folder_from_tray(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let folder = match state.settings.0.read() {
        Ok(s) => s.sync_folder.clone(),
        Err(_) => return,
    };
    if let Err(e) = open_path(app, folder) {
        tracing::warn!("open folder failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_normalised_to_six_digits() {
        assert_eq!(clean_code("123 456").unwrap(), "123456");
        assert_eq!(clean_code(" 1-2-3-4-5-6 ").unwrap(), "123456");
        assert!(clean_code("12345").is_err());
        assert!(clean_code("abcdef").is_err());
    }

    #[test]
    fn user_messages_hide_transport_noise() {
        assert_eq!(user_message(DriveError::SignInRequired("x".into())), "x");
        assert!(user_message(DriveError::Network("tls".into())).contains("connection"));
        assert!(user_message(DriveError::RateLimited).contains("slow down"));
    }
}
