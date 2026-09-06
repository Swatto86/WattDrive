//! One sync pass: scan, list, plan, execute, report.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use wattdrive_domain::{plan, DriveError, PlanInput, RemoteDrive, SyncAction};

use crate::executor::{Executor, TRASH_DIR_NAME};
use crate::state::StateStore;
use crate::{local, remote_tree};

/// Coarse progress for the tray / status screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    ScanningLocal,
    ListingRemote,
    Executing {
        done: usize,
        total: usize,
        current: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionFailure {
    pub path: String,
    pub action: String,
    pub error: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub planned: usize,
    pub downloaded: usize,
    pub uploaded: usize,
    pub trashed_local: usize,
    pub trashed_remote: usize,
    pub conflicts: usize,
    pub folders_created: usize,
    pub failures: Vec<ActionFailure>,
    /// Set when the pass stopped early (network / rate limit); the remaining
    /// actions are simply re-planned next time.
    pub aborted: Option<String>,
}

pub struct SyncEngine {
    root: PathBuf,
    drive: Arc<dyn RemoteDrive>,
    state: Arc<dyn StateStore>,
    host: String,
}

impl SyncEngine {
    pub fn new(
        root: PathBuf,
        drive: Arc<dyn RemoteDrive>,
        state: Arc<dyn StateStore>,
        host: String,
    ) -> Self {
        Self {
            root,
            drive,
            state,
            host,
        }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// Run one full pass. `Err` only for conditions that must pause syncing
    /// altogether (sign-in needed, the folder unreadable); per-item failures
    /// and a mid-pass network drop come back inside the report.
    pub async fn run_once(
        &self,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<SyncReport, DriveError> {
        tokio::fs::create_dir_all(&self.root).await?;

        progress(Progress::ScanningLocal);
        let local = local::scan(self.root.clone()).await?;
        progress(Progress::ListingRemote);
        let tree = remote_tree::walk(self.drive.as_ref()).await?;
        let state = self
            .state
            .load_all()
            .await
            .map_err(|e| DriveError::Other(e.to_string()))?;

        let actions = plan(PlanInput {
            remote: &tree.nodes,
            local: &local,
            state: &state,
        });
        let mut report = SyncReport {
            planned: actions.len(),
            ..SyncReport::default()
        };
        tracing::info!(
            "planned {} actions ({} local, {} remote, {} recorded)",
            actions.len(),
            local.len(),
            tree.nodes.len(),
            state.len()
        );

        let mut exec = Executor {
            root: &self.root,
            trash_batch: self
                .root
                .join(TRASH_DIR_NAME)
                .join(local::trash_batch_name(SystemTime::now())),
            drive: self.drive.as_ref(),
            state: self.state.as_ref(),
            folder_ids: tree.folder_ids,
            root_id: tree.root_id,
            host: &self.host,
        };

        let total = actions.len();
        for (i, action) in actions.iter().enumerate() {
            progress(Progress::Executing {
                done: i,
                total,
                current: action.path().to_string(),
            });
            tracing::debug!("{}", describe(action));
            match exec.execute(action).await {
                Ok(()) => tally(&mut report, action),
                Err(e) => {
                    tracing::warn!("{} failed: {e}", describe(action));
                    if let crate::executor::ExecError::Drive(DriveError::SignInRequired(m)) = e {
                        return Err(DriveError::SignInRequired(m));
                    }
                    // A failed prerequisite must never be followed by the
                    // download that would overwrite the unpreserved original.
                    let aborts = e.aborts_pass()
                        || matches!(action, SyncAction::MoveLocalAside { .. });
                    report.failures.push(ActionFailure {
                        path: action.path().to_string(),
                        action: action_name(action).to_string(),
                        error: e.to_string(),
                    });
                    if aborts {
                        report.aborted = Some(e.to_string());
                        break;
                    }
                }
            }
        }
        progress(Progress::Executing {
            done: total,
            total,
            current: String::new(),
        });
        Ok(report)
    }
}

fn action_name(a: &SyncAction) -> &'static str {
    match a {
        SyncAction::MoveLocalAside { .. } => "keep conflict copy",
        SyncAction::CreateLocalFolder { .. } => "create local folder",
        SyncAction::CreateRemoteFolder { .. } => "create iCloud folder",
        SyncAction::RecordFolder { .. } => "record folder",
        SyncAction::Download { .. } => "download",
        SyncAction::Upload { .. } => "upload",
        SyncAction::Adopt { .. } => "adopt",
        SyncAction::TrashLocal { .. } => "move to local trash",
        SyncAction::TrashRemote { .. } => "move to iCloud trash",
        SyncAction::Forget { .. } => "forget",
    }
}

fn describe(a: &SyncAction) -> String {
    format!("{} {}", action_name(a), a.path())
}

fn tally(report: &mut SyncReport, a: &SyncAction) {
    match a {
        SyncAction::MoveLocalAside { .. } => report.conflicts += 1,
        SyncAction::CreateLocalFolder { .. } | SyncAction::CreateRemoteFolder { .. } => {
            report.folders_created += 1
        }
        SyncAction::Download { .. } => report.downloaded += 1,
        SyncAction::Upload { .. } => report.uploaded += 1,
        SyncAction::TrashLocal { .. } => report.trashed_local += 1,
        SyncAction::TrashRemote { .. } => report.trashed_remote += 1,
        SyncAction::RecordFolder { .. } | SyncAction::Adopt { .. } | SyncAction::Forget { .. } => {}
    }
}
