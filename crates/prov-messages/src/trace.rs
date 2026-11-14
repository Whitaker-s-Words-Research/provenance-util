//! `prov trace export`.

use std::fmt::Display;

pub const SCHEMA_DOES_NOT_PARSE: &str = "the pinned Agent Trace schema does not parse";
pub const GIVE_RANGE: &str = "give a range, for example main..HEAD";

pub fn invalid_record(sha: impl Display, error: impl Display) -> String {
    format!("the record of commit {sha} is not valid Agent Trace 0.1.0: {error}")
}
