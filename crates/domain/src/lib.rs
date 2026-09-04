//! WattDrive domain — the sync model and the pure planner.
//!
//! Nothing here performs I/O. The planner takes three snapshots (what iCloud
//! holds, what the local folder holds, what the last sync recorded) and returns
//! an ordered list of actions. That is the part of a sync tool that earns its
//! bugs, so it lives where it can be tested exhaustively with plain data.

mod error;
mod model;
mod plan;
mod ports;
mod rel_path;

#[cfg(test)]
mod plan_tests;

pub use error::DriveError;
pub use model::{ItemKind, LocalNode, RemoteChild, RemoteFile, RemoteId, RemoteNode, SyncEntry};
pub use plan::{plan, PlanInput, SyncAction};
pub use ports::RemoteDrive;
pub use rel_path::{RelPath, RelPathError};
