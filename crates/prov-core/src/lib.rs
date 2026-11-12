//! Core types of `prov`: configuration, live state, the session state
//! machine, the file lock and the trailer format.

pub mod config;
pub mod pattern;
pub mod state;
pub mod trailer;

pub use config::Config;
pub use state::{SessionRecord, State, Store};

/// The version of this build, without the build SHA.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
