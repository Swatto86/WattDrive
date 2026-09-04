//! The status snapshot the tray and the window render.

use serde::Serialize;
use wattdrive_application::SyncReport;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncState {
    /// Not signed in yet.
    SignedOut,
    Idle,
    Syncing,
    Paused,
    /// Session expired or 2FA needed; syncing is stopped until sign-in.
    SignInRequired,
    /// Last pass aborted (network, rate limit); will retry on the timer.
    Offline,
    /// Last pass finished with per-file failures.
    Error,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressDto {
    pub done: usize,
    pub total: usize,
    pub current: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureDto {
    pub path: String,
    pub action: String,
    pub error: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDto {
    pub planned: usize,
    pub downloaded: usize,
    pub uploaded: usize,
    pub trashed_local: usize,
    pub trashed_remote: usize,
    pub conflicts: usize,
    pub folders_created: usize,
    pub failures: Vec<FailureDto>,
    pub aborted: Option<String>,
}

impl From<&SyncReport> for ReportDto {
    fn from(r: &SyncReport) -> Self {
        Self {
            planned: r.planned,
            downloaded: r.downloaded,
            uploaded: r.uploaded,
            trashed_local: r.trashed_local,
            trashed_remote: r.trashed_remote,
            conflicts: r.conflicts,
            folders_created: r.folders_created,
            failures: r
                .failures
                .iter()
                .map(|f| FailureDto {
                    path: f.path.clone(),
                    action: f.action.clone(),
                    error: f.error.clone(),
                })
                .collect(),
            aborted: r.aborted.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: SyncState,
    /// One line for the tooltip / header ("Up to date", "Syncing 3 of 12…").
    pub detail: String,
    pub signed_in: bool,
    pub apple_id: Option<String>,
    pub sync_folder: String,
    /// RFC 3339, when the last pass finished.
    pub last_sync: Option<String>,
    pub last_report: Option<ReportDto>,
    pub progress: Option<ProgressDto>,
}

impl Status {
    pub fn signed_out(sync_folder: String) -> Self {
        Self {
            state: SyncState::SignedOut,
            detail: "Not signed in".into(),
            signed_in: false,
            apple_id: None,
            sync_folder,
            last_sync: None,
            last_report: None,
            progress: None,
        }
    }
}
