//! The background loop: runs a sync pass on a timer, on local file changes
//! (debounced) and on demand; owns the current status and pushes it to the
//! window and the tray.

use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use wattdrive_application::{Progress, SyncEngine, SyncReport};
use wattdrive_domain::DriveError;
use wattdrive_infrastructure::icloud::{IcloudClient, IcloudDrive};
use wattdrive_infrastructure::state_db::SqliteStateStore;

use crate::settings::Settings;
use crate::status::{ProgressDto, ReportDto, Status, SyncState};

/// Wait this long after the last local change before syncing.
const LOCAL_DEBOUNCE: Duration = Duration::from_secs(2);
/// Ignore local change events for this long after a pass — they are our own
/// downloads landing.
const SELF_CHANGE_GRACE: Duration = Duration::from_secs(3);
const STARTUP_DELAY: Duration = Duration::from_secs(4);
pub const STATUS_EVENT: &str = "sync-status";

pub enum Command {
    SyncNow,
    Reconfigure(Settings),
    SignedIn,
    SignedOut,
}

#[derive(Clone)]
pub struct SyncRunner {
    tx: mpsc::UnboundedSender<Command>,
    status: Arc<RwLock<Status>>,
}

impl SyncRunner {
    pub fn status(&self) -> Status {
        self.status
            .read()
            .map(|s| s.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

struct Loop {
    app: AppHandle,
    client: Arc<IcloudClient>,
    state_db: Arc<SqliteStateStore>,
    settings: Settings,
    status: Arc<RwLock<Status>>,
    local_rx: mpsc::UnboundedReceiver<()>,
    local_tx: mpsc::UnboundedSender<()>,
    watcher: Option<notify::RecommendedWatcher>,
    last_pass_end: Option<Instant>,
}

/// Start the loop on Tauri's runtime.
pub fn start(
    app: AppHandle,
    client: Arc<IcloudClient>,
    state_db: Arc<SqliteStateStore>,
    settings: Settings,
) -> SyncRunner {
    let (tx, rx) = mpsc::unbounded_channel();
    let (local_tx, local_rx) = mpsc::unbounded_channel();
    let mut initial = Status::signed_out(settings.sync_folder.display().to_string());
    if client.has_credentials() {
        initial.signed_in = true;
        initial.apple_id = client.apple_id();
        initial.state = if settings.paused {
            SyncState::Paused
        } else {
            SyncState::Idle
        };
        initial.detail = if settings.paused {
            "Paused".into()
        } else {
            "Waiting to sync".into()
        };
    }
    let status = Arc::new(RwLock::new(initial));
    let runner = SyncRunner {
        tx,
        status: status.clone(),
    };
    let mut lp = Loop {
        app,
        client,
        state_db,
        settings,
        status,
        local_rx,
        local_tx,
        watcher: None,
        last_pass_end: None,
    };
    tauri::async_runtime::spawn(async move {
        lp.run(rx).await;
    });
    runner
}

impl Loop {
    fn signed_in(&self) -> bool {
        self.client.has_credentials()
    }

    fn set_status(&self, f: impl FnOnce(&mut Status)) {
        if let Ok(mut s) = self.status.write() {
            f(&mut s);
            let snapshot = s.clone();
            drop(s);
            let _ = self.app.emit(STATUS_EVENT, &snapshot);
            #[cfg(target_os = "linux")]
            crate::tray_linux::update(snapshot.state, snapshot.detail.clone());
        }
    }

    fn install_watcher(&mut self) {
        self.watcher = None;
        let folder = self.settings.sync_folder.clone();
        if let Err(e) = std::fs::create_dir_all(&folder) {
            tracing::warn!("cannot create sync folder {}: {e}", folder.display());
            return;
        }
        let tx = self.local_tx.clone();
        let root = folder.clone();
        let result = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                if event.paths.iter().any(|p| is_interesting(&root, p)) {
                    let _ = tx.send(());
                }
            }
        });
        match result {
            Ok(mut w) => match w.watch(&folder, RecursiveMode::Recursive) {
                Ok(()) => self.watcher = Some(w),
                Err(e) => tracing::warn!("cannot watch {}: {e}", folder.display()),
            },
            Err(e) => tracing::warn!("file watcher unavailable: {e}"),
        }
    }

    async fn run(&mut self, mut rx: mpsc::UnboundedReceiver<Command>) {
        self.install_watcher();
        let mut next_timer = Instant::now() + STARTUP_DELAY;
        let mut local_due: Option<Instant> = None;

        loop {
            let now = Instant::now();
            let timer_wait = next_timer.saturating_duration_since(now);
            let local_wait = local_due.map(|d| d.saturating_duration_since(now));
            let sleep_for = match local_wait {
                Some(l) => l.min(timer_wait),
                None => timer_wait,
            };

            let mut trigger: Option<&'static str> = None;
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    None => return,
                    Some(Command::SyncNow) => trigger = Some("manual"),
                    Some(Command::Reconfigure(s)) => {
                        let folder_changed = s.sync_folder != self.settings.sync_folder;
                        let was_paused = self.settings.paused;
                        self.settings = s;
                        if folder_changed {
                            self.install_watcher();
                            trigger = Some("folder changed");
                        }
                        let folder = self.settings.sync_folder.display().to_string();
                        let paused = self.settings.paused;
                        let signed_in = self.signed_in();
                        self.set_status(|st| {
                            st.sync_folder = folder;
                            if signed_in && paused {
                                st.state = SyncState::Paused;
                                st.detail = "Paused".into();
                            } else if signed_in && was_paused && !paused {
                                st.state = SyncState::Idle;
                                st.detail = "Resuming".into();
                            }
                        });
                        if was_paused && !paused {
                            trigger = Some("resumed");
                        }
                    }
                    Some(Command::SignedIn) => {
                        let id = self.client.apple_id();
                        self.set_status(|st| {
                            st.signed_in = true;
                            st.apple_id = id;
                            st.state = SyncState::Idle;
                            st.detail = "Signed in".into();
                        });
                        trigger = Some("signed in");
                    }
                    Some(Command::SignedOut) => {
                        let folder = self.settings.sync_folder.display().to_string();
                        self.set_status(|st| *st = Status::signed_out(folder));
                    }
                },
                _ = tokio::time::sleep(sleep_for) => {
                    if local_due.is_some_and(|d| Instant::now() >= d) {
                        local_due = None;
                        trigger = Some("local change");
                    } else {
                        trigger = Some("timer");
                    }
                },
                Some(()) = self.local_rx.recv() => {
                    let in_grace = self
                        .last_pass_end
                        .is_some_and(|t| t.elapsed() < SELF_CHANGE_GRACE);
                    if !in_grace {
                        local_due = Some(Instant::now() + LOCAL_DEBOUNCE);
                    }
                }
            }

            if let Some(why) = trigger {
                next_timer = Instant::now() + Duration::from_secs(self.settings.poll_interval_secs);
                if !self.signed_in() {
                    continue;
                }
                if self.settings.paused && why != "manual" {
                    continue;
                }
                if matches!(self.status().state, SyncState::SignInRequired) && why == "timer" {
                    continue;
                }
                tracing::info!("sync pass ({why})");
                self.pass().await;
                self.last_pass_end = Some(Instant::now());
                // Drain change events our own pass produced.
                while self.local_rx.try_recv().is_ok() {}
                local_due = None;
            }
        }
    }

    fn status(&self) -> Status {
        self.status
            .read()
            .map(|s| s.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    async fn pass(&mut self) {
        self.set_status(|st| {
            st.state = SyncState::Syncing;
            st.detail = "Checking iCloud…".into();
            st.progress = None;
        });

        if let Err(e) = self.client.ensure_ready().await {
            self.finish_with_error(e);
            return;
        }

        let drive = Arc::new(IcloudDrive::new(self.client.clone()));
        let engine = SyncEngine::new(
            self.settings.sync_folder.clone(),
            drive,
            self.state_db.clone(),
            crate::paths::host_name(),
        );
        let status = self.status.clone();
        let app = self.app.clone();
        let progress = move |p: Progress| {
            if let Ok(mut s) = status.write() {
                match p {
                    Progress::ScanningLocal => s.detail = "Scanning local folder…".into(),
                    Progress::ListingRemote => s.detail = "Listing iCloud Drive…".into(),
                    Progress::Executing {
                        done,
                        total,
                        current,
                    } => {
                        s.detail = if total == 0 {
                            "Up to date".into()
                        } else {
                            format!("Syncing {} of {total}…", done.min(total))
                        };
                        s.progress = Some(ProgressDto {
                            done,
                            total,
                            current,
                        });
                    }
                }
                let snapshot = s.clone();
                drop(s);
                let _ = app.emit(STATUS_EVENT, &snapshot);
                #[cfg(target_os = "linux")]
                crate::tray_linux::update(snapshot.state, snapshot.detail.clone());
            }
        };

        match engine.run_once(&progress).await {
            Ok(report) => self.finish_with_report(report).await,
            Err(e) => self.finish_with_error(e),
        }
    }

    async fn finish_with_report(&mut self, report: SyncReport) {
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        let _ = self.state_db.set_meta("last_sync", &now).await;
        let notify_on = self.settings.notifications_enabled;
        let dto = ReportDto::from(&report);
        let (state, detail) = match (&report.aborted, report.failures.is_empty()) {
            (Some(reason), _) => (SyncState::Offline, format!("Sync interrupted: {reason}")),
            (None, false) => (
                SyncState::Error,
                format!("{} item(s) could not sync", report.failures.len()),
            ),
            (None, true) => (SyncState::Idle, summary_line(&report)),
        };
        if notify_on && report.conflicts > 0 {
            crate::notify::notify(
                "WattDrive kept a conflict copy",
                &format!(
                    "{} file(s) were edited on both sides; both versions are in your iCloud Drive folder.",
                    report.conflicts
                ),
            );
        }
        self.set_status(|st| {
            st.state = state;
            st.detail = detail;
            st.last_sync = Some(now);
            st.last_report = Some(dto);
            st.progress = None;
        });
    }

    fn finish_with_error(&mut self, e: DriveError) {
        tracing::warn!("sync pass failed: {e}");
        let (state, detail) = match &e {
            DriveError::SignInRequired(msg) => (SyncState::SignInRequired, msg.clone()),
            DriveError::RateLimited => (SyncState::Offline, "iCloud asked us to slow down".into()),
            DriveError::Network(_) => (SyncState::Offline, "iCloud is unreachable".into()),
            other => (SyncState::Error, other.to_string()),
        };
        if state == SyncState::SignInRequired && self.settings.notifications_enabled {
            crate::notify::notify("WattDrive needs you to sign in again", &detail);
        }
        self.set_status(|st| {
            st.state = state;
            st.detail = detail;
            st.progress = None;
        });
    }
}

fn summary_line(r: &SyncReport) -> String {
    let moved = r.downloaded + r.uploaded + r.trashed_local + r.trashed_remote + r.conflicts;
    if moved == 0 {
        "Up to date".into()
    } else {
        let mut parts = Vec::new();
        if r.downloaded > 0 {
            parts.push(format!("{} down", r.downloaded));
        }
        if r.uploaded > 0 {
            parts.push(format!("{} up", r.uploaded));
        }
        if r.trashed_local + r.trashed_remote > 0 {
            parts.push(format!("{} removed", r.trashed_local + r.trashed_remote));
        }
        if r.conflicts > 0 {
            parts.push(format!("{} conflict(s)", r.conflicts));
        }
        format!("Synced: {}", parts.join(", "))
    }
}

/// Skip events for our own temp/trash files so a pass does not retrigger itself.
fn is_interesting(root: &Path, changed: &Path) -> bool {
    let Ok(rel) = changed.strip_prefix(root) else {
        return false;
    };
    !rel.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(wattdrive_application::ignore::is_ignored_name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn watcher_ignores_our_own_files() {
        let root = PathBuf::from("/home/u/iCloud Drive");
        assert!(is_interesting(&root, &root.join("Docs/a.txt")));
        assert!(!is_interesting(
            &root,
            &root.join(".wattdrive-trash/x/a.txt")
        ));
        assert!(!is_interesting(
            &root,
            &root.join("Docs/.wattdrive-part-abc")
        ));
        assert!(!is_interesting(&root, Path::new("/elsewhere/a.txt")));
    }

    #[test]
    fn summary_line_reads_naturally() {
        let mut r = SyncReport::default();
        assert_eq!(summary_line(&r), "Up to date");
        r.downloaded = 2;
        r.uploaded = 1;
        r.conflicts = 1;
        assert_eq!(summary_line(&r), "Synced: 2 down, 1 up, 1 conflict(s)");
    }
}
