//! What an editor shows for a `.env.schema`: problems, completions, hover text,
//! where a key is declared, and the outline. Pure over the text; the binary
//! speaks the Language Server Protocol. Positions are 0-based and count UTF-16
//! units, as the protocol does.

use std::sync::OnceLock;

use serde_json::{Value, json};

use crate::ir::{BaseType, Diagnostic, Schema};
use crate::parse::{is_divider, parse};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn to_json(&self) -> Value {
        json!({
            "start": { "line": self.start.line, "character": self.start.character },
            "end": { "line": self.end.line, "character": self.end.character },
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error = 1,
    Warning = 2,
    Information = 3,
}

/// One problem, placed on the text it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub range: Range,
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

impl Note {
    pub fn to_json(&self) -> Value {
        json!({
            "range": self.range.to_json(),
            "severity": self.severity as u8,
            "code": self.code,
            "source": "penv",
            "message": self.message,
        })
    }
}

/// Something the binary knows about a key that the text does not say, such as
/// a `@rotate` reminder. It is shown on the decorator named, else on the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyNote {
    pub key: String,
    pub decorator: Option<String>,
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    /// The protocol's CompletionItemKind.
    pub kind: u8,
    pub detail: String,
    pub doc: String,
    pub insert: String,
    pub snippet: bool,
    pub replace: Range,
}

impl Completion {
    pub fn to_json(&self) -> Value {
        json!({
            "label": self.label,
            "kind": self.kind,
            "detail": self.detail,
            "documentation": { "kind": "markdown", "value": self.doc },
            "insertTextFormat": if self.snippet { 2 } else { 1 },
            "textEdit": { "range": self.replace.to_json(), "newText": self.insert },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tip {
    pub range: Range,
    pub markdown: String,
}

impl Tip {
    pub fn to_json(&self) -> Value {
        json!({
            "contents": { "kind": "markdown", "value": self.markdown },
            "range": self.range.to_json(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub detail: String,
    pub range: Range,
    pub selection: Range,
}

impl Symbol {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "detail": self.detail,
            "kind": 13,
            "range": self.range.to_json(),
            "selectionRange": self.selection.to_json(),
        })
    }
}

// CompletionItemKind values.
const KIND_FUNCTION: u8 = 3;
const KIND_VARIABLE: u8 = 6;
const KIND_KEYWORD: u8 = 14;
const KIND_VALUE: u8 = 12;
const KIND_PROPERTY: u8 = 10;
const KIND_OPERATOR: u8 = 24;

// ---------------------------------------------------------------- vocabulary

/// One decorator, function, filter or type, as `vocabulary.json` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub name: String,
    /// `penv`, `env-spec` or `varlock`.
    pub origin: String,
    /// `header`, `item` or `both`; decorators only.
    pub scope: String,
    pub form: String,
    pub insert: String,
    pub doc: String,
    /// Offered as a completion. Words penv only reads past are not.
    pub complete: bool,
}

impl Word {
    fn origin_label(&self) -> &'static str {
        match self.origin.as_str() {
            "penv" => "penv extension",
            "varlock" => "varlock; penv reads past it",
            _ => "@env-spec",
        }
    }

    fn markdown(&self) -> String {
        format!("`{}` · {}\n\n{}", self.form, self.origin_label(), self.doc)
    }
}

#[derive(Debug)]
pub struct Vocabulary {
    pub decorators: Vec<Word>,
    pub functions: Vec<Word>,
    pub filters: Vec<Word>,
    pub types: Vec<Word>,
    pub rotate: Vec<String>,
}

/// Every word an editor offers or explains, from `vocabulary.json`.
pub fn vocabulary() -> &'static Vocabulary {
    static WORDS: OnceLock<Vocabulary> = OnceLock::new();
    WORDS.get_or_init(|| {
        let root: Value = serde_json::from_str(include_str!("vocabulary.json"))
            .expect("vocabulary.json is valid JSON; a test reads it");
        let words = |name: &str| -> Vec<Word> {
            root[name]
                .as_array()
                .map(|list| list.iter().map(word).collect())
                .unwrap_or_default()
        };
        Vocabulary {
            decorators: words("decorators"),
            functions: words("functions"),
            filters: words("filters"),
            types: words("types"),
            rotate: root["rotate"]
                .as_array()
                .map(|l| {
                    l.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        }
    })
}

fn word(v: &Value) -> Word {
    let text = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    Word {
        name: text("name"),
        origin: v["origin"].as_str().unwrap_or("env-spec").to_string(),
        scope: text("scope"),
        form: text("form"),
        insert: text("insert"),
        doc: text("doc"),
        complete: v["complete"].as_bool().unwrap_or(true),
    }
}

fn find<'a>(list: &'a [Word], name: &str) -> Option<&'a Word> {
    list.iter().find(|w| w.name == name)
}

// ---------------------------------------------------------------- text model

struct Line {
    chars: Vec<char>,
    kind: Kind,
    /// Chars before the parser's column 1: a byte-order mark on the first line.
    shift: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Blank,
    Divider,
    Comment,
    /// `name` is the key's char span; `eq` the index of the `=`.
    Key {
        start: usize,
        end: usize,
        eq: usize,
    },
    Other,
}

fn lines(text: &str) -> Vec<Line> {
    text.split('\n')
        .enumerate()
        .map(|(i, raw)| {
            let raw = raw.strip_suffix('\r').unwrap_or(raw);
            // The parser drops a byte-order mark; keep its char so positions line up.
            let body = if i == 0 {
                raw.strip_prefix('\u{feff}').unwrap_or(raw)
            } else {
                raw
            };
            let chars: Vec<char> = raw.chars().collect();
            let shift = chars.len() - body.chars().count();
            let trimmed = body.trim();
            let kind = if trimmed.is_empty() {
                Kind::Blank
            } else if is_divider(trimmed) {
                Kind::Divider
            } else if trimmed.starts_with('#') {
                Kind::Comment
            } else if let Some(eq) = chars.iter().position(|c| *c == '=') {
                let start = chars.iter().position(|c| !c.is_whitespace()).unwrap_or(0);
                let mut end = eq;
                while end > start && chars[end - 1].is_whitespace() {
                    end -= 1;
                }
                Kind::Key {
                    start: start.max(shift),
                    end,
                    eq,
                }
            } else {
                Kind::Other
            };
            Line { chars, kind, shift }
        })
        .collect()
}

fn key_name(line: &Line) -> Option<String> {
    match line.kind {
        Kind::Key { start, end, .. } => Some(line.chars[start..end].iter().collect()),
        _ => None,
    }
}

fn key_line(lines: &[Line], name: &str) -> Option<usize> {
    lines
        .iter()
        .position(|l| key_name(l).as_deref() == Some(name))
}

/// The comment lines directly above a key: its block.
fn block_of(lines: &[Line], key: usize) -> std::ops::Range<usize> {
    let mut start = key;
    while start > 0 && lines[start - 1].kind == Kind::Comment {
        start -= 1;
    }
    start..key
}

/// True for a line in the file's first run of comments, where header decorators live.
fn in_first_block(lines: &[Line], at: usize) -> bool {
    let first = lines
        .iter()
        .position(|l| l.kind != Kind::Blank)
        .unwrap_or(0);
    at >= first && lines[first..=at].iter().all(|l| l.kind == Kind::Comment)
}

fn to_utf16(chars: &[char], index: usize) -> u32 {
    chars[..index.min(chars.len())]
        .iter()
        .map(|c| c.len_utf16() as u32)
        .sum()
}

fn from_utf16(chars: &[char], units: u32) -> usize {
    let mut seen = 0u32;
    for (i, c) in chars.iter().enumerate() {
        if seen >= units {
            return i;
        }
        seen += c.len_utf16() as u32;
    }
    chars.len()
}

fn span(lines: &[Line], line: usize, start: usize, end: usize) -> Range {
    let chars = &lines[line].chars;
    Range {
        start: Position {
            line: line as u32,
            character: to_utf16(chars, start),
        },
        end: Position {
            line: line as u32,
            character: to_utf16(chars, end),
        },
    }
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The identifier around `at`: `[start, end)`.
fn word_at(chars: &[char], at: usize) -> (usize, usize) {
    let mut start = at.min(chars.len());
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    let mut end = at.min(chars.len());
    while end < chars.len() && is_word(chars[end]) {
        end += 1;
    }
    (start, end)
}

/// One decorator on a comment line, located by char index.
#[derive(Debug)]
struct Dec {
    at: usize,
    name_end: usize,
    /// `(start, end)` of what follows `=` or sits inside `( )`.
    value: Option<(usize, usize)>,
}

impl Dec {
    fn name(&self, chars: &[char]) -> String {
        chars[self.at + 1..self.name_end].iter().collect()
    }
}

/// The decorators on a comment line, read the way the parser reads them but
/// forgiving of text still being typed: an unclosed bracket runs to the end.
fn decorators(chars: &[char]) -> Vec<Dec> {
    let Some(hash) = chars.iter().position(|c| *c == '#') else {
        return Vec::new();
    };
    let body = hash + 1;
    let first = (body..chars.len()).find(|&i| !chars[i].is_whitespace());
    if first.is_none_or(|i| chars[i] != '@') {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = body;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if chars[i] != '@' {
            break;
        }
        let at = i;
        i += 1;
        while i < chars.len() && is_word(chars[i]) {
            i += 1;
        }
        let name_end = i;
        let mut value = None;
        if i < chars.len() && chars[i] == '(' {
            let open = i + 1;
            let mut depth = 0usize;
            let mut quoted: Option<char> = None;
            while i < chars.len() {
                let c = chars[i];
                match quoted {
                    Some(q) if c == q => quoted = None,
                    Some(_) => {}
                    None if c == '"' || c == '\'' => quoted = Some(c),
                    None if c == '(' => depth += 1,
                    None if c == ')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    None => {}
                }
                i += 1;
            }
            value = Some((open, i.min(chars.len())));
            i = (i + 1).min(chars.len());
        } else if i < chars.len() && chars[i] == '=' {
            i += 1;
            let start = i;
            let mut depth = 0usize;
            let mut quoted: Option<char> = None;
            while i < chars.len() {
                let c = chars[i];
                match quoted {
                    Some(q) if c == q => quoted = None,
                    Some(_) => {}
                    None if c == '"' || c == '\'' => quoted = Some(c),
                    None if c == '(' => depth += 1,
                    None if c == ')' => depth = depth.saturating_sub(1),
                    None if depth == 0 && c.is_whitespace() => break,
                    None => {}
                }
                i += 1;
            }
            value = Some((start, i));
        }
        out.push(Dec {
            at,
            name_end,
            value,
        });
    }
    out
}

// ---------------------------------------------------------------- diagnostics

/// What the parser alone finds in the text: its errors, else its warnings.
pub fn problems(text: &str) -> Vec<(Diagnostic, Severity)> {
    match parse(text) {
        Ok(schema) => schema
            .warnings
            .into_iter()
            .map(|d| (d, Severity::Warning))
            .collect(),
        Err(errors) => errors.into_iter().map(|d| (d, Severity::Error)).collect(),
    }
}

/// The parser's problems and the binary's notes, placed on the text.
pub fn diagnostics(text: &str, notes: &[KeyNote]) -> Vec<Note> {
    place(text, &problems(text), notes)
}

/// Problems found by any means, and notes, placed on the text they are about.
pub fn place(text: &str, found: &[(Diagnostic, Severity)], notes: &[KeyNote]) -> Vec<Note> {
    let lines = lines(text);
    let mut out: Vec<Note> = found
        .iter()
        .map(|(d, severity)| {
            let line = (d.line.max(1) as usize - 1).min(lines.len().saturating_sub(1));
            Note {
                range: problem_range(&lines, line, d.column),
                severity: *severity,
                code: d.code.clone(),
                message: without_line(&d.message),
            }
        })
        .collect();
    for note in notes {
        if let Some(range) = locate(&lines, &note.key, note.decorator.as_deref()) {
            out.push(Note {
                range,
                severity: note.severity,
                code: note.code.clone(),
                message: note.message.clone(),
            });
        }
    }
    out
}

/// The parser's messages open with `line N: ` for the terminal; the editor
/// already shows where.
fn without_line(message: &str) -> String {
    message
        .strip_prefix("line ")
        .and_then(|rest| rest.split_once(": "))
        .filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit()))
        .map_or_else(|| message.to_string(), |(_, rest)| rest.to_string())
}

/// From the reported column (1-based, in chars) to the end of that token, or
/// the whole line's text when the column is the line's start.
fn problem_range(lines: &[Line], line: usize, column: u32) -> Range {
    let chars = &lines[line].chars;
    let first = chars.iter().position(|c| !c.is_whitespace()).unwrap_or(0);
    let last = chars
        .iter()
        .rposition(|c| !c.is_whitespace())
        .map_or(0, |i| i + 1);
    let start = (column.max(1) as usize - 1 + lines[line].shift).min(chars.len());
    if start <= first {
        return span(lines, line, first, last.max(first));
    }
    let token = decorators(chars)
        .into_iter()
        .find(|d| d.at == start)
        .map(|d| {
            d.value
                .map_or(d.name_end, |(_, end)| (end + 1).min(chars.len()))
        });
    let end = token.unwrap_or_else(|| {
        (start..chars.len())
            .find(|&i| chars[i].is_whitespace())
            .unwrap_or(chars.len())
    });
    span(lines, line, start, end.max(start))
}

/// The `@decorator` in a key's block, else the key's name.
fn locate(lines: &[Line], key: &str, decorator: Option<&str>) -> Option<Range> {
    let at = key_line(lines, key)?;
    if let Some(name) = decorator {
        for l in block_of(lines, at).rev() {
            let chars = &lines[l].chars;
            if let Some(d) = decorators(chars)
                .into_iter()
                .find(|d| d.name(chars) == name)
            {
                let end = d.value.map_or(d.name_end, |(_, end)| end);
                return Some(span(lines, l, d.at, end));
            }
        }
    }
    let Kind::Key { start, end, .. } = lines[at].kind else {
        return None;
    };
    Some(span(lines, at, start, end))
}

// ---------------------------------------------------------------- completion

/// What fits at the cursor.
pub fn complete(text: &str, at: Position) -> Vec<Completion> {
    let lines = lines(text);
    let Some(line) = lines.get(at.line as usize) else {
        return Vec::new();
    };
    let row = at.line as usize;
    let c = from_utf16(&line.chars, at.character);
    let keys: Vec<String> = lines.iter().filter_map(key_name).collect();
    match line.kind {
        Kind::Comment => complete_comment(&lines, row, c, &keys),
        Kind::Key { eq, .. } if c > eq => complete_value(&lines, row, c, eq + 1, &keys),
        _ => Vec::new(),
    }
}

fn complete_comment(lines: &[Line], row: usize, c: usize, keys: &[String]) -> Vec<Completion> {
    let chars = &lines[row].chars;
    let header = in_first_block(lines, row);
    let decs = decorators(chars);

    if let Some(d) = decs.iter().find(|d| c > d.at && c <= d.name_end) {
        let replace = span(lines, row, d.at + 1, d.name_end);
        let mut items = decorator_items(header, replace, false);
        // Renaming a decorator that has a value keeps the value.
        if d.value.is_some() {
            for item in &mut items {
                item.insert = item.label[1..].to_string();
                item.snippet = false;
            }
        }
        return items;
    }
    if let Some((d, (start, end))) = decs
        .iter()
        .filter_map(|d| d.value.map(|v| (d, v)))
        .find(|(_, (start, end))| c >= *start && c <= *end)
    {
        return complete_decorator_value(lines, row, c, &d.name(chars), start, end, keys);
    }
    // A bare `#` line or a space after the last decorator: offer `@name`.
    let before: String = chars[..c].iter().collect();
    let body = before.trim_start().trim_start_matches('#');
    let fresh = c == 0 || chars[c - 1].is_whitespace() || chars[c - 1] == '#';
    if fresh && (body.trim().is_empty() || body.trim_start().starts_with('@')) {
        let replace = span(lines, row, c, c);
        return decorator_items(header, replace, true);
    }
    Vec::new()
}

fn decorator_items(header: bool, replace: Range, with_at: bool) -> Vec<Completion> {
    vocabulary()
        .decorators
        .iter()
        .filter(|w| w.complete && (header || w.scope != "header"))
        .map(|w| Completion {
            label: format!("@{}", w.name),
            kind: KIND_PROPERTY,
            detail: w.origin_label().to_string(),
            doc: w.markdown(),
            insert: if with_at {
                format!("@{}", w.insert)
            } else {
                w.insert.clone()
            },
            snippet: true,
            replace,
        })
        .collect()
}

fn plain(label: &str, kind: u8, detail: &str, doc: &str, replace: Range) -> Completion {
    Completion {
        label: label.to_string(),
        kind,
        detail: detail.to_string(),
        doc: doc.to_string(),
        insert: label.to_string(),
        snippet: false,
        replace,
    }
}

fn key_items(keys: &[String], prefix: &str, replace: Range) -> Vec<Completion> {
    keys.iter()
        .map(|k| {
            let label = format!("{prefix}{k}");
            plain(&label, KIND_VARIABLE, "key", "", replace)
        })
        .collect()
}

fn function_items(replace: Range) -> Vec<Completion> {
    vocabulary()
        .functions
        .iter()
        .map(|w| Completion {
            label: w.name.clone(),
            kind: KIND_FUNCTION,
            detail: w.form.clone(),
            doc: w.markdown(),
            insert: w.insert.clone(),
            snippet: true,
            replace,
        })
        .collect()
}

fn complete_decorator_value(
    lines: &[Line],
    row: usize,
    c: usize,
    name: &str,
    start: usize,
    end: usize,
    keys: &[String],
) -> Vec<Completion> {
    let chars = &lines[row].chars;
    let whole = span(lines, row, start, end);
    let typed: String = chars[start..c].iter().collect();
    let choices = |list: &[&str]| -> Vec<Completion> {
        list.iter()
            .map(|v| plain(v, KIND_VALUE, name, "", whole))
            .collect()
    };
    match name {
        "type" => match typed.find('(') {
            None => vocabulary()
                .types
                .iter()
                .map(|w| plain(&w.name, KIND_KEYWORD, "type", &w.doc, whole))
                .collect(),
            Some(open) => {
                let base = BaseType::from_name(typed[..open].trim());
                let arg_start = typed.rfind([',', '(']).map_or(0, |i| i + 1);
                let arg = &typed[arg_start..];
                if arg.contains('=') {
                    return Vec::new();
                }
                let (ws, we) = word_at(chars, c);
                let replace = span(lines, row, ws, we);
                base.map(|b| b.constraints())
                    .unwrap_or_default()
                    .iter()
                    .map(|k| Completion {
                        insert: format!("{k}="),
                        ..plain(k, KIND_PROPERTY, "constraint", "", replace)
                    })
                    .collect()
            }
        },
        "rotate" => {
            let list: Vec<&str> = vocabulary().rotate.iter().map(String::as_str).collect();
            choices(&list)
        }
        "required" | "optional" => {
            let mut out = choices(&["true", "false"]);
            out.push(Completion {
                insert: "forEnv(${1:production})".to_string(),
                snippet: true,
                ..plain("forEnv(...)", KIND_FUNCTION, name, "", whole)
            });
            out
        }
        "sensitive" | "defaultSensitive" => choices(&["true", "false"]),
        "defaultRequired" => choices(&["true", "false", "infer"]),
        "currentEnv" => key_items(keys, "$", whole),
        "assert" => {
            let (ws, we) = word_at(chars, c);
            if ws > 0 && chars[ws - 1] == '$' {
                key_items(keys, "", span(lines, row, ws, we))
            } else {
                function_items(span(lines, row, ws, we))
            }
        }
        _ => Vec::new(),
    }
}

fn complete_value(
    lines: &[Line],
    row: usize,
    c: usize,
    from: usize,
    keys: &[String],
) -> Vec<Completion> {
    let chars = &lines[row].chars;
    let before: String = chars[from..c].iter().collect();
    let (ws, we) = word_at(chars, c);
    let here = span(lines, row, ws, we);

    if let Some(open) = before.rfind("${")
        && !before[open..].contains('}')
    {
        let inner = &before[open + 2..];
        return if inner.contains('|') {
            vocabulary()
                .filters
                .iter()
                .map(|w| plain(&w.name, KIND_OPERATOR, &w.form, &w.doc, here))
                .collect()
        } else {
            key_items(keys, "", here)
        };
    }
    if ws > from && chars[ws - 1] == '$' {
        return key_items(keys, "", here);
    }
    if let Some(open) = before.rfind("penv(") {
        let inner = &before[open + 5..];
        if !inner.contains(')') {
            return if inner.contains('/') {
                key_items(keys, "", here)
            } else {
                Vec::new()
            };
        }
    }
    let lead = before[..before.len() - (c - ws).min(before.len())].trim_end();
    if lead.is_empty() || lead.ends_with(['(', ',']) {
        return function_items(here);
    }
    Vec::new()
}

// ---------------------------------------------------------------- hover

/// What the word under the cursor is.
pub fn hover(text: &str, at: Position, notes: &[KeyNote]) -> Option<Tip> {
    let lines = lines(text);
    let line = lines.get(at.line as usize)?;
    let row = at.line as usize;
    let c = from_utf16(&line.chars, at.character);
    let chars = &line.chars;

    if line.kind == Kind::Comment {
        for d in decorators(chars) {
            if c >= d.at && c <= d.name_end {
                let word = find(&vocabulary().decorators, &d.name(chars))?;
                return Some(Tip {
                    range: span(&lines, row, d.at, d.name_end),
                    markdown: word.markdown(),
                });
            }
            if let Some((start, end)) = d.value
                && c >= start
                && c <= end
            {
                return hover_expression(&lines, row, c, text, notes, d.name(chars) == "type");
            }
        }
        return None;
    }
    let Kind::Key { start, end, eq } = line.kind else {
        return None;
    };
    if c >= start && c <= end {
        let name: String = chars[start..end].iter().collect();
        return Some(Tip {
            range: span(&lines, row, start, end),
            markdown: summary(text, &name, notes),
        });
    }
    if c > eq {
        return hover_expression(&lines, row, c, text, notes, false);
    }
    None
}

fn hover_expression(
    lines: &[Line],
    row: usize,
    c: usize,
    text: &str,
    notes: &[KeyNote],
    type_value: bool,
) -> Option<Tip> {
    let chars = &lines[row].chars;
    let (ws, we) = word_at(chars, c);
    if ws == we {
        return None;
    }
    let name: String = chars[ws..we].iter().collect();
    let range = span(lines, row, ws, we);
    let words = vocabulary();
    if type_value && let Some(w) = find(&words.types, &name) {
        return Some(Tip {
            range,
            markdown: format!("`{}`\n\n{}", w.name, w.doc),
        });
    }
    if reference_at(chars, ws, we) && key_line(lines, &name).is_some() {
        return Some(Tip {
            range,
            markdown: summary(text, &name, notes),
        });
    }
    if chars.get(we) == Some(&'(')
        && let Some(w) = find(&words.functions, &name)
    {
        return Some(Tip {
            range,
            markdown: w.markdown(),
        });
    }
    let before: String = chars[..ws].iter().collect();
    if before.trim_end().ends_with('|')
        && let Some(w) = find(&words.filters, &name)
    {
        return Some(Tip {
            range,
            markdown: format!("`{}` · penv extension\n\n{}", w.form, w.doc),
        });
    }
    None
}

/// `$KEY`, `${KEY ...}` or `ref(KEY)`.
fn reference_at(chars: &[char], ws: usize, _we: usize) -> bool {
    if ws == 0 {
        return false;
    }
    let before: String = chars[..ws].iter().collect();
    let trimmed = before.trim_end_matches(['"', '\'', ' ']);
    chars[ws - 1] == '$' || before.ends_with("${") || trimmed.ends_with("ref(")
}

/// A key as the schema declares it, with the binary's notes.
fn summary(text: &str, name: &str, notes: &[KeyNote]) -> String {
    let schema: Option<Schema> = parse(text).ok();
    let mut out = format!("**{name}**");
    if let Some(key) = schema.as_ref().and_then(|s| s.get(name)) {
        out.push_str(&format!(": {}", code(&key.ty.to_string())));
        let mut facts = Vec::new();
        facts.push(if key.required { "required" } else { "optional" }.to_string());
        if key.sensitive {
            facts.push("sensitive".to_string());
        }
        if !key.hosts.is_empty() {
            let hosts: Vec<String> = key.hosts.iter().map(|h| code(h)).collect();
            facts.push(format!("sealed to {}", hosts.join(", ")));
        }
        if let Some(rotate) = &key.rotate {
            facts.push(format!("rotate every {}", code(rotate)));
        }
        if key.deprecated.is_some() {
            facts.push("deprecated".to_string());
        }
        out.push_str(&format!("\n\n{}", facts.join(" · ")));
        if let Some(description) = &key.description {
            out.push_str(&format!("\n\n{}", escape(description)));
        }
    }
    for note in notes.iter().filter(|n| n.key == name) {
        out.push_str(&format!("\n\n{}", note.message));
    }
    out
}

/// Schema text shown as markdown: every markdown character escaped, so a
/// comment cannot render a link, an image that loads from a server, or HTML.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A code span that holds any text, backticks included.
fn code(text: &str) -> String {
    let mut run = 0;
    let mut longest = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

// ---------------------------------------------------------------- definition and outline

/// Where the key referenced under the cursor is declared.
pub fn definition(text: &str, at: Position) -> Option<Range> {
    let lines = lines(text);
    let line = lines.get(at.line as usize)?;
    let c = from_utf16(&line.chars, at.character);
    let (ws, we) = word_at(&line.chars, c);
    if ws == we || !reference_at(&line.chars, ws, we) {
        return None;
    }
    let name: String = line.chars[ws..we].iter().collect();
    let target = key_line(&lines, &name)?;
    let Kind::Key { start, end, .. } = lines[target].kind else {
        return None;
    };
    Some(span(&lines, target, start, end))
}

/// The keys, each spanning its block, for the outline and breadcrumbs.
pub fn symbols(text: &str) -> Vec<Symbol> {
    let lines = lines(text);
    let schema = parse(text).ok();
    lines
        .iter()
        .enumerate()
        .filter_map(|(row, line)| {
            let Kind::Key { start, end, .. } = line.kind else {
                return None;
            };
            let name: String = line.chars[start..end].iter().collect();
            let first = block_of(&lines, row).start;
            let detail = schema
                .as_ref()
                .and_then(|s| s.get(&name))
                .map(|k| k.ty.to_string())
                .unwrap_or_default();
            Some(Symbol {
                range: Range {
                    start: Position {
                        line: first as u32,
                        character: 0,
                    },
                    end: Position {
                        line: row as u32,
                        character: to_utf16(&line.chars, line.chars.len()),
                    },
                },
                selection: span(&lines, row, start, end),
                name,
                detail,
            })
        })
        .collect()
}
