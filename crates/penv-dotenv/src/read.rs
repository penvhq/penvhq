use penv_schema::{Values, is_valid_key_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub value: String,
    pub line: u32,
    /// The physical line the value ends on: `line` unless the value spans.
    pub end: u32,
    /// Single-quoted or backticked: read as written, never expanded.
    pub literal: bool,
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
    /// Every assignment as (key, first line, last line), repeats included.
    pub assignments: Vec<(String, u32, u32)>,
    /// Keys a `# penv:redacted KEY` line names, in file order, each once:
    /// present in penv.cloud and withheld from whoever pulled the file.
    pub redacted: Vec<String>,
}

impl Dotenv {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.key == key)
            .map(|e| e.value.as_str())
    }

    /// Each value with whether it may be computed, for `penv_schema::resolve`.
    pub fn raw(&self) -> std::collections::BTreeMap<String, penv_schema::resolve::Raw> {
        self.entries
            .iter()
            .map(|e| {
                let raw = if e.literal {
                    penv_schema::resolve::Raw::literal(e.value.clone())
                } else {
                    penv_schema::resolve::Raw::computed(e.value.clone())
                };
                (e.key.clone(), raw)
            })
            .collect()
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
        if let Some(key) = redacted_marker(trimmed)
            && !out.redacted.iter().any(|k| k == key)
        {
            out.redacted.push(key.to_string());
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Only the start is trimmed: a quote left open keeps its trailing spaces.
        let mut rest = line.trim_start();
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
        // The text before an = may be a line of some value, so it is named
        // only when it could not be.
        // A base64 block's short last line ends in `=` padding, so it reads as a
        // key with nothing after it.
        let padding = value_part.trim().is_empty() && looks_encoded(key);
        if !is_plausible_key(key) || padding {
            let message = if nameable(key) {
                format!("{key:?} is not a usable key name and was skipped")
            } else {
                format!("line {line_no}: the text before = is not a usable key name")
            };
            out.warn(line_no, "invalid_key_name", message);
            continue;
        }
        if name_part != key || value_part.starts_with(' ') || value_part.starts_with('\t') {
            out.warn(
                line_no,
                "spaces_around_equals",
                format!("{key} has whitespace around its ="),
            );
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
        let column = line.len() - first.len() + 1;
        let literal = first.starts_with(['\'', '`']);
        let value = if first.starts_with('#') && first.len() < value_part.len() {
            out.warn(
                line_no,
                "inline_comment",
                "a comment after a value is not part of the safe subset",
            );
            String::new()
        } else {
            read_value(first, &lines, &mut i, line_no, column, &mut out)
        };
        let end = i as u32;
        out.assignments.push((key.to_string(), line_no, end));

        match out.entries.iter_mut().find(|e| e.key == key) {
            Some(existing) => {
                existing.value = value;
                existing.literal = literal;
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
                end,
                literal,
            }),
        }
    }
    out
}

/// The comment `pull` writes for a key it could not read the value of.
pub const REDACTED_MARKER: &str = "# penv:redacted ";

/// The key a `# penv:redacted KEY` line names. Anything else is a plain comment.
pub fn redacted_marker(line: &str) -> Option<&str> {
    let key = line.strip_prefix(REDACTED_MARKER)?;
    is_plausible_key(key).then_some(key)
}

/// A name a `.env` line can set. A line of base64 is a valid identifier too, so
/// a long mixed-case run with digits and no underscore is not one; a short one
/// (`s3Bucket`, `oauth2ClientId`) is a name somebody typed.
pub fn is_plausible_key(name: &str) -> bool {
    is_valid_key_name(name) && !(name.len() >= 20 && looks_encoded(name))
}

/// Short and plain enough to be a name someone typed, never key material.
fn nameable(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 40
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        && !looks_encoded(text)
}

fn looks_encoded(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_uppercase())
        && text.chars().any(|c| c.is_ascii_lowercase())
        && text.chars().any(|c| c.is_ascii_digit())
        && !text.contains('_')
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
        if first.starts_with("-----BEGIN ") && !first.contains("-----END") {
            return read_pem(first, lines, i, line_no, out);
        }
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

/// An unquoted PEM block pasted across lines: every line through the `-----END`
/// one is the value, so no line of the key is read as a key of its own.
fn read_pem(first: &str, lines: &[&str], i: &mut usize, line_no: u32, out: &mut Dotenv) -> String {
    let mut value = first.trim_end().to_string();
    while *i < lines.len() {
        let line = lines[*i].trim();
        *i += 1;
        value.push('\n');
        value.push_str(line);
        if line.starts_with("-----END ") && line.ends_with("-----") {
            break;
        }
    }
    out.warn(
        line_no,
        "unquoted_multiline",
        format!(
            "the unquoted PEM block on lines {line_no} to {} was read as one value; double-quote it with \\n for each line break",
            *i
        ),
    );
    value
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
            } // Kept escaped: `\$` is how a computed value says a literal dollar.
            Some((_, '$')) => result.push_str("\\$"),
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
