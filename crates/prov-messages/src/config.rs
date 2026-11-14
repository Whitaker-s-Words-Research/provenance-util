//! The config file, `.provenance.toml`.

use std::fmt::Display;

pub fn invalid_required_version(file: impl Display, req: impl Display) -> String {
    format!("{file}: invalid required_version `{req}`")
}

pub fn invalid_known_gap(file: impl Display, gap: impl Display) -> String {
    format!("{file}: known_gaps: `{gap}` is not a commit ID of 7 or more hex characters")
}

pub fn invalid_version(version: impl Display) -> String {
    format!("invalid version `{version}`")
}

pub fn version_not_satisfied(version: impl Display, req: impl Display) -> String {
    format!("prov {version} does not satisfy required_version = \"{req}\"")
}

/// The context of an error in the `protected` patterns.
pub fn in_protected(file: impl Display) -> String {
    format!("{file}: protected")
}
