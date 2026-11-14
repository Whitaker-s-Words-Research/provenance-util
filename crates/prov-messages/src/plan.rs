//! Copy plans: the plan file of `prov cp --plan` and `--apply`.

use std::fmt::{Debug, Display};

pub fn binary_has_ranges(dest: impl Display) -> String {
    format!("{dest}: a binary file or a symbolic link has no ranges")
}

pub fn not_a_range(dest: impl Display, label: impl Display) -> String {
    format!("{dest}: `{label}` is not a range of lines")
}

pub fn line_in_two_ranges(dest: impl Display, line: impl Display) -> String {
    format!("{dest}: line {line} is in two ranges")
}

pub fn lines_without_range(dest: impl Display, first: impl Display, last: impl Display) -> String {
    format!("{dest}: no range has the lines {first}-{last}")
}

pub fn ranges_past_end(dest: impl Display, end: impl Display, line_count: impl Display) -> String {
    format!("{dest}: the ranges end at line {end}, but the file has {line_count} lines")
}

pub const DOES_NOT_PARSE: &str = "the plan does not parse";

pub fn lines_syntax(dest: impl Display, lines: impl Debug) -> String {
    format!("{dest}: `lines = {lines:?}` is not a range such as \"5-9\"")
}

pub fn ai_fields_without_ai(dest: impl Display, lines: impl Display) -> String {
    format!("{dest}: lines {lines}: `agent`, `model` and `session` go with author = \"ai\"")
}

pub fn duplicate_destination(dest: impl Display) -> String {
    format!("two files of the plan have the destination {dest}")
}
