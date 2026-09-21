use penv_schema::{Values, is_valid_key_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub value: String,
    pub line: u32,
}

/// A construct outside the safe subset. The file still parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub line: u32,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dotenv {
    pub entries: Vec<Entry>,
    pub warnings: Vec<Warning>,
}

impl Dotenv {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.key == key)
            .map(|e| e.value.as_str())
    }

    pub fn values(&self) -> Values {
        self.entries
            .iter()
            .map(|e| (e.key.clone(), e.value.clone()))
            .collect()
    }

    fn warn(&mut self, line: u32, code: &'static str, message: impl Into<String>) {
        self.warnings.push(Warning {
            line,
            code,
            message: message.into(),
        });
    }
}

/// Read a `.env`, tolerating the dialects real files use and reporting each one.
pub fn read(input: &str) -> Dotenv {
    let mut out = Dotenv::default();
    let body = match input.strip_prefix('\u{feff}') {
        Some(rest) => {
            out.warn(1, "bom", "the file starts with a byte order mark");
            rest
        }
        None => input,
    };
    if body.contains("\r\n") {
        out.warn(1, "crlf", "the file uses CRLF line endings");
    }

    let lines: Vec<&str> = body
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut i = 0usize;
    while i < lines.len() {
        let line_no = i as u32 + 1;
        let line = lines[i];
        i += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let mut rest = trimmed;
        if let Some(stripped) = strip_export(rest) {
            out.warn(
                line_no,
                "export_prefix",
                "an export prefix is not part of the safe subset",
            );
            rest = stripped;
        }

        let Some(eq) = rest.find('=') else {
            out.warn(
                line_no,
                "invalid_line",
                "this line has no = and was skipped",
            );
            continue;
        };
        let (name_part, value_part) = rest.split_at(eq);
        let value_part = &value_part[1..];
        let key = name_part.trim();
        if name_part != key || value_part.starts_with(' ') || value_part.starts_with('\t') {
            out.warn(
                line_no,
                "spaces_around_equals",
                format!("{key} has whitespace around its ="),
            );
        }
        if !is_valid_key_name(key) {
            out.warn(
                line_no,
                "invalid_key_name",
                format!("{key:?} is not a usable key name and was skipped"),
            );
            continue;
        }
        if key != key.to_ascii_uppercase() {
            out.warn(
                line_no,
                "lowercase_key",
                format!("{key} is not upper snake case"),
            );
        }

        let first = value_part.trim_start();
        // Where the value starts on this line, so an escape can be pointed at
        // without printing what it sits in.
        let column = line.trim_end().len() - first.len() + 1;
        let value = read_value(first, &lines, &mut i, line_no, column, &mut out);
        if value.contains('$') {
            out.warn(
                line_no,
                "interpolation",
                format!("{key} contains a $, which penv never expands"),
            );
        }

        match out.entries.iter_mut().find(|e| e.key == key) {
            Some(existing) => {
                existing.value = value;
                out.warnings.push(Warning {
                    line: line_no,
                    code: "duplicate_key",
                    message: format!("{key} is set more than once; the last value wins"),
                });
            }
            None => out.entries.push(Entry {
                key: key.to_string(),
                value,
                line: line_no,
            }),
        }
    }
    out
}

fn strip_export(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("export")?;
    if rest.starts_with([' ', '\t']) {
        Some(rest.trim_start())
    } else {
        None
    }
}

/// Read one value, following a quoted value across lines when it is left open.
fn read_value(
    first: &str,
    lines: &[&str],
    i: &mut usize,
    line_no: u32,
    column: usize,
    out: &mut Dotenv,
) -> String {
    let quote = first
        .chars()
        .next()
        .filter(|c| ['"', '\'', '`'].contains(c));
    let Some(quote) = quote else {
        let raw = strip_inline_comment(first, line_no, out);
        return raw.trim_end().to_string();
    };

    let body_at = column + quote.len_utf8();
    let mut body = first[quote.len_utf8()..].to_string();
    let mut spanned = false;
    loop {
        if let Some(end) = find_close(&body, quote) {
            let value = &body[..end];
            return if quote == '"' {
                unescape(value, line_no, body_at, out)
            } else {
                value.to_string()
            };
        }
        if *i >= lines.len() {
            out.warn(
                line_no,
                "unterminated_quote",
                "a quoted value is never closed; the rest of the file was read as its value",
            );
            return if quote == '"' {
                unescape(&body, line_no, body_at, out)
            } else {
                body
            };
        }
        if !spanned {
            spanned = true;
            out.warn(
                line_no,
                "multiline_value",
                "a value spans more than one line",
            );
        }
        body.push('\n');
        body.push_str(lines[*i]);
        *i += 1;
    }
}

fn find_close(body: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (idx, c) in body.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && quote == '"' {
            escaped = true;
            continue;
        }
        if c == quote {
            return Some(idx);
        }
    }
    None
}

fn strip_inline_comment<'a>(value: &'a str, line_no: u32, out: &mut Dotenv) -> &'a str {
    let bytes = value.as_bytes();
    for (idx, b) in bytes.iter().enumerate() {
        if *b == b'#' && idx > 0 && (bytes[idx - 1] == b' ' || bytes[idx - 1] == b'\t') {
            out.warn(
                line_no,
                "inline_comment",
                "a comment after a value is not part of the safe subset",
            );
            return &value[..idx];
        }
    }
    value
}

/// Only `\n` is the portable escape. Another still decodes, so a file some other
/// tool wrote reads, and is named by line and column, never by character.
fn unescape(value: &str, line_no: u32, at: usize, out: &mut Dotenv) -> String {
    if !value.contains('\\') {
        return value.to_string();
    }
    let mut result = String::with_capacity(value.len());
    let mut chars = value.char_indices();
    let mut warned = false;
    let mut warn = |out: &mut Dotenv, index: usize| {
        if !warned {
            warned = true;
            let (line, column) = position(value, index, line_no, at);
            out.warn(
                line,
                "escape_sequences",
                format!("the escape at column {column} is outside \\n"),
            );
        }
    };
    while let Some((index, c)) = chars.next() {
        if c != '\\' {
            result.push(c);
            continue;
        }
        match chars.next() {
            Some((_, 'n')) => result.push('\n'),
            Some((_, 'r')) => {
                warn(out, index);
                result.push('\r');
            }
            Some((_, 't')) => {
                warn(out, index);
                result.push('\t');
            }
            Some((_, quoted @ ('"' | '\\'))) => {
                warn(out, index);
                result.push(quoted);
            }
            Some((_, other)) => {
                warn(out, index);
                result.push(other);
            }
            None => {
                warn(out, index);
                result.push('\\');
            }
        }
    }
    result
}

/// Where an escape sits: the line it is on and its column there, so a value that
/// spans lines is not reported all at its first one.
fn position(body: &str, index: usize, line_no: u32, at: usize) -> (u32, usize) {
    let before = &body[..index];
    match before.rfind('\n') {
        Some(newline) => (
            line_no + before.matches('\n').count() as u32,
            index - newline,
        ),
        None => (line_no, at + index),
    }
}
