//! Bounded file concurrency between serial prerequisite and deletion phases.
use crate::{
    executor::{ExecError, Executor},
    ActionFailure, Progress, SyncReport,
};
use futures_util::stream::{FuturesUnordered, StreamExt};
use wattdrive_domain::{DriveError, SyncAction};

const TRANSFERS: usize = 4;

fn is_file(action: &SyncAction) -> bool {
    matches!(
        action,
        SyncAction::Download { .. } | SyncAction::Upload { .. } | SyncAction::Adopt { .. }
    )
}

pub async fn execute_plan(
    exec: &mut Executor<'_>,
    actions: &[SyncAction],
    report: &mut SyncReport,
    progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<(), DriveError> {
    let mut next = 0;
    let mut done = 0;
    let mut sign_in = None;
    let total = actions.len();
    progress(Progress::Executing {
        done,
        total,
        current: String::new(),
    });
    while next < total && report.aborted.is_none() {
        if is_file(&actions[next]) {
            let end = next + actions[next..].iter().take_while(|a| is_file(a)).count();
            let mut pending = FuturesUnordered::new();
            let shared = &*exec;
            loop {
                while report.aborted.is_none() && next < end && pending.len() < TRANSFERS {
                    let action = &actions[next];
                    pending.push(async move { (action, shared.execute_file(action).await) });
                    next += 1;
                }
                let Some((action, result)) = pending.next().await else {
                    break;
                };
                record(report, action, result, &mut sign_in, progress);
                done += 1;
                progress(Progress::Executing {
                    done,
                    total,
                    current: action.path().to_string(),
                });
                // Never drop active uploads: finish and persist them even after a fatal error.
            }
        } else {
            let action = &actions[next];
            record(
                report,
                action,
                exec.execute(action).await,
                &mut sign_in,
                progress,
            );
            next += 1;
            done += 1;
            progress(Progress::Executing {
                done,
                total,
                current: action.path().to_string(),
            });
        }
    }
    if let Some(message) = sign_in {
        return Err(DriveError::SignInRequired(message));
    }
    Ok(())
}

fn record(
    report: &mut SyncReport,
    action: &SyncAction,
    result: Result<(), ExecError>,
    sign_in: &mut Option<String>,
    progress: &(dyn Fn(Progress) + Send + Sync),
) {
    match result {
        Ok(()) => tally(report, action),
        Err(e) => {
            // Folder/conflict prerequisites and state persistence failures stop dependent work.
            let aborts = e.aborts_pass()
                || matches!(e, ExecError::State(_))
                || matches!(
                    action,
                    SyncAction::MoveLocalAside { .. }
                        | SyncAction::CreateLocalFolder { .. }
                        | SyncAction::CreateRemoteFolder { .. }
                        | SyncAction::RecordFolder { .. }
                );
            tracing::warn!(action = action_name(action), "sync action failed");
            let failure = ActionFailure {
                path: action.path().to_string(),
                action: action_name(action).into(),
                error: e.to_string(),
            };
            progress(Progress::Failed(failure.clone()));
            report.failures.push(failure);
            if aborts && report.aborted.is_none() {
                report.aborted = Some(e.to_string());
            }
            if let ExecError::Drive(DriveError::SignInRequired(message)) = e {
                *sign_in = Some(message);
            }
        }
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
