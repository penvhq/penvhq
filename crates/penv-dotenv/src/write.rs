use std::fmt::Write as _;

use penv_schema::is_valid_key_name;
use thiserror::Error;

/// A pair that cannot be written in the safe subset. Each names the key and the fix.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WriteError {
    #[error("{key} is not a usable key name. Use upper snake case, such as DATABASE_URL.")]
    InvalidKey { key: String },
    #[error("{key} is written twice. Keep one value per key.")]
    DuplicateKey { key: String },
    #[error("{key} contains a $. penv never expands values, so store the expanded value instead.")]
    Interpolation { key: String },
    #[error(
        "{key} mixes quotes, backslashes and line breaks in a way no .env dialect reads back. Keep it in the cloud and read it with penv run."
    )]
    Unquotable { key: String },
}

/// Write the safe subset only: UTF-8, LF, no comments, quotes only where needed.
pub fn write(entries: &[(&str, &str)]) -> Result<String, WriteError> {
    let mut out = String::new();
    let mut seen: Vec<&str> = Vec::new();
    for (key, value) in entries {
        let key = *key;
        if !is_valid_key_name(key) || key != key.to_ascii_uppercase() {
            return Err(WriteError::InvalidKey {
                key: key.to_string(),
            });
        }
        if seen.contains(&key) {
            return Err(WriteError::DuplicateKey {
                key: key.to_string(),
            });
        }
        seen.push(key);
        if value.contains('$') {
            return Err(WriteError::Interpolation {
                key: key.to_string(),
            });
        }
        // A trailing CR is line-ending residue; CRLF and a lone CR are written as
        // the LF they mean, because only `\n` survives every reader.
        let value = value
            .trim_end_matches('\r')
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let quoted = quote(&value).ok_or_else(|| WriteError::Unquotable {
            key: key.to_string(),
        })?;
        let _ = writeln!(out, "{key}={quoted}");
    }
    Ok(out)
}

/// Only `\n` inside double quotes is an escape every dialect reads back, so a
/// value carrying a `"` or a `\` goes in single quotes, where none are.
fn quote(value: &str) -> Option<String> {
    let breaks = value.contains('\n');
    let literal = value.contains(['"', '\\']);
    if breaks {
        if literal {
            return None;
        }
        return Some(format!("\"{}\"", value.replace('\n', "\\n")));
    }
    if literal {
        return (!value.contains('\'')).then(|| format!("'{value}'"));
    }
    if value.contains('#') || value.contains('\'') || value.chars().any(char::is_whitespace) {
        return Some(format!("\"{value}\""));
    }
    Some(value.to_string())
}
