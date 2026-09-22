use std::fmt::Write as _;

use penv_schema::is_valid_key_name;
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
        "{key} contains all three quote marks (' \" and `), so a file of KEY=value lines has no way to wrap it."
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
/// value carrying a `"`, a `\` or a `$` goes in a quote that reads literally,
/// line breaks and all: `'`, or a backtick when the value holds a `'`.
fn quote(value: &str) -> Option<String> {
    if value.contains(['"', '\\', '$']) {
        let mark = ['\'', '`'].into_iter().find(|m| !value.contains(*m))?;
        return Some(format!("{mark}{value}{mark}"));
    }
    if value.contains('\n') {
        return Some(format!("\"{}\"", value.replace('\n', "\\n")));
    }
    if value.contains(['#', '\'', '`']) || value.chars().any(char::is_whitespace) {
        return Some(format!("\"{value}\""));
    }
    Some(value.to_string())
}

/// Set one key in an existing file, leaving every other line as written. A key
/// the file lacks is appended.
pub fn upsert(source: &str, key: &str, value: &str) -> Result<String, WriteError> {
    let line = write(&[(key, value)])?;
    let (mut lines, at) = split_at_key(source, key);
    match at {
        Some((start, span)) => {
            lines.splice(
                start..start + span,
                [line.trim_end_matches('\n').to_string()],
            );
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

/// Drop one key's lines. The file is returned unchanged when the key is absent.
pub fn remove(source: &str, key: &str) -> (String, bool) {
    let (mut lines, at) = split_at_key(source, key);
    match at {
        Some((start, span)) => {
            lines.drain(start..start + span);
            (join(lines), true)
        }
        None => (source.to_string(), false),
    }
}

/// The file's lines, and where the last assignment of `key` starts and how
/// many physical lines it covers.
fn split_at_key(source: &str, key: &str) -> (Vec<String>, Option<(usize, usize)>) {
    let lines: Vec<String> = source
        .strip_suffix('\n')
        .unwrap_or(source)
        .split('\n')
        .map(str::to_string)
        .filter(|_| !source.is_empty())
        .collect();
    let read = crate::read(source);
    let at = read.entries.iter().find(|e| e.key == key).map(|entry| {
        let spans = read
            .warnings
            .iter()
            .any(|w| w.line == entry.line && w.code == "multiline_value");
        let span = if spans {
            entry.value.matches('\n').count() + 1
        } else {
            1
        };
        (entry.line as usize - 1, span)
    });
    (lines, at)
}

fn join(lines: Vec<String>) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}
