//! Path patterns in the config file.

use std::fmt::Display;

pub fn empty(raw: impl Display) -> String {
    format!("invalid pattern `{raw}`: empty")
}

pub fn negation(raw: impl Display) -> String {
    format!("invalid pattern `{raw}`: negation is not supported")
}

pub fn invalid(raw: impl Display) -> String {
    format!("invalid pattern `{raw}`")
}

pub fn empty_component(raw: impl Display) -> String {
    format!("invalid pattern `{raw}`: empty path component")
}
