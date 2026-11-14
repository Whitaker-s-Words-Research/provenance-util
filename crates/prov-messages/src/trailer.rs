//! Git trailers that prov writes.

use std::fmt::Display;

pub fn not_one_line(key: impl Display) -> String {
    format!("the value of a {key} trailer must be one line")
}

pub fn empty(key: impl Display) -> String {
    format!("the value of a {key} trailer is empty")
}
