//! A validator for the part of JSON Schema that the pinned Agent Trace
//! schema uses. A keyword that it does not know is an error, so a change of
//! the pinned schema cannot pass without notice.

use regex_lite::Regex;
use prov_messages as msg;
use serde_json::Value;

/// Keywords that describe and do not constrain.
const ANNOTATIONS: [&str; 5] = ["description", "title", "$schema", "$id", "definitions"];

/// Checks `value` against the schema `root`. The error names the place.
pub fn validate(root: &Value, value: &Value) -> Result<(), String> {
    check(root, root, value, "$")
}

fn check(root: &Value, schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    let Some(schema) = schema.as_object() else { return Err(msg::schema::not_an_object(at)) };
    for (keyword, rule) in schema {
        match keyword.as_str() {
            k if ANNOTATIONS.contains(&k) => {}
            "$ref" => {
                let target = rule
                    .as_str()
                    .and_then(|r| r.strip_prefix('#'))
                    .and_then(|pointer| root.pointer(pointer))
                    .ok_or_else(|| msg::schema::cannot_resolve(at, rule))?;
                check(root, target, value, at)?;
            }
            "type" => {
                let ok = match rule.as_str() {
                    Some("object") => value.is_object(),
                    Some("array") => value.is_array(),
                    Some("string") => value.is_string(),
                    Some("integer") => value.is_i64() || value.is_u64(),
                    Some("number") => value.is_number(),
                    Some("boolean") => value.is_boolean(),
                    _ => return Err(msg::schema::unsupported_type(at, rule)),
                };
                if !ok {
                    return Err(msg::schema::wrong_type(at, rule));
                }
            }
            "enum" => {
                if !rule.as_array().is_some_and(|options| options.contains(value)) {
                    return Err(msg::schema::not_in_enum(at, value, rule));
                }
            }
            "pattern" => {
                let pattern = rule.as_str().ok_or_else(|| msg::schema::pattern_not_string(at))?;
                let regex = Regex::new(pattern).map_err(|e| msg::schema::invalid_pattern(at, pattern, e))?;
                if let Some(text) = value.as_str() {
                    if !regex.is_match(text) {
                        return Err(msg::schema::no_match(at, text, pattern));
                    }
                }
            }
            "format" => {
                if let Some(text) = value.as_str() {
                    let ok = match rule.as_str() {
                        Some("uuid") => is_uuid(text),
                        Some("date-time") => is_date_time(text),
                        Some("uri") => is_uri(text),
                        _ => return Err(msg::schema::unsupported_format(at, rule)),
                    };
                    if !ok {
                        return Err(msg::schema::invalid_format(at, text, rule));
                    }
                }
            }
            "minimum" => {
                if let (Some(number), Some(minimum)) = (value.as_f64(), rule.as_f64()) {
                    if number < minimum {
                        return Err(msg::schema::below_minimum(at, number, minimum));
                    }
                }
            }
            "maxLength" => {
                if let (Some(text), Some(max)) = (value.as_str(), rule.as_u64()) {
                    if text.chars().count() as u64 > max {
                        return Err(msg::schema::too_long(at, max));
                    }
                }
            }
            "required" => {
                if let Some(object) = value.as_object() {
                    for name in rule.as_array().into_iter().flatten().filter_map(Value::as_str) {
                        if !object.contains_key(name) {
                            return Err(msg::schema::missing_property(at, name));
                        }
                    }
                }
            }
            "properties" => {
                if let (Some(object), Some(properties)) = (value.as_object(), rule.as_object()) {
                    for (name, sub) in properties {
                        if let Some(v) = object.get(name) {
                            check(root, sub, v, &format!("{at}.{name}"))?;
                        }
                    }
                }
            }
            "additionalProperties" => {
                if let Some(object) = value.as_object() {
                    let known = schema.get("properties").and_then(Value::as_object);
                    for (name, v) in object {
                        if known.is_some_and(|k| k.contains_key(name)) {
                            continue;
                        }
                        match rule {
                            Value::Bool(false) => return Err(msg::schema::unknown_property(at, name)),
                            Value::Bool(true) => {}
                            sub => check(root, sub, v, &format!("{at}.{name}"))?,
                        }
                    }
                }
            }
            "items" => {
                for (i, v) in value.as_array().into_iter().flatten().enumerate() {
                    check(root, rule, v, &format!("{at}[{i}]"))?;
                }
            }
            other => return Err(msg::schema::unsupported_keyword(at, other)),
        }
    }
    Ok(())
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// RFC 3339: `YYYY-MM-DDThh:mm:ss[.fraction](Z|+hh:mm|-hh:mm)`.
fn is_date_time(text: &str) -> bool {
    let b = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| b.get(range).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    if !(digits(0..4) && b.get(4) == Some(&b'-') && digits(5..7) && b.get(7) == Some(&b'-') && digits(8..10)) {
        return false;
    }
    if !(matches!(b.get(10), Some(b'T' | b't')) && digits(11..13) && b.get(13) == Some(&b':') && digits(14..16)) {
        return false;
    }
    if !(b.get(16) == Some(&b':') && digits(17..19)) {
        return false;
    }
    let mut rest = &b[19..];
    if let Some((b'.', after)) = rest.split_first() {
        let n = after.iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return false;
        }
        rest = &after[n..];
    }
    match rest {
        [b'Z' | b'z'] => true,
        [b'+' | b'-', h1, h2, b':', m1, m2] => [h1, h2, m1, m2].iter().all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

fn is_uri(text: &str) -> bool {
    text.split_once(':').is_some_and(|(scheme, rest)| {
        !rest.is_empty()
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formats() {
        assert!(is_uuid("6f1c8e0a-5d3b-5a7e-9c21-0b8f4d2e7a15"));
        assert!(!is_uuid("6f1c8e0a5d3b5a7e9c210b8f4d2e7a15"));
        assert!(is_date_time("2026-11-11T10:00:00+01:00"));
        assert!(is_date_time("2026-11-11T10:00:00.25Z"));
        assert!(!is_date_time("2026-11-11 10:00:00"));
        assert!(!is_date_time("2026-11-11T10:00:00"));
        assert!(is_uri("https://example.invalid/x") && !is_uri("no scheme"));
    }

    #[test]
    fn keywords() {
        let schema = json!({
            "$ref": "#/definitions/T",
            "definitions": {"T": {
                "type": "object",
                "properties": {
                    "n": {"type": "integer", "minimum": 1},
                    "kind": {"type": "string", "enum": ["a", "b"]},
                    "list": {"type": "array", "items": {"type": "string", "maxLength": 3}},
                    "free": {"type": "object", "additionalProperties": {}},
                },
                "required": ["n"],
                "additionalProperties": false,
            }},
        });
        assert!(validate(&schema, &json!({"n": 1, "kind": "a", "list": ["abc"], "free": {"x": [1]}})).is_ok());
        assert!(validate(&schema, &json!({"kind": "a"})).unwrap_err().contains("no property n"));
        assert!(validate(&schema, &json!({"n": 0})).unwrap_err().contains("less than"));
        assert!(validate(&schema, &json!({"n": 1.5})).unwrap_err().contains("type"));
        assert!(validate(&schema, &json!({"n": 1, "kind": "c"})).unwrap_err().contains("not one of"));
        assert!(validate(&schema, &json!({"n": 1, "list": ["abcd"]})).unwrap_err().contains("$.list[0]"));
        assert!(validate(&schema, &json!({"n": 1, "other": 1})).unwrap_err().contains("unknown property other"));
        // A keyword that the validator does not know is an error, not a pass.
        assert!(validate(&json!({"oneOf": []}), &json!(1)).unwrap_err().contains("oneOf"));
    }
}
