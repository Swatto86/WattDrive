//! Errors a remote drive adapter can surface to the sync engine.

use thiserror::Error;

/// Failure talking to the cloud drive. The engine reacts to the variant, not
/// the text: `SignInRequired` pauses syncing and asks the user to sign in,
/// `RateLimited` backs off, the rest are logged against the affected path.
#[derive(Debug, Error)]
pub enum DriveError {
    /// The saved session is no longer accepted and could not be renewed
    /// silently (trust token expired, password changed, 2FA needed).
    #[error("sign-in required: {0}")]
    SignInRequired(String),
    /// Apple asked us to slow down (HTTP 429 / 503).
    #[error("rate limited by iCloud")]
    RateLimited,
    /// The network itself failed (DNS, TLS, timeout, connection reset).
    #[error("network error: {0}")]
    Network(String),
    /// iCloud answered, but with an error status or an unexpected body.
    #[error("iCloud API error ({status}): {message}")]
    Api { status: u16, message: String },
    /// A local file operation failed while transferring.
    #[error("local I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The remote item changed under us (etag mismatch) — replan next cycle.
    #[error("remote item changed concurrently")]
    Conflict,
    /// Anything else, with context.
    #[error("{0}")]
    Other(String),
}
