//! What an editor shows for `.penv/config.toml`: problems, completions and hover
//! text, from `config_vocabulary.json`. Pure over the text; positions are 0-based
//! UTF-16 units, as in `editor`.

use std::ops::Range as Bytes;
use std::sync::OnceLock;

use serde_json::Value;
use toml_edit::{Item, Key, TableLike};

use crate::editor::{Completion, Note, Position, Range, Severity, Tip, vocabulary};

const KIND_MODULE: u8 = 9;
const KIND_PROPERTY: u8 = 10;
const KIND_VALUE: u8 = 12;
const KIND_VARIABLE: u8 = 6;

/// One setting, or the shape of every entry of a section keyed by schema keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub name: String,
    /// `boolean`, `integer`, `string`, `list` or `table`.
    pub kind: String,
    pub default: Option<String>,
    /// `url-parts`, `schema-keys` or `schema-version`.
    pub values_from: Option<String>,
    pub doc: String,
    /// The message for a value of another type, naming the key and the fix.
    pub if_wrong: String,
    /// `check` fails on a mistake here; elsewhere penv ignores it.
    pub refused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub form: String,
    pub doc: String,
    /// `schema-key` or `name`: one sub-section per key or per name, as `[json.KEY]`.
    pub per: Option<String>,
    /// Any setting is accepted, as in `[targets.<name>]`.
    pub open: bool,
    /// `check` fails when this is not a table.
    pub refused: bool,
    pub keys: Vec<Setting>,
    /// Sections such as `[hosts]` whose keys are schema keys.
    pub entries: Option<Setting>,
}

impl Section {
    fn key(&self, name: &str) -> Option<&Setting> {
        self.entries
            .as_ref()
            .or_else(|| self.keys.iter().find(|k| k.name == name))
    }
}

/// Every section and setting `.penv/config.toml` takes.
pub fn config_vocabulary() -> &'static [Section] {
    static SECTIONS: OnceLock<Vec<Section>> = OnceLock::new();
    SECTIONS.get_or_init(|| {
        let root: Value = serde_json::from_str(include_str!("config_vocabulary.json"))
            .expect("config_vocabulary.json is valid JSON; a test reads it");
        root["sections"]
            .as_array()
            .into_iter()
            .flatten()
            .map(section)
            .collect()
    })
}

fn text(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

fn setting(v: &Value) -> Setting {
    Setting {
        name: text(v, "name"),
        kind: text(v, "type"),
        default: v["default"].as_str().map(str::to_string),
        values_from: v["values_from"].as_str().map(str::to_string),
        doc: text(v, "doc"),
        if_wrong: text(v, "if_wrong"),
        refused: v["refused"].as_bool().unwrap_or(false),
    }
}

fn section(v: &Value) -> Section {
    let name = text(v, "name");
    Section {
        form: v["form"]
            .as_str()
            .map_or_else(|| format!("[{name}]"), str::to_string),
        doc: text(v, "doc"),
        per: v["per"].as_str().map(str::to_string),
        open: v["open"].as_bool().unwrap_or(false),
        refused: v["refused"].as_bool().unwrap_or(false),
        keys: v["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .map(setting)
            .collect(),
        entries: v.get("entries").filter(|e| e.is_object()).map(setting),
        name,
    }
}

fn find_section(name: &str) -> Option<&'static Section> {
    config_vocabulary().iter().find(|s| s.name == name)
}

// ---------------------------------------------------------------- problems

/// What penv refuses or ignores in this text. `schema_keys` is what `.env.schema`
/// declares; `None` when it is unknown, and references go unchecked.
pub fn config_problems(text: &str, schema_keys: Option<&[String]>) -> Vec<Note> {
    let doc = match toml_edit::Document::parse(text) {
        Ok(doc) => doc,
        Err(e) => {
            let message = format!(
                "Not valid TOML: {}. penv refuses this file until you fix it.",
                e.message().trim_end_matches('.')
            );
            return vec![note(
                text,
                e.span(),
                Severity::Error,
                "invalid_config",
                message,
            )];
        }
    };
    let mut found = Found {
        text,
        schema_keys,
        notes: Vec::new(),
    };
    let root = doc.as_table();
    for (name, item) in root.iter() {
        let key = root.key(name);
        match find_section(name) {
            Some(section) => found.section(section, key, item),
            None => {
                let hint = nearest(name, config_vocabulary().iter().map(|s| s.name.as_str()))
                    .map(|n| format!("; did you mean [{n}]?"))
                    .unwrap_or_default();
                found.warn(
                    spot(key, item),
                    "unknown_setting",
                    format!("penv reads no section by this name and ignores it{hint}"),
                );
            }
        }
    }
    found.notes
}

struct Found<'a> {
    text: &'a str,
    schema_keys: Option<&'a [String]>,
    notes: Vec<Note>,
}

impl Found<'_> {
    fn push(
        &mut self,
        span: Option<Bytes<usize>>,
        severity: Severity,
        code: &str,
        message: String,
    ) {
        self.notes
            .push(note(self.text, span, severity, code, message));
    }

    fn warn(&mut self, span: Option<Bytes<usize>>, code: &str, message: String) {
        self.push(span, Severity::Warning, code, message);
    }

    fn declared(&self, name: &str) -> bool {
        self.schema_keys
            .is_none_or(|keys| keys.iter().any(|k| k == name))
    }

    fn known(&self, name: &str) -> bool {
        self.schema_keys
            .is_some_and(|keys| keys.iter().any(|k| k == name))
    }

    fn section(&mut self, section: &Section, key: Option<&Key>, item: &Item) {
        let Some(table) = item.as_table_like() else {
            let (severity, tail) = if section.refused {
                (Severity::Error, "")
            } else {
                (Severity::Warning, ", so penv ignores it")
            };
            let message = format!(
                "{} is not a section{tail}; write it as {} with its settings below",
                section.name, section.form
            );
            self.push(spot(key, item), severity, "wrong_type", message);
            return;
        };
        if section.per.is_none() {
            self.settings(section, table, &format!("[{}]", section.name));
            return;
        }
        for (index, (name, sub)) in table.iter().enumerate() {
            let sub_key = table.key(name);
            let per_key = section.per.as_deref() == Some("schema-key");
            if per_key && sub.is_table_like() && !self.declared(name) {
                self.push(
                    spot(sub_key, sub),
                    Severity::Error,
                    "unknown_key",
                    format!(
                        "This names no key in .env.schema; rename {} to a declared key or remove it",
                        section.form
                    ),
                );
            }
            let Some(inner) = sub.as_table_like() else {
                let severity = if section.refused {
                    Severity::Error
                } else {
                    Severity::Warning
                };
                let message = format!(
                    "{} takes one section per {}, such as {}",
                    section.name,
                    if section.per.as_deref() == Some("schema-key") {
                        "key"
                    } else {
                        "name"
                    },
                    section.form
                );
                self.push(spot(sub_key, sub), severity, "wrong_type", message);
                continue;
            };
            if section.open {
                continue;
            }
            // Names a key only when the schema declares it, so a pasted value is never echoed.
            let shown = if !per_key || self.known(name) {
                name.to_string()
            } else {
                format!("entry {}", index + 1)
            };
            let label = format!("[{}.{shown}]", section.name);
            for setting in section.keys.iter().filter(|s| s.refused) {
                if inner.get(&setting.name).is_none() {
                    self.push(
                        sub.span().or_else(|| sub_key.and_then(Key::span)),
                        Severity::Error,
                        "public_list",
                        format!(
                            "{label} {} is missing; write a list of read paths, such as [\"client_email\"]",
                            setting.name
                        ),
                    );
                }
            }
            self.settings(section, inner, &label);
        }
    }

    fn settings(&mut self, section: &Section, table: &dyn TableLike, label: &str) {
        if section.open {
            return;
        }
        for (name, item) in table.iter() {
            let key = table.key(name);
            match section.key(name) {
                Some(setting) => self.value(setting, key, item),
                None => {
                    let hint = nearest(name, section.keys.iter().map(|k| k.name.as_str()))
                        .map(|n| format!("; did you mean {n}?"))
                        .unwrap_or_default();
                    self.warn(
                        spot(key, item),
                        "unknown_setting",
                        format!(
                            "penv reads no setting by this name in {label} and ignores it{hint}"
                        ),
                    );
                }
            }
        }
    }

    fn value(&mut self, setting: &Setting, key: Option<&Key>, item: &Item) {
        let at = item.span().or_else(|| key.and_then(Key::span));
        let severity = if setting.refused {
            Severity::Error
        } else {
            Severity::Warning
        };
        let fits = match setting.kind.as_str() {
            "boolean" => item.is_bool(),
            "integer" => item.is_integer(),
            "string" => item.is_str(),
            "list" => item.is_array(),
            "table" => item.is_table_like(),
            _ => true,
        };
        if !fits {
            self.push(at, severity, "wrong_type", setting.if_wrong.clone());
            return;
        }
        if setting.values_from.as_deref() == Some("schema-version")
            && let Some(version) = item.as_integer()
        {
            let supported = i64::from(crate::SCHEMA_VERSION);
            if !(1..=supported).contains(&version) {
                self.push(
                    at,
                    Severity::Error,
                    "unsupported_schema",
                    format!(
                        "[schema] version = {version}, and this penv reads up to version {supported}; run penv upgrade"
                    ),
                );
            }
        }
        let Some(list) = item.as_array() else {
            return;
        };
        for entry in list.iter() {
            let span = entry.span();
            let Some(text) = entry.as_str() else {
                let message = if setting.refused {
                    "This entry is not a string; quote it"
                } else {
                    "penv skips this entry because it is not a string; quote it"
                };
                self.push(span, severity, "wrong_type", message.into());
                continue;
            };
            match setting.values_from.as_deref() {
                Some("url-parts") => {
                    let parts = &vocabulary().url_parts;
                    if parts.iter().any(|p| p.sensitive && p.names(text)) {
                        self.push(
                            span,
                            Severity::Error,
                            "public_list",
                            format!("{text} is always sensitive; remove it"),
                        );
                    } else if !parts.iter().any(|p| p.names(text)) {
                        let names: Vec<&str> = parts
                            .iter()
                            .filter(|p| !p.sensitive)
                            .map(|p| p.name.as_str())
                            .collect();
                        self.push(
                            span,
                            Severity::Error,
                            "public_list",
                            format!("This is not a URL part; use {}", names.join(", ")),
                        );
                    }
                }
                Some("schema-keys") if setting.refused && !self.declared(text) => self.push(
                    span,
                    Severity::Error,
                    "unknown_key",
                    "This names no key in .env.schema; list a key whose value is a hash() call, or remove it"
                        .into(),
                ),
                _ => {}
            }
        }
    }
}

/// Where to put a problem about an entry: its name, else its value.
fn spot(key: Option<&Key>, item: &Item) -> Option<Bytes<usize>> {
    key.and_then(Key::span).or_else(|| item.span())
}

fn note(
    text: &str,
    span: Option<Bytes<usize>>,
    severity: Severity,
    code: &str,
    message: String,
) -> Note {
    let span = span.unwrap_or(0..0);
    let start = position_of(text, span.start);
    let mut end = position_of(text, span.end.max(span.start));
    if end == start {
        // An empty span still marks one character, so the editor shows it.
        let next = text[span.start.min(text.len())..]
            .chars()
            .next()
            .filter(|c| *c != '\n')
            .map_or(0, char::len_utf16);
        end.character += next as u32;
    }
    Note {
        range: Range { start, end },
        severity,
        code: code.into(),
        message,
    }
}

/// The closest name within two edits, for a typo.
fn nearest<'a>(name: &str, names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    names
        .map(|n| (distance(name, n), n))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, n)| n)
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(here + 1);
            prev = here;
        }
    }
    row[b.len()]
}

// ---------------------------------------------------------------- positions

fn position_of(text: &str, byte: usize) -> Position {
    let byte = byte.min(text.len());
    let before = text.get(..byte).unwrap_or(text);
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    Position {
        line: before.matches('\n').count() as u32,
        character: before[line_start..]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>() as u32,
    }
}

fn line_chars(text: &str) -> Vec<Vec<char>> {
    text.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).chars().collect())
        .collect()
}

fn from_utf16(chars: &[char], units: u32) -> usize {
    let mut used = 0u32;
    for (i, c) in chars.iter().enumerate() {
        if used >= units {
            return i;
        }
        used += c.len_utf16() as u32;
    }
    chars.len()
}

fn to_utf16(chars: &[char], index: usize) -> u32 {
    chars[..index.min(chars.len())]
        .iter()
        .map(|c| c.len_utf16() as u32)
        .sum()
}

fn range(row: usize, chars: &[char], start: usize, end: usize) -> Range {
    let at = |i| Position {
        line: row as u32,
        character: to_utf16(chars, i),
    };
    Range {
        start: at(start),
        end: at(end),
    }
}

fn is_name(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// The name around `at`, as char indexes.
fn name_at(chars: &[char], at: usize, part: fn(char) -> bool) -> (usize, usize) {
    let mut start = at.min(chars.len());
    while start > 0 && part(chars[start - 1]) {
        start -= 1;
    }
    let mut end = at.min(chars.len());
    while end < chars.len() && part(chars[end]) {
        end += 1;
    }
    (start, end)
}

/// A header line's path: `[json."GCP_SA_JSON"]` is `json`, `GCP_SA_JSON`.
fn header_path(line: &[char]) -> Option<Vec<String>> {
    let s: String = line.iter().collect();
    let s = s.trim();
    let inner = s.strip_prefix('[')?;
    let inner = inner.strip_prefix('[').unwrap_or(inner);
    let inner = inner.split(']').next().unwrap_or(inner);
    Some(
        inner
            .split('.')
            .map(|p| p.trim().trim_matches(['"', '\'']).to_string())
            .collect(),
    )
}

/// The path of the section `row` sits in; empty above the first header.
fn section_at(lines: &[Vec<char>], row: usize) -> Vec<String> {
    (0..row)
        .rev()
        .find_map(|r| header_path(&lines[r]))
        .unwrap_or_default()
}

fn before_comment(line: &[char]) -> usize {
    let mut quoted = false;
    for (i, c) in line.iter().enumerate() {
        match c {
            '"' | '\'' => quoted = !quoted,
            '#' if !quoted => return i,
            _ => {}
        }
    }
    line.len()
}

/// The setting whose value the cursor is in, following a list across lines.
fn value_of(lines: &[Vec<char>], row: usize, c: usize) -> Option<String> {
    let key_of = |line: &[char], eq: usize| {
        let name: String = line[..eq].iter().collect();
        let name = name.trim().trim_matches(['"', '\'']);
        name.rsplit('.').next().unwrap_or(name).trim().to_string()
    };
    let here = &lines[row];
    if let Some(eq) = here[..c.min(here.len())].iter().position(|ch| *ch == '=') {
        return Some(key_of(here, eq));
    }
    let mut depth = 0i32;
    let mut count = |chars: &[char]| {
        for ch in chars {
            match ch {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
        }
    };
    count(&here[..c.min(here.len())]);
    for r in (0..row).rev() {
        let line = &lines[r][..before_comment(&lines[r])];
        if header_path(line).is_some() {
            return None;
        }
        match line.iter().position(|ch| *ch == '=') {
            Some(eq) => {
                count(&line[eq + 1..]);
                return (depth > 0).then(|| key_of(line, eq));
            }
            None => count(line),
        }
    }
    None
}

// ---------------------------------------------------------------- completion

fn item(
    label: &str,
    kind: u8,
    detail: &str,
    doc: &str,
    insert: String,
    snippet: bool,
    replace: Range,
) -> Completion {
    Completion {
        label: label.into(),
        kind,
        detail: detail.into(),
        doc: doc.into(),
        insert,
        snippet,
        replace,
    }
}

fn detail(setting: &Setting) -> String {
    match &setting.default {
        Some(d) => format!("{} · default {d}", setting.kind),
        None => setting.kind.clone(),
    }
}

/// What may go at `at`: a section in a header, a setting where a name goes, a
/// value after `=`, and declared keys where the setting takes one.
pub fn config_complete(text: &str, at: Position, schema_keys: &[String]) -> Vec<Completion> {
    let lines = line_chars(text);
    let row = at.line as usize;
    let Some(line) = lines.get(row) else {
        return Vec::new();
    };
    let c = from_utf16(line, at.character);
    let lead = line.iter().take_while(|ch| ch.is_whitespace()).count();
    if line.get(lead) == Some(&'[') && !line[..c].contains(&']') {
        return complete_header(line, row, c, schema_keys);
    }
    let path = section_at(&lines, row);
    let section = path.first().and_then(|p| find_section(p));
    if let Some(name) = value_of(&lines, row, c) {
        return section
            .and_then(|s| s.key(&name))
            .map(|s| complete_value(s, line, row, c, schema_keys))
            .unwrap_or_default();
    }
    let (start, end) = name_at(line, c, is_name);
    let replace = range(row, line, start, end);
    let Some(section) = section else {
        if !path.is_empty() {
            return Vec::new();
        }
        return config_vocabulary()
            .iter()
            .filter(|s| s.per.is_none())
            .map(|s| {
                item(
                    &s.form,
                    KIND_MODULE,
                    "section",
                    &s.doc,
                    s.form.clone(),
                    false,
                    replace,
                )
            })
            .collect();
    };
    if let Some(entries) = &section.entries {
        return schema_keys
            .iter()
            .map(|k| {
                item(
                    k,
                    KIND_VARIABLE,
                    &entries.kind,
                    &entries.doc,
                    format!("{k} = "),
                    false,
                    replace,
                )
            })
            .collect();
    }
    if section.per.is_some() && path.len() != 2 {
        return Vec::new();
    }
    let taken = taken(&lines, row);
    section
        .keys
        .iter()
        .filter(|k| k.kind != "table" && !taken.contains(&k.name))
        .map(|k| {
            let value = match k.kind.as_str() {
                "boolean" => "${1|true,false|}".to_string(),
                "list" => "[$1]".to_string(),
                "string" => "\"$1\"".to_string(),
                _ => format!("${{1:{}}}", k.default.as_deref().unwrap_or_default()),
            };
            item(
                &k.name,
                KIND_PROPERTY,
                &detail(k),
                &k.doc,
                format!("{} = {value}", k.name),
                true,
                replace,
            )
        })
        .collect()
}

/// Settings already written in the section `row` is in.
fn taken(lines: &[Vec<char>], row: usize) -> Vec<String> {
    let top = (0..row).rev().find(|r| header_path(&lines[*r]).is_some());
    let bottom = (row + 1..lines.len())
        .find(|r| header_path(&lines[*r]).is_some())
        .unwrap_or(lines.len());
    (top.map_or(0, |t| t + 1)..bottom)
        .filter(|r| *r != row)
        .filter_map(|r| {
            let line: String = lines[r].iter().collect();
            let (name, _) = line.split_once('=')?;
            Some(name.trim().to_string())
        })
        .collect()
}

fn complete_header(line: &[char], row: usize, c: usize, schema_keys: &[String]) -> Vec<Completion> {
    let (start, end) = name_at(line, c, |ch| {
        ch.is_alphanumeric() || matches!(ch, '_' | '-')
    });
    let replace = range(row, line, start, end);
    let close = if line[end..].contains(&']') { "" } else { "]" };
    let before: String = line[..start].iter().collect();
    let before = before.trim_start().trim_start_matches('[');
    if let Some(head) = before.strip_suffix('.') {
        let head = head.trim().trim_matches(['"', '\'']);
        let Some(section) = find_section(head) else {
            return Vec::new();
        };
        if section.per.as_deref() != Some("schema-key") {
            return Vec::new();
        }
        return schema_keys
            .iter()
            .map(|k| {
                item(
                    k,
                    KIND_VARIABLE,
                    &section.form,
                    &section.doc,
                    format!("{k}{close}"),
                    false,
                    replace,
                )
            })
            .collect();
    }
    if !before.trim().is_empty() {
        return Vec::new();
    }
    config_vocabulary()
        .iter()
        .map(|s| {
            let insert = if s.per.is_some() {
                format!("{}.", s.name)
            } else {
                format!("{}{close}", s.name)
            };
            item(
                &s.name,
                KIND_MODULE,
                &s.form,
                &s.doc,
                insert,
                false,
                replace,
            )
        })
        .collect()
}

fn complete_value(
    setting: &Setting,
    line: &[char],
    row: usize,
    c: usize,
    schema_keys: &[String],
) -> Vec<Completion> {
    let (start, end) = name_at(line, c, is_name);
    let replace = range(row, line, start, end);
    let quoted = line[..start].iter().filter(|ch| **ch == '"').count() % 2 == 1;
    let wrap = |s: &str| {
        if quoted {
            s.to_string()
        } else {
            format!("\"{s}\"")
        }
    };
    if setting.kind == "boolean" {
        return ["true", "false"]
            .iter()
            .map(|v| {
                item(
                    v,
                    KIND_VALUE,
                    "boolean",
                    &setting.doc,
                    v.to_string(),
                    false,
                    replace,
                )
            })
            .collect();
    }
    match setting.values_from.as_deref() {
        Some("url-parts") => vocabulary()
            .url_parts
            .iter()
            .filter(|p| !p.sensitive)
            .map(|p| match p.name.strip_suffix("NAME") {
                Some(prefix) => {
                    let insert = if quoted {
                        format!("{prefix}${{1:NAME}}")
                    } else {
                        format!("\"{prefix}${{1:NAME}}\"")
                    };
                    item(
                        &p.name, KIND_VALUE, "URL part", &p.doc, insert, true, replace,
                    )
                }
                None => item(
                    &p.name,
                    KIND_VALUE,
                    "URL part",
                    &p.doc,
                    wrap(&p.name),
                    false,
                    replace,
                ),
            })
            .collect(),
        Some("schema-keys") => schema_keys
            .iter()
            .map(|k| {
                item(
                    k,
                    KIND_VARIABLE,
                    "key in .env.schema",
                    &setting.doc,
                    wrap(k),
                    false,
                    replace,
                )
            })
            .collect(),
        Some("schema-version") => {
            let v = crate::SCHEMA_VERSION.to_string();
            vec![item(
                &v,
                KIND_VALUE,
                "schema version",
                &setting.doc,
                v.clone(),
                false,
                replace,
            )]
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------- hover

/// What the section or setting under `at` does.
pub fn config_hover(text: &str, at: Position) -> Option<Tip> {
    let lines = line_chars(text);
    let row = at.line as usize;
    let line = lines.get(row)?;
    let c = from_utf16(line, at.character);
    let (start, end) = name_at(line, c, |ch| {
        ch.is_alphanumeric() || matches!(ch, '_' | '-')
    });
    if start == end {
        return None;
    }
    let word: String = line[start..end].iter().collect();
    let tip = |markdown: String| Tip {
        range: range(row, line, start, end),
        markdown,
    };
    let section_tip = |s: &Section| tip(format!("`{}`\n\n{}", s.form, s.doc));
    if let Some(path) = header_path(line) {
        let section = find_section(path.first()?)?;
        let dots = line[..start].iter().filter(|ch| **ch == '.').count();
        if dots >= 2
            && let Some(setting) = section.keys.iter().find(|k| k.name == word)
        {
            return Some(setting_tip(section, setting, tip));
        }
        return Some(section_tip(section));
    }
    let eq = line.iter().position(|ch| *ch == '=')?;
    if start > eq {
        return None;
    }
    let path = section_at(&lines, row);
    let Some(first) = path.first() else {
        return find_section(&word).map(section_tip);
    };
    let section = find_section(first)?;
    if let Some(entries) = &section.entries {
        return Some(tip(format!(
            "`{}` · {}\n\n{}",
            section.form, entries.kind, entries.doc
        )));
    }
    let setting = section.keys.iter().find(|k| k.name == word)?;
    Some(setting_tip(section, setting, tip))
}

fn setting_tip(section: &Section, setting: &Setting, tip: impl Fn(String) -> Tip) -> Tip {
    tip(format!(
        "`{} {}` · {}\n\n{}",
        section.form,
        setting.name,
        detail(setting),
        setting.doc
    ))
}
