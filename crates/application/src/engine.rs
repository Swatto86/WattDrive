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
    PreparingRemote {
        done: usize,
        total: usize,
    },
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
        let mut tree = remote_tree::walk(self.drive.as_ref()).await?;
        let state = self
            .state
            .load_all()
            .await
            .map_err(|e| DriveError::Other(e.to_string()))?;

        // Files already eligible for adoption need no content request. Resolve
        // transfers before the final plan so a ZIP never replaces a bundle.
        let transfers = plan(PlanInput {
            remote: &tree.nodes,
            local: &local,
            state: &state,
        })
        .into_iter()
        .filter_map(|action| match action {
            SyncAction::Download { path, .. } => Some((path, false)),
            SyncAction::Upload {
                path,
                replaces: Some(_),
            } => Some((path, true)),
            _ => None,
        })
        .collect();
        let package_originals = remote_tree::prepare_files(
            self.drive.as_ref(),
            &mut tree,
            &state,
            &transfers,
            &|done, total| {
                progress(Progress::PreparingRemote { done, total });
            },
        )
        .await?;

        for (path, original) in &package_originals {
            if local.contains_key(original)
                || (cfg!(windows)
                    && local
                        .keys()
                        .any(|p| p.as_str().to_lowercase() == original.as_str().to_lowercase()))
            {
                return Err(DriveError::Other(format!(
                            "Local item {original} conflicts with an Apple bundle. Move the local item outside the sync folder; the bundle is mirrored as {path}."
                        )));
            }
        }

        #[cfg(windows)]
        crate::windows_paths::validate(local.keys().chain(tree.nodes.keys()).chain(state.keys()))?;

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

        crate::execution::execute_plan(&mut exec, &actions, &mut report, progress).await?;
        Ok(report)
    }
}
