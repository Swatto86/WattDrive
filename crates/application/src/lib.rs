//! WattDrive application layer — one sync pass from end to end.
//!
//! [`SyncEngine::run_once`] scans the local folder, walks iCloud, asks the
//! domain planner what to do, and executes the actions through the
//! [`RemoteDrive`](wattdrive_domain::RemoteDrive) and [`StateStore`] ports.
//! Everything here is testable with a fake drive and a temp directory.

mod engine;
mod executor;
pub mod ignore;
pub mod local;
mod remote_tree;
mod state;

#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod fake_drive;
#[cfg(test)]
mod test_drives;

pub use engine::{ActionFailure, Progress, SyncEngine, SyncReport};
pub use state::{MemoryStateStore, StateError, StateStore};
