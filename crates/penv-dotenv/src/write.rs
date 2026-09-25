use std::fmt::Write as _;

use penv_schema::is_valid_key_name;
use penv_schema::resolve::is_expression;
use thiserror::Error;

/// A pair that cannot be written in the safe subset. Each names the key and the fix.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WriteError {
    #[error(
        "{key} is not a valid key name. Use capitals, digits and underscores, such as DATABASE_URL."
    )]
    InvalidKey { key: String },
    #[error("{key} appears twice. Keep one value per key.")]
    DuplicateKey { key: String },
    #[error(
        "{key} holds a ' and also a \", a \\, a $ or a name(...) call, so only single quotes read it back as written and they cannot wrap it. Change the value, or set {key} in the process environment instead."
    )]
    Unquotable { key: String },
    #[error(
        "{key} holds a carriage return, or a line break together with a \" or a \\, which no .env reader decodes the same way. Encode the value, as base64 say, or set {key} in the process environment instead."
    )]
    Unportable { key: String },
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
        let quoted = quote(key, &value.replace("\r\n", "\n"))?;
        let _ = writeln!(out, "{key}={quoted}");
    }
    Ok(out)
}

/// [`write`], then one `# penv:redacted KEY` line for each key penv.cloud
/// holds and withheld. A comment, so no loader reads it as a value.
pub fn write_redacted(entries: &[(&str, &str)], redacted: &[&str]) -> Result<String, WriteError> {
    let mut out = write(entries)?;
    let mut seen: Vec<&str> = entries.iter().map(|(key, _)| *key).collect();
    for key in redacted {
        if !is_valid_key_name(key) || *key != key.to_ascii_uppercase() {
            return Err(WriteError::InvalidKey {
                key: key.to_string(),
            });
        }
        if seen.contains(key) {
            return Err(WriteError::DuplicateKey {
                key: key.to_string(),
            });
        }
        seen.push(key);
        let _ = writeln!(out, "{}{key}", crate::REDACTED_MARKER);
    }
    Ok(out)
}

/// Only `\n` inside double quotes is an escape every dialect reads back, so a
/// value carrying a `"`, a `\`, a `$` or a function call goes in single quotes,
/// where nothing is an escape or computed.
fn quote(key: &str, value: &str) -> Result<String, WriteError> {
    let breaks = value.contains('\n');
    if value.contains('\r') || (breaks && value.contains(['"', '\\'])) {
        return Err(WriteError::Unportable {
            key: key.to_string(),
        });
    }
    if value.contains(['"', '\\', '$']) || is_expression(value) {
        if value.contains('\'') {
            return Err(WriteError::Unquotable {
                key: key.to_string(),
            });
        }
        return Ok(format!("'{value}'"));
    }
    if breaks {
        return Ok(format!("\"{}\"", value.replace('\n', "\\n")));
    }
    if value.contains(['#', '\'', '`']) || value.chars().any(char::is_whitespace) {
        return Ok(format!("\"{value}\""));
    }
    Ok(value.to_string())
}

/// Set one key in an existing file, leaving every other line as written. A key
/// the file sets more than once keeps one line, where it was last set. A key the
/// file lacks is appended. A redacted marker for the key goes: the value replaces it.
pub fn upsert(source: &str, key: &str, value: &str) -> Result<String, WriteError> {
    let line = write(&[(key, value)])?;
    let (mut lines, spans) = split_at_key(source, key);
    match spans.split_last() {
        Some((&(start, end), earlier)) => {
            lines.splice(start..end, [line.trim_end_matches('\n').to_string()]);
            for &(start, end) in earlier.iter().rev() {
                lines.drain(start..end);
            }
        }
        None => {
            while lines.last().is_some_and(|l| l.is_empty()) {
                lines.pop();
            }
            lines.push(line.trim_end_matches('\n').to_string());
        }
    }
    Ok(join(lines))
}

/// Drop every line of one key, its redacted marker included. The file is
/// returned unchanged when the key is absent.
pub fn remove(source: &str, key: &str) -> (String, bool) {
    let (mut lines, spans) = split_at_key(source, key);
    if spans.is_empty() {
        return (source.to_string(), false);
    }
    for &(start, end) in spans.iter().rev() {
        lines.drain(start..end);
    }
    (join(lines), true)
}

/// The file's lines, and the line range of each assignment of `key`, in order.
fn split_at_key(source: &str, key: &str) -> (Vec<String>, Vec<(usize, usize)>) {
    let lines: Vec<String> = source
        .strip_suffix('\n')
        .unwrap_or(source)
        .split('\n')
        .map(str::to_string)
        .filter(|_| !source.is_empty())
        .collect();
    let every: Vec<(String, usize, usize)> = crate::read(source)
        .assignments
        .into_iter()
        .map(|(name, first, last)| (name, first as usize - 1, (last as usize).min(lines.len())))
        .collect();
    let mut spans: Vec<(usize, usize)> = every
        .iter()
        .filter(|(name, _, _)| name == key)
        .map(|&(_, start, end)| (start, end))
        .collect();
    // A line inside some value is part of that value, whatever it reads like.
    let inside = |i: usize| {
        every
            .iter()
            .any(|&(_, start, end)| (start..end).contains(&i))
    };
    let markers: Vec<(usize, usize)> = lines
        .iter()
        .enumerate()
        .filter(|(i, line)| crate::redacted_marker(line.trim()) == Some(key) && !inside(*i))
        .map(|(i, _)| (i, i + 1))
        .collect();
    spans.extend(markers);
    spans.sort_unstable();
    (lines, spans)
}

fn join(lines: Vec<String>) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}
