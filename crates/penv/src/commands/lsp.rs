//! `penv lsp`: a Language Server Protocol server on stdin and stdout for
//! `.env.schema`. What it says comes from `penv_schema::editor`; this file only
//! speaks the protocol and reads `.penv/config.toml` for `@rotate` clocks. It
//! reads no value file and sends no request.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use penv_schema::Diagnostic;
use penv_schema::editor::{self, KeyNote, Position, Severity};
use penv_schema::rotate::{Rotation, Span, parse_instant, rotation};
use serde_json::{Value, json};

use crate::error::CliError;
use crate::output::Report;

/// A message larger than this is refused rather than read into memory.
const MAX_MESSAGE: usize = 32 * 1024 * 1024;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const SERVER_NOT_INITIALIZED: i64 = -32002;

pub fn run() -> Result<Report, CliError> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let code = serve(&mut BufReader::new(stdin.lock()), &mut stdout.lock(), now);
    if code == 0 {
        Ok(Report::silent())
    } else {
        Err(CliError::new(
            "lsp_ended",
            "the editor closed the connection without asking the server to shut down",
            "Restart the language server from the editor.",
        ))
    }
}

fn now() -> u64 {
    use penv_cloud::Clock as _;
    penv_cloud::SystemClock.now()
}

/// Serves until `exit`, and returns the exit code the protocol asks for: 0 when
/// `shutdown` came first, 1 otherwise, including when the input ends.
pub fn serve(input: &mut impl BufRead, output: &mut impl Write, clock: fn() -> u64) -> i32 {
    let mut server = Server {
        documents: BTreeMap::new(),
        initialized: false,
        shutdown: false,
        clock,
    };
    loop {
        let message = match read_message(input) {
            Ok(Some(message)) => message,
            Ok(None) | Err(Frame::Closed) => return 1,
            Err(Frame::Bad(reason)) => {
                let _ = write_message(output, &error(Value::Null, PARSE_ERROR, &reason));
                continue;
            }
        };
        let Ok(message) = serde_json::from_slice::<Value>(&message) else {
            let _ = write_message(output, &error(Value::Null, PARSE_ERROR, "not JSON"));
            continue;
        };
        let method = message["method"].as_str().unwrap_or_default().to_string();
        if method == "exit" {
            return if server.shutdown { 0 } else { 1 };
        }
        let id = message.get("id").cloned();
        let replies = server.handle(&method, id, &message["params"]);
        for reply in replies {
            if write_message(output, &reply).is_err() {
                return 1;
            }
        }
    }
}

enum Frame {
    Closed,
    Bad(String),
}

fn read_message(input: &mut impl BufRead) -> Result<Option<Vec<u8>>, Frame> {
    let mut length: Option<usize> = None;
    let mut header = String::new();
    loop {
        header.clear();
        match input.read_line(&mut header) {
            Ok(0) => {
                return if length.is_none() {
                    Ok(None)
                } else {
                    Err(Frame::Closed)
                };
            }
            Ok(_) => {}
            Err(_) => return Err(Frame::Closed),
        }
        let line = header.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok();
            if length.is_none() {
                return Err(Frame::Bad("Content-Length is not a number".into()));
            }
        }
    }
    let length = length.unwrap_or_default();
    if length > MAX_MESSAGE {
        // Read past it so the next message still frames.
        let _ = std::io::copy(&mut input.take(length as u64), &mut std::io::sink());
        return Err(Frame::Bad(format!(
            "a message over {MAX_MESSAGE} bytes was dropped"
        )));
    }
    let mut body = vec![0u8; length];
    input.read_exact(&mut body).map_err(|_| Frame::Closed)?;
    Ok(Some(body))
}

fn write_message(output: &mut impl Write, message: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()
}

fn reply(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn notify(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

struct Server {
    /// Open `.env.schema` documents by URI.
    documents: BTreeMap<String, String>,
    initialized: bool,
    shutdown: bool,
    clock: fn() -> u64,
}

impl Server {
    fn handle(&mut self, method: &str, id: Option<Value>, params: &Value) -> Vec<Value> {
        let Some(id) = id else {
            return self.notification(method, params);
        };
        if method == "initialize" {
            self.initialized = true;
            return vec![reply(id, capabilities())];
        }
        if !self.initialized {
            return vec![error(id, SERVER_NOT_INITIALIZED, "initialize first")];
        }
        if self.shutdown {
            return vec![error(id, INVALID_REQUEST, "the server is shutting down")];
        }
        let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
        let at = position(&params["position"]);
        let text = self.documents.get(uri);
        let result = match method {
            "shutdown" => {
                self.shutdown = true;
                Value::Null
            }
            "textDocument/completion" => {
                let items = text.map_or_else(Vec::new, |t| editor::complete(t, at));
                json!({
                    "isIncomplete": false,
                    "items": items.iter().map(|c| c.to_json()).collect::<Vec<_>>(),
                })
            }
            "textDocument/hover" => text
                .and_then(|t| editor::hover(t, at, &self.notes(uri, t)))
                .map_or(Value::Null, |tip| tip.to_json()),
            "textDocument/definition" => text.and_then(|t| editor::definition(t, at)).map_or(
                Value::Null,
                |range| json!({ "uri": uri, "range": range.to_json() }),
            ),
            "textDocument/documentSymbol" => json!(
                text.map(|t| editor::symbols(t))
                    .unwrap_or_default()
                    .iter()
                    .map(|s| s.to_json())
                    .collect::<Vec<_>>()
            ),
            _ => {
                return vec![error(
                    id,
                    METHOD_NOT_FOUND,
                    &format!("penv does not answer {method}"),
                )];
            }
        };
        vec![reply(id, result)]
    }

    fn notification(&mut self, method: &str, params: &Value) -> Vec<Value> {
        if !self.initialized || self.shutdown {
            return Vec::new();
        }
        let uri = params["textDocument"]["uri"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        match method {
            "textDocument/didOpen" => {
                if !is_schema(&uri) {
                    return Vec::new();
                }
                let text = params["textDocument"]["text"].as_str().unwrap_or_default();
                self.documents.insert(uri.clone(), text.to_string());
                vec![self.publish(&uri)]
            }
            "textDocument/didChange" => {
                if !self.documents.contains_key(&uri) {
                    return Vec::new();
                }
                // The server asks for whole-document sync, so the last change is the text.
                let change = params["contentChanges"]
                    .as_array()
                    .and_then(|c| c.iter().rev().find(|c| c.get("range").is_none()));
                let Some(text) = change.and_then(|c| c["text"].as_str()) else {
                    return Vec::new();
                };
                self.documents.insert(uri.clone(), text.to_string());
                vec![self.publish(&uri)]
            }
            "textDocument/didSave" => {
                // `.penv/config.toml` may have moved a rotation clock since.
                if self.documents.contains_key(&uri) {
                    vec![self.publish(&uri)]
                } else {
                    Vec::new()
                }
            }
            "textDocument/didClose" => {
                if self.documents.remove(&uri).is_none() {
                    return Vec::new();
                }
                vec![notify(
                    "textDocument/publishDiagnostics",
                    json!({ "uri": uri, "diagnostics": [] }),
                )]
            }
            _ => Vec::new(),
        }
    }

    fn publish(&self, uri: &str) -> Value {
        let text = self
            .documents
            .get(uri)
            .map(String::as_str)
            .unwrap_or_default();
        let overdue: Vec<KeyNote> = self
            .notes(uri, text)
            .into_iter()
            .filter(|n| n.severity == Severity::Warning)
            .collect();
        let found = editor::place(text, &problems(uri, text), &overdue);
        notify(
            "textDocument/publishDiagnostics",
            json!({
                "uri": uri,
                "diagnostics": found.iter().map(|d| d.to_json()).collect::<Vec<_>>(),
            }),
        )
    }

    /// `@rotate` reminders in `check`'s words. Local mode reads the clock from
    /// `.penv/config.toml` beside the schema; under `@penv=` the clock is the
    /// provider's, which the editor does not read.
    fn notes(&self, uri: &str, text: &str) -> Vec<KeyNote> {
        let Ok(schema) = penv_schema::parse(text) else {
            return Vec::new();
        };
        let dir = uri_path(uri).and_then(|p| p.parent().map(Path::to_path_buf));
        let config = match (&dir, schema.is_cloud()) {
            (Some(dir), false) => crate::config::Config::load(dir).unwrap_or_default(),
            _ => crate::config::Config::default(),
        };
        let now = (self.clock)();
        schema
            .keys
            .iter()
            .filter_map(|key| {
                let rotate = key.rotate.as_deref()?;
                let span = Span::parse(rotate)?;
                if schema.is_cloud() {
                    return Some(KeyNote {
                        key: key.name.clone(),
                        decorator: Some("rotate".into()),
                        severity: Severity::Information,
                        code: "rotate_cloud".into(),
                        message: format!(
                            "The clock for {} is the provider's last write; penv check reads it.",
                            key.name
                        ),
                    });
                }
                let written = config.rotated(&key.name).and_then(parse_instant);
                let state = rotation(span, written, now);
                let overdue = matches!(state, Rotation::Due { left, .. } if left <= 0);
                Some(KeyNote {
                    key: key.name.clone(),
                    decorator: Some("rotate".into()),
                    severity: if overdue {
                        Severity::Warning
                    } else {
                        Severity::Information
                    },
                    code: if overdue { "rotate_overdue" } else { "rotate" }.into(),
                    message: super::check::reminder(&key.name, rotate, span, &state, false),
                })
            })
            .collect()
    }
}

/// What `check` would report for this text. A file on disk gets the same layer
/// `check` uses (imports, `.penv/config.toml`); an unsaved buffer gets the parser.
/// Warnings are this file's own, so an imported file's never land on its lines.
fn problems(uri: &str, text: &str) -> Vec<(Diagnostic, Severity)> {
    let own = editor::problems(text);
    if own.iter().any(|(_, s)| *s == Severity::Error) {
        return own;
    }
    let Some(path) = uri_path(uri) else {
        return own;
    };
    let located = match crate::source::parse_text(&path, text) {
        Ok(Ok(_)) => return own,
        Ok(Err(located)) => located,
        Err(error) => vec![(
            path.clone(),
            Diagnostic::new(1, 1, error.code, error.message),
        )],
    };
    located
        .into_iter()
        .map(|(file, d)| {
            if file == path {
                (d, Severity::Error)
            } else {
                let message = format!("{}: {}", crate::files::show(&file), d.message);
                (Diagnostic::new(1, 1, &d.code, message), Severity::Error)
            }
        })
        .collect()
}

fn capabilities() -> Value {
    json!({
        "capabilities": {
            "positionEncoding": "utf-16",
            "textDocumentSync": { "openClose": true, "change": 1, "save": true },
            "completionProvider": {
                "triggerCharacters": ["@", "=", "$", "{", "(", "|", "/", ","],
            },
            "hoverProvider": true,
            "definitionProvider": true,
            "documentSymbolProvider": true,
        },
        "serverInfo": { "name": "penv", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn position(v: &Value) -> Position {
    let n = |k: &str| {
        v[k].as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    };
    Position {
        line: n("line"),
        character: n("character"),
    }
}

/// Only `.env.schema` is served. Value files hold secrets and are never read.
fn is_schema(uri: &str) -> bool {
    let path = uri.split(['?', '#']).next().unwrap_or_default();
    let name = path.rsplit('/').next().unwrap_or_default();
    percent_decode(name).as_deref() == Some(".env.schema")
}

/// A `file:` URI as a path; `file:///c%3A/x` and `file:///C:/x` on Windows alike.
pub fn uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.split(['?', '#']).next()?;
    // A host other than localhost is a network share penv does not read from.
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    let decoded = percent_decode(path)?;
    let bytes = decoded.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        return Some(PathBuf::from(&decoded[1..]));
    }
    Some(PathBuf::from(decoded))
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_schema_is_served() {
        assert!(is_schema("file:///home/a/app/.env.schema"));
        assert!(is_schema("file:///c%3A/app/.env.schema"));
        assert!(is_schema("untitled:/x/.env.schema"));
        assert!(!is_schema("file:///home/a/app/.env"));
        assert!(!is_schema("file:///home/a/app/.env.production"));
        assert!(!is_schema("file:///home/a/app/.env.schema.bak"));
        assert!(!is_schema("file:///home/a/app/x.env.schema"));
    }

    #[test]
    fn file_uris_become_paths_on_every_platform() {
        assert_eq!(
            uri_path("file:///home/a/my%20app/.env.schema").unwrap(),
            PathBuf::from("/home/a/my app/.env.schema")
        );
        assert_eq!(
            uri_path("file:///c%3A/app/.env.schema").unwrap(),
            PathBuf::from("c:/app/.env.schema")
        );
        assert_eq!(
            uri_path("file:///C:/app/.env.schema").unwrap(),
            PathBuf::from("C:/app/.env.schema")
        );
        assert!(uri_path("untitled:Untitled-1").is_none());
        assert!(uri_path("file://server/share/.env.schema").is_none());
        assert_eq!(
            uri_path("file://localhost/srv/.env.schema").unwrap(),
            PathBuf::from("/srv/.env.schema")
        );
        assert!(uri_path("file:///bad%zz").is_none());
    }
}
