use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::ir::{BaseType, Key, Schema};

/// The values a `.env` (or the cloud) supplies, by key.
pub type Values = BTreeMap<String, String>;

/// One key failing one rule. Never carries the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub key: String,
    pub rule: String,
    pub message: String,
}

impl Violation {
    pub fn new(key: &str, rule: &str, message: impl Into<String>) -> Violation {
        Violation {
            key: key.to_string(),
            rule: rule.to_string(),
            message: message.into(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "key": self.key, "rule": self.rule, "message": self.message })
    }
}

/// Check every key in the schema against the values supplied.
pub fn validate(schema: &Schema, values: &Values) -> Vec<Violation> {
    schema
        .keys
        .iter()
        .flat_map(|key| validate_key(key, values.get(&key.name).map(String::as_str)))
        .collect()
}

/// Values supplied that the schema does not declare. Drift, not a violation:
/// `run` masks them and `check` names them.
pub fn extras(schema: &Schema, values: &Values) -> Vec<String> {
    values
        .keys()
        .filter(|name| schema.get(name).is_none())
        .cloned()
        .collect()
}

/// Check one key against the value supplied for it, if any.
pub fn validate_key(key: &Key, value: Option<&str>) -> Vec<Violation> {
    let mut out = Vec::new();
    let present = value.filter(|v| !v.is_empty());
    let Some(value) = present else {
        // A literal default fills the key; a computed one that came out empty
        // does not. No value at all is a random() still pending.
        let defaulted = key.default.is_some() && (!key.default_expr || value.is_none());
        if key.required && !defaulted {
            out.push(Violation::new(
                &key.name,
                "required",
                format!(
                    "{} is required and has no value. Set it in .env, or give it a default in .env.schema.",
                    key.name
                ),
            ));
        }
        return out;
    };

    let name = key.name.as_str();
    let ty = &key.ty;
    match ty.base {
        BaseType::String => {}
        BaseType::Number => {
            if !matches!(value.parse::<f64>(), Ok(n) if n.is_finite()) {
                out.push(Violation::new(
                    name,
                    "type",
                    format!("{name} must be a number."),
                ));
                return out;
            }
        }
        BaseType::Boolean => {
            if parse_boolean(value).is_none() {
                out.push(Violation::new(
                    name,
                    "type",
                    format!("{name} must be true or false."),
                ));
                return out;
            }
        }
        BaseType::Url => {
            if !is_absolute_url(value) {
                out.push(Violation::new(
                    name,
                    "type",
                    format!("{name} must be an absolute URL, such as https://example.com."),
                ));
                return out;
            }
        }
        BaseType::Email => {
            if !is_email(value) {
                out.push(Violation::new(
                    name,
                    "type",
                    format!("{name} must be an email address."),
                ));
                return out;
            }
        }
        BaseType::Port => match value.parse::<u32>() {
            Ok(n) if (1..=65535).contains(&n) => {}
            _ => {
                out.push(Violation::new(
                    name,
                    "type",
                    format!("{name} must be a port between 1 and 65535."),
                ));
                return out;
            }
        },
        BaseType::Enum => {
            if !ty.members.iter().any(|m| m == value) {
                out.push(Violation::new(
                    name,
                    "enum",
                    format!("{name} must be one of {}.", ty.members.join(", ")),
                ));
                return out;
            }
        }
    }

    for (rule, arg) in &ty.constraints {
        match rule.as_str() {
            "startsWith" if !value.starts_with(arg.as_str()) => out.push(Violation::new(
                name,
                rule,
                format!("{name} must start with {arg}."),
            )),
            "endsWith" if !value.ends_with(arg.as_str()) => out.push(Violation::new(
                name,
                rule,
                format!("{name} must end with {arg}."),
            )),
            "minLength" => {
                if let Some(min) = arg
                    .parse::<usize>()
                    .ok()
                    .filter(|m| value.chars().count() < *m)
                {
                    out.push(Violation::new(
                        name,
                        rule,
                        format!("{name} must be at least {min} characters."),
                    ));
                }
            }
            "maxLength" => {
                if let Some(max) = arg
                    .parse::<usize>()
                    .ok()
                    .filter(|m| value.chars().count() > *m)
                {
                    out.push(Violation::new(
                        name,
                        rule,
                        format!("{name} must be at most {max} characters."),
                    ));
                }
            }
            "isInt" if arg == "true" && value.parse::<i64>().is_err() => out.push(Violation::new(
                name,
                rule,
                format!("{name} must be a whole number."),
            )),
            "min" => {
                if let (Ok(n), Ok(min)) = (value.parse::<f64>(), arg.parse::<f64>())
                    && n < min
                {
                    out.push(Violation::new(
                        name,
                        rule,
                        format!("{name} must be {min} or more."),
                    ));
                }
            }
            "max" => {
                if let (Ok(n), Ok(max)) = (value.parse::<f64>(), arg.parse::<f64>())
                    && n > max
                {
                    out.push(Violation::new(
                        name,
                        rule,
                        format!("{name} must be {max} or less."),
                    ));
                }
            }
            // matches and precision are parsed and preserved for the cloud and the
            // targets; penv carries no regex engine, so it enforces neither.
            _ => {}
        }
    }
    out
}

/// The boolean spellings penv accepts, both to validate and to infer, and the
/// ones every generated loader reads.
pub fn parse_boolean(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// An absolute URL: a scheme, `://`, and a host.
pub fn is_absolute_url(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    let scheme_ok = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !scheme_ok {
        return false;
    }
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    !authority.is_empty() && !authority.contains(char::is_whitespace)
}

pub fn is_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !value.contains(char::is_whitespace)
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain.contains('.')
        && !domain.contains('@')
}
