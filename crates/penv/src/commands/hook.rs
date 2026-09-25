use std::io::Read;

use penv_guards::{Hook, Payload};
use serde_json::Value;

use crate::error::{CliError, Exit};
use crate::files::{Disk, home};
use crate::output::Report;

pub const READS_ENV: &str = "penv blocks reads of .env files. Run penv ls for the key names and penv check for what is missing; .env.schema is readable.";
pub const DUMPS_ENV: &str = "penv blocks dumping the environment, because it prints every value. Run penv ls for the key names.";
pub const PULLS_VALUES: &str = "penv pull writes every value to a file, so it is refused in an agent session. Run penv run to give a command its values; penv reveal KEY asks a person first.";

/// What a harness handed the hook, reduced to the two things that matter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub command: Option<String>,
    pub path: Option<String>,
}

impl Request {
    pub fn command(command: &str) -> Request {
        Request {
            command: Some(command.to_string()),
            path: None,
        }
    }

    pub fn path(path: &str) -> Request {
        Request {
            command: None,
            path: Some(path.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(&'static str),
}

/// Read the harness payload from stdin, decide, and answer in the shape that
/// harness's folder declares. A payload with something to match on that cannot
/// be read is a refusal; only empty stdin is an allow. The answer's shape never
/// comes from the repository's own guard folders, which an agent can write.
pub fn run(harness: &str) -> Result<Report, CliError> {
    let hook = penv_guards::hook(&Disk, home().as_deref(), harness).unwrap_or_else(Hook::generic);

    let mut payload = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload);

    let Decision::Deny(reason) = decide(&extract(hook.payload, &payload)) else {
        return Ok(Report::silent());
    };

    let body = penv_guards::deny(harness, &hook, reason).map_err(|e| {
        CliError::new(
            "guard_failed",
            e.to_string(),
            "Fix the [hook] deny template in the guard folder, or drop it so the built-in one is used again.",
        )
    })?;
    let exit = exit_code(hook.deny.exit);

    if hook.deny.stdout.is_some() {
        let json = serde_json::from_str::<Value>(&body).unwrap_or(Value::String(body.clone()));
        return Ok(Report::new(json, body).with_exit(exit));
    }
    eprintln!("{body}");
    Ok(Report::silent().with_exit(exit))
}

/// A hook protocol reads 0 as answered and 2 as blocked. Anything else a folder
/// asks for is this binary's plain failure.
fn exit_code(code: i64) -> Exit {
    match code {
        0 => Exit::Ok,
        2 => Exit::Auth,
        _ => Exit::Error,
    }
}

/// Pull a command and a path out of the JSON the harness sent. The family says
/// where its own fields sit; anything else is found by name, wherever it is.
pub fn extract(family: Payload, payload: &str) -> Request {
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        let text = payload.trim();
        return if text.is_empty() {
            Request::default()
        } else {
            Request::command(text)
        };
    };
    let mut request = Request::default();
    match family {
        Payload::ClaudeCode => walk(&value["tool_input"], &mut request),
        Payload::Cursor => fields(&value, &mut request),
        Payload::Generic => {}
    }
    walk(&value, &mut request);
    request
}

fn walk(value: &Value, request: &mut Request) {
    match value {
        Value::Object(_) => {
            fields(value, request);
            for child in value.as_object().into_iter().flatten().map(|(_, v)| v) {
                walk(child, request);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| walk(item, request)),
        _ => {}
    }
}

/// The field names every harness uses for the thing about to run or be read.
/// A Glob or Grep `pattern` is read as a path, so a glob over `.env` counts.
fn fields(value: &Value, request: &mut Request) {
    for (name, child) in value.as_object().into_iter().flatten() {
        match (name.as_str(), child) {
            // Windsurf's pre_run_command sends command_line.
            ("command" | "cmd" | "command_line", Value::String(text))
                if request.command.is_none() =>
            {
                request.command = Some(text.clone());
            }
            (
                "file_path" | "filePath" | "absolute_path" | "path" | "file" | "pattern" | "glob"
                // Gemini's search_file_content and Cline's search_files name a glob.
                | "include" | "file_pattern",
                Value::String(text),
            ) => offer(request, text),
            // Gemini's read_many_files names several at once.
            ("paths" | "include" | "file_paths" | "files", Value::Array(items)) => {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .for_each(|p| offer(request, p));
            }
            _ => {}
        }
    }
}

/// The first path named, unless a later one would be refused: a Grep carries a
/// pattern and a path, and field order is the harness's, not ours.
fn offer(request: &mut Request, path: &str) {
    let held = request.path.as_deref().is_some_and(touches_env_file);
    if request.path.is_none() || (!held && touches_env_file(path)) {
        request.path = Some(path.to_string());
    }
}

/// The whole policy, as a function of the request. No I/O, no environment.
pub fn decide(request: &Request) -> Decision {
    decide_at(request, 0)
}

/// A shell that runs another command is decided again on the inner text, up to a
/// depth no real command line reaches.
const DEPTH: u8 = 4;

fn decide_at(request: &Request, depth: u8) -> Decision {
    if request.path.as_deref().is_some_and(touches_env_file) {
        return Decision::Deny(READS_ENV);
    }
    let Some(command) = &request.command else {
        return Decision::Allow;
    };
    // A quoted snippet carries its own separators, so it is read whole before
    // the line is split into the things that actually run.
    if inline_snippet_prints_the_environment(&words(command)) {
        return Decision::Deny(DUMPS_ENV);
    }
    for segment in segments(command) {
        if let Some(reason) = refuse(&segment, depth) {
            return Decision::Deny(reason);
        }
    }
    Decision::Allow
}

/// True for `.env` and `.env.<anything>`, and false for `.env.schema`, which
/// holds no values and is the file an agent needs most. A glob that could name
/// one of them counts as naming it. Case is folded: `.ENV` is the same file on
/// macOS and Windows.
pub fn touches_env_file(candidate: &str) -> bool {
    candidate.split('=').any(|piece| {
        let piece = piece.trim_matches(['"', '\'', '`', '(', ')', '<', '>']);
        let name = piece
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(piece)
            .to_ascii_lowercase();
        if name == ".env.schema" {
            return false;
        }
        name == ".env"
            || name.starts_with(".env.")
            || glob_reaches_a_value_file(&name)
            || is_penv_key_file(piece)
    })
}

/// penv's local encryption key, where it lives when the machine has no
/// keychain: `~/.config/penv/local.key`, `%APPDATA%\\penv\\local.key`, or
/// under `$XDG_CONFIG_HOME`. Read with it, an encrypted `.env` is plain text.
fn is_penv_key_file(path: &str) -> bool {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    matches!(parts.as_slice(), [.., dir, file] if dir.eq_ignore_ascii_case("penv") && file.eq_ignore_ascii_case("local.key"))
}

/// A pattern with two or more literal characters that matches `.env` or an
/// `.env.<anything>`. The literal count keeps a bare `*` from denying every
/// command that ends in one.
fn glob_reaches_a_value_file(pattern: &str) -> bool {
    if !pattern.contains(['*', '?', '[']) {
        return false;
    }
    let literals = pattern
        .chars()
        .filter(|c| !matches!(c, '*' | '?' | '[' | ']'))
        .count();
    literals >= 2
        && [".env", ".env.local"]
            .iter()
            .any(|name| glob_matches(pattern, name))
}

/// `*`, `?` and `[...]` against one name, the subset every shell agrees on.
fn glob_matches(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut i, mut j) = (0, 0);
    let (mut star, mut resume) = (None, 0);
    while j < n.len() {
        match p.get(i) {
            Some('*') => {
                star = Some(i);
                resume = j;
                i += 1;
            }
            Some('[') => {
                let end = p[i..].iter().position(|c| *c == ']').map(|at| i + at);
                match end.filter(|end| class(&p[i + 1..*end], n[j])) {
                    Some(end) => {
                        i = end + 1;
                        j += 1;
                    }
                    None => match star {
                        Some(at) => {
                            i = at + 1;
                            resume += 1;
                            j = resume;
                        }
                        None => return false,
                    },
                }
            }
            Some('?') => {
                i += 1;
                j += 1;
            }
            Some(c) if *c == n[j] => {
                i += 1;
                j += 1;
            }
            _ => match star {
                Some(at) => {
                    i = at + 1;
                    resume += 1;
                    j = resume;
                }
                None => return false,
            },
        }
    }
    p[i..].iter().all(|c| *c == '*')
}

fn class(members: &[char], c: char) -> bool {
    let (negated, members) = match members.first() {
        Some('!') | Some('^') => (true, &members[1..]),
        _ => (false, members),
    };
    let mut hit = false;
    let mut at = 0;
    while at < members.len() {
        if at + 2 < members.len() && members[at + 1] == '-' {
            hit |= members[at] <= c && c <= members[at + 2];
            at += 3;
        } else {
            hit |= members[at] == c;
            at += 1;
        }
    }
    hit != negated
}

fn refuse(segment: &str, depth: u8) -> Option<&'static str> {
    let words = words(segment);
    if words.iter().any(|w| touches_env_file(w)) {
        return Some(READS_ENV);
    }
    if dumps_environment(&words) {
        return Some(DUMPS_ENV);
    }
    if pulls_values(&words) {
        return Some(PULLS_VALUES);
    }
    if depth < DEPTH
        && let Some(inner) = inner_command(&words)
        && let Decision::Deny(reason) = decide_at(&Request::command(&inner), depth + 1)
    {
        return Some(reason);
    }
    None
}

/// The command a shell was handed to run: `sh -c ...`, `pwsh -Command ...`,
/// `cmd /c ...`. Without this, every matcher below reads only the shell.
fn inner_command(words: &[String]) -> Option<String> {
    let rest = skip_assignments(words);
    let first = rest.first().map(|w| leaf(w))?;
    let shell = matches!(
        first.as_str(),
        "sh" | "bash"
            | "zsh"
            | "dash"
            | "ksh"
            | "fish"
            | "busybox"
            | "pwsh"
            | "pwsh.exe"
            | "powershell"
            | "powershell.exe"
            | "cmd"
            | "cmd.exe"
    );
    if !shell {
        return None;
    }
    let at = rest.iter().position(|w| {
        matches!(
            w.to_lowercase().as_str(),
            "-c" | "/c" | "/k" | "-command" | "-encodedcommand"
        )
    })?;
    let inner = rest[at + 1..].join(" ");
    (!inner.trim().is_empty()).then_some(inner)
}

fn dumps_environment(words: &[String]) -> bool {
    let rest = skip_assignments(words);
    let Some(first) = rest.first().map(|w| leaf(w)) else {
        return false;
    };
    let arguments = &rest[1..];

    // printenv only ever prints values; env and set only when nothing follows.
    if first == "printenv" {
        return true;
    }
    if first == "env" && arguments.iter().all(|a| a.starts_with('-')) {
        return true;
    }
    if first == "set" && arguments.is_empty() {
        return true;
    }
    if matches!(
        first.to_lowercase().as_str(),
        "get-childitem" | "gci" | "ls" | "dir" | "get-item" | "gi"
    ) && arguments
        .iter()
        .any(|a| a.to_lowercase().starts_with("env:"))
    {
        return true;
    }
    false
}

/// `node -e process.env`, `python -c os.environ`. An interpreter and an inline
/// flag have to be there too, so `grep -e process.env` stays a search.
fn inline_snippet_prints_the_environment(words: &[String]) -> bool {
    let interpreter = words.iter().any(|w| {
        matches!(
            leaf(w).as_str(),
            "node"
                | "node.exe"
                | "deno"
                | "bun"
                | "python"
                | "python3"
                | "python.exe"
                | "py"
                | "ruby"
                | "perl"
                | "sh"
                | "bash"
                | "zsh"
                | "pwsh"
                | "powershell"
                | "powershell.exe"
        )
    });
    let flagged = words.iter().any(|w| {
        matches!(
            w.as_str(),
            "-c" | "-e" | "-p" | "--eval" | "--print" | "--command"
        )
    });
    interpreter
        && flagged
        && words
            .iter()
            .any(|w| w.contains("process.env") || w.contains("os.environ"))
}

// `penv reveal` is let through: under an agent it asks a person before it prints anything.
fn pulls_values(words: &[String]) -> bool {
    let rest = skip_assignments(words);
    let Some(first) = rest.first().map(|w| leaf(w)) else {
        return false;
    };
    if first != "penv" && first != "penv.exe" {
        return false;
    }
    rest[1..]
        .iter()
        .find(|w| !w.starts_with('-'))
        .is_some_and(|w| w == "pull")
}

fn skip_assignments(words: &[String]) -> &[String] {
    let start = words
        .iter()
        .position(|w| !w.contains('=') || w.starts_with('-'))
        .unwrap_or(words.len());
    &words[start..]
}

fn leaf(word: &str) -> String {
    word.rsplit(['/', '\\'])
        .next()
        .unwrap_or(word)
        .to_lowercase()
}

/// One shell line per thing that actually runs.
fn segments(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ';' | '\n' | '|' | '&' => {
                if (c == '|' || c == '&') && chars.peek() == Some(&c) {
                    chars.next();
                }
                out.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    out.push(current);
    out.into_iter()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .collect()
}

fn words(segment: &str) -> Vec<String> {
    segment
        .split_whitespace()
        .map(|word| {
            word.trim_matches(['"', '\'', '`', '(', ')'])
                .trim_start_matches(['<', '>'])
                .to_string()
        })
        .filter(|word| !word.is_empty())
        .collect()
}
