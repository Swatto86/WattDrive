//! WattDrive infrastructure — the adapters behind the domain and application
//! ports: Apple's iCloud web API (auth + Drive), the OS keyring for the saved
//! session and credentials, and SQLite for the per-path sync records.

pub mod icloud;
pub mod session_store;
pub mod state_db;
