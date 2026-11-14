//! Errors of file operations. Shared by all areas.

use std::fmt::Display;

pub fn cannot_read(path: impl Display) -> String {
    format!("cannot read {path}")
}

pub fn cannot_write(path: impl Display) -> String {
    format!("cannot write {path}")
}

pub fn cannot_create(path: impl Display) -> String {
    format!("cannot create {path}")
}

pub fn cannot_delete(path: impl Display) -> String {
    format!("cannot delete {path}")
}

pub fn cannot_open(path: impl Display) -> String {
    format!("cannot open {path}")
}

pub fn cannot_lock(path: impl Display) -> String {
    format!("cannot lock {path}")
}

pub fn does_not_parse(path: impl Display) -> String {
    format!("{path} does not parse")
}

pub fn does_not_exist(path: impl Display) -> String {
    format!("{path} does not exist")
}

pub const NO_CURRENT_DIR: &str = "no current directory";
