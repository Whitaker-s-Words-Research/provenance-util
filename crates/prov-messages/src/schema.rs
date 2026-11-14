//! The JSON Schema validator of `prov trace export`. Each error starts with
//! the location in the record (`at`).

use std::fmt::{Debug, Display};

pub fn not_an_object(at: impl Display) -> String {
    format!("{at}: the schema is not an object")
}

pub fn cannot_resolve(at: impl Display, rule: impl Display) -> String {
    format!("{at}: cannot resolve $ref {rule}")
}

pub fn unsupported_type(at: impl Display, rule: impl Display) -> String {
    format!("{at}: unsupported type {rule}")
}

pub fn wrong_type(at: impl Display, rule: impl Display) -> String {
    format!("{at}: is not of type {rule}")
}

pub fn not_in_enum(at: impl Display, value: impl Display, rule: impl Display) -> String {
    format!("{at}: {value} is not one of {rule}")
}

pub fn pattern_not_string(at: impl Display) -> String {
    format!("{at}: pattern is not a string")
}

pub fn invalid_pattern(at: impl Display, pattern: impl Display, error: impl Display) -> String {
    format!("{at}: invalid pattern {pattern}: {error}")
}

pub fn no_match(at: impl Display, text: impl Debug, pattern: impl Display) -> String {
    format!("{at}: {text:?} does not match {pattern}")
}

pub fn unsupported_format(at: impl Display, rule: impl Display) -> String {
    format!("{at}: unsupported format {rule}")
}

pub fn invalid_format(at: impl Display, text: impl Debug, rule: impl Display) -> String {
    format!("{at}: {text:?} is not a valid {rule}")
}

pub fn below_minimum(at: impl Display, number: impl Display, minimum: impl Display) -> String {
    format!("{at}: {number} is less than {minimum}")
}

pub fn too_long(at: impl Display, max: impl Display) -> String {
    format!("{at}: longer than {max} characters")
}

pub fn missing_property(at: impl Display, name: impl Display) -> String {
    format!("{at}: no property {name}")
}

pub fn unknown_property(at: impl Display, name: impl Display) -> String {
    format!("{at}: unknown property {name}")
}

pub fn unsupported_keyword(at: impl Display, keyword: impl Display) -> String {
    format!("{at}: the schema uses the keyword `{keyword}`, which this validator does not support")
}
