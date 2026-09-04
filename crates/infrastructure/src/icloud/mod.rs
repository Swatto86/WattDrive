//! iCloud web API client, ported from the endpoints icloud.com itself uses
//! (the same ones rclone's `iclouddrive` backend and pyicloud rely on). Apple
//! publishes no API for this: every path here is private and may change.
//!
//! Layout: [`srp`] is the password proof, [`session`] the cookies/tokens and
//! HTTP plumbing, [`auth`] the sign-in / 2FA / trust flow, [`drive`] the Drive
//! endpoints, and [`adapter`] the [`wattdrive_domain::RemoteDrive`] impl.

pub mod adapter;
pub mod auth;
pub mod drive;
pub mod session;
pub mod srp;
mod wire;

pub use adapter::IcloudDrive;
pub use auth::{Credentials, IcloudClient, SignInStep, TrustedPhone};
pub use session::SavedSession;
