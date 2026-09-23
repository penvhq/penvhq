//! Values that are computed: `${KEY}` expansion and the varlock functions penv
//! runs. Pure: the caller hands in every raw value and the process environment.

use std::collections::{BTreeMap, BTreeSet};

use crate::validate::Values;

/// The functions penv evaluates. `exec` is refused by name: a schema never
/// starts a process.
pub const FUNCTIONS: [&str; 15] = [
    "ref",
    "concat",
    "fallback",
    "if",
    "eq",
    "not",
    "isEmpty",
    "forEnv",
    "penv",
    "match",
    "and",
    "or",
    "startsWith",
    "endsWith",
    "random",
];

/// Filters a `${KEY | filter}` reference runs, left to right.
pub const FILTERS: [&str; 5] = ["urlencode", "base64", "lower", "upper", "trim"];

/// How deep references may chain before penv stops rather than exhaust the stack.
pub const MAX_DEPTH: usize = 128;

/// One raw value and whether it may be computed. A single-quoted value is literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raw {
    pub text: String,
    pub computed: bool,
}

impl Raw {
    pub fn literal(text: impl Into<String>) -> Raw {
        Raw {
            text: text.into(),
            computed: false,
        }
    }

    pub fn computed(text: impl Into<String>) -> Raw {
        Raw {
            text: text.into(),
            computed: true,
        }
    }
}

/// A key whose value could not be computed. Never carries a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveError {
    pub key: String,
    pub message: String,
    /// A reference to a key nothing sets: the value was computed with that part
    /// empty. Reported, never fatal, because `$word` inside a password is how a
    /// secret gets silently cut short.
    pub soft: bool,
}

/// True when the text would be evaluated rather than passed through.
pub fn is_expression(text: &str) -> bool {
    call_name(text).is_some() || has_reference(text)
}

/// Compute every value. `outside` is the process environment, read only when a
/// reference names a key that no file or default supplies. `fetched` holds each
/// `penv(address)` the caller looked up, keyed by the address as written.
pub fn resolve(
    raw: &BTreeMap<String, Raw>,
    outside: &Values,
    environment: &str,
    fetched: &Values,
) -> (Values, Vec<ResolveError>) {
    let out = resolve_full(raw, outside, environment, fetched);
    (out.values, out.errors)
}

/// Everything one resolution learned: the values, what failed, and which keys
/// (or `penv:<address>` reads) each value was computed from.
#[derive(Debug, Clone, Default)]
pub struct Resolution {
    pub values: Values,
    pub errors: Vec<ResolveError>,
    pub deps: BTreeMap<String, BTreeSet<String>>,
}

pub fn resolve_full(
    raw: &BTreeMap<String, Raw>,
    outside: &Values,
    environment: &str,
    fetched: &Values,
) -> Resolution {
    let mut run = Run {
        raw,
        outside,
        environment,
        fetched,
        done: Values::new(),
        failed: BTreeSet::new(),
        stack: Vec::new(),
        errors: Vec::new(),
        quiet: 0,
        deps: BTreeMap::new(),
        control: 0,
    };
    for name in raw.keys() {
        run.value(name);
    }
    Resolution {
        values: run.done,
        errors: run.errors,
        deps: run.deps,
    }
}

/// Keys whose value was computed, however indirectly, from a sensitive key or a
/// `penv()` read of one: a URL built from a password is a password.
pub fn tainted(
    deps: &BTreeMap<String, BTreeSet<String>>,
    sensitive: impl Fn(&str) -> bool,
) -> BTreeSet<String> {
    let source = |dep: &str| match dep.strip_prefix("penv:") {
        // `penv(env/KEY)` inherits KEY's sensitivity; an address penv cannot
        // judge is treated as sensitive.
        Some(address) => sensitive(address.rsplit('/').next().unwrap_or(address)),
        None => sensitive(dep),
    };
    let mut out: BTreeSet<String> = BTreeSet::new();
    loop {
        let before = out.len();
        for (key, from) in deps {
            if !out.contains(key) && from.iter().any(|d| source(d) || out.contains(d)) {
                out.insert(key.clone());
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

/// Every `penv(address)` the computed values name, so the caller can fetch them
/// before `resolve` runs.
pub fn penv_addresses(raw: &BTreeMap<String, Raw>) -> Vec<String> {
    let mut out = Vec::new();
    for (name, value) in raw
        .iter()
        .filter(|(_, r)| r.computed && is_expression(&r.text))
    {
        if let Ok(expr) = parse(&value.text) {
            collect(&expr, name, &mut out);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `key` is the key the expression belongs to: `penv()` with no argument reads
/// that same key, the form varlock's penv plugin writes.
fn collect(expr: &Expr, key: &str, out: &mut Vec<String>) {
    match expr {
        Expr::Call(name, args) => {
            match (name == "penv", args.as_slice()) {
                (true, [Expr::Text(address)]) => out.push(address.trim().to_string()),
                (true, []) => out.push(key.to_string()),
                _ => {}
            }
            args.iter().for_each(|a| collect(a, key, out));
        }
        Expr::Template(parts) => parts.iter().for_each(|p| collect(p, key, out)),
        Expr::Default { or, .. } => collect(or, key, out),
        Expr::Filter { inner, .. } => collect(inner, key, out),
        Expr::Text(_) | Expr::Ref(_) => {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    Text(String),
    Ref(String),
    /// `${KEY:-or}` (empty or unset) and `${KEY-or}` (unset only).
    Default {
        key: String,
        or: Box<Expr>,
        when_empty: bool,
    },
    /// `${KEY | urlencode | lower}`.
    Filter {
        inner: Box<Expr>,
        filters: Vec<String>,
    },
    Template(Vec<Expr>),
    Call(String, Vec<Expr>),
}

struct Run<'a> {
    raw: &'a BTreeMap<String, Raw>,
    outside: &'a Values,
    environment: &'a str,
    fetched: &'a Values,
    done: Values,
    failed: BTreeSet<String>,
    stack: Vec<String>,
    errors: Vec<ResolveError>,
    /// Inside `fallback`, `isEmpty`, an `if` condition or `${KEY-or}`, where an
    /// unset key is the point rather than a mistake.
    quiet: usize,
    deps: BTreeMap<String, BTreeSet<String>>,
    /// Inside a condition: a key read here decides which value is chosen but is
    /// not copied into it, so it does not taint the result.
    control: usize,
}

impl Run<'_> {
    fn value(&mut self, name: &str) -> Option<String> {
        if let Some(v) = self.done.get(name) {
            return Some(v.clone());
        }
        if self.failed.contains(name) {
            return None;
        }
        let Some(raw) = self.raw.get(name) else {
            return self.outside.get(name).cloned();
        };
        if !raw.computed || !is_expression(&raw.text) {
            self.done.insert(name.to_string(), raw.text.clone());
            return Some(raw.text.clone());
        }
        if self.stack.len() >= MAX_DEPTH {
            self.fail(
                name,
                format!("{name}: references nest more than {MAX_DEPTH} deep"),
            );
            return None;
        }
        if self.stack.iter().any(|k| k == name) {
            let cycle = [self.stack.as_slice(), &[name.to_string()]]
                .concat()
                .join(" -> ");
            self.fail(name, format!("{name} refers back to itself: {cycle}"));
            return None;
        }
        let expr = match parse(&raw.text) {
            Ok(expr) => expr,
            Err(message) => {
                self.fail(name, format!("{name}: {message}"));
                return None;
            }
        };
        // Another key is computed on its own terms: a condition or fallback
        // around the reference to it does not reach inside it.
        let (quiet, control) = (self.quiet, self.control);
        (self.quiet, self.control) = (0, 0);
        self.stack.push(name.to_string());
        let result = self.eval(&expr);
        self.stack.pop();
        (self.quiet, self.control) = (quiet, control);
        match result {
            Ok(v) => {
                self.done.insert(name.to_string(), v.clone());
                Some(v)
            }
            Err(message) => {
                self.fail(name, format!("{name}: {message}"));
                None
            }
        }
    }
    /// The reference's name is never said: in a password, `$word` is part of the
    /// secret, not a key someone meant.
    fn unset(&mut self, _reference: &str) {
        let Some(key) = self.stack.last().cloned() else {
            return;
        };
        let message = format!(
            "{key} holds a $ reference to a key nothing sets, so that part is empty; write \\$ or single-quote the value to keep it as written"
        );
        if !self
            .errors
            .iter()
            .any(|e| e.key == key && e.message == message)
        {
            self.errors.push(ResolveError {
                key,
                message,
                soft: true,
            });
        }
    }

    /// Note that the value being computed reads `dep`.
    fn depend(&mut self, dep: &str) {
        if self.control > 0 {
            return;
        }
        if let Some(key) = self.stack.last().cloned() {
            self.deps.entry(key).or_default().insert(dep.to_string());
        }
    }

    /// Evaluate where an unset key is expected.
    fn quietly(&mut self, expr: &Expr) -> Result<String, String> {
        self.quiet += 1;
        let out = self.eval(expr);
        self.quiet -= 1;
        out
    }

    fn fail(&mut self, name: &str, message: String) {
        if self.failed.insert(name.to_string()) {
            self.errors.push(ResolveError {
                key: name.to_string(),
                message,
                soft: false,
            });
        }
    }

    fn eval(&mut self, expr: &Expr) -> Result<String, String> {
        match expr {
            Expr::Text(t) => Ok(t.clone()),
            Expr::Ref(name) => {
                if self.raw.contains_key(name) && self.value(name).is_none() {
                    return Err(format!("{name} could not be computed"));
                }
                self.depend(name);
                let found = self.value(name);
                if found.is_none() && self.quiet == 0 {
                    self.unset(name);
                }
                Ok(found.unwrap_or_default())
            }
            Expr::Default {
                key,
                or,
                when_empty,
            } => {
                let set = self.raw.contains_key(key) || self.outside.contains_key(key);
                // Looking is control: only a value that is used is a dependency,
                // so a fallback taken does not inherit the key's sensitivity.
                self.control += 1;
                let value = self.quietly(&Expr::Ref(key.clone()));
                self.control -= 1;
                let value = value?;
                if !set || (*when_empty && value.is_empty()) {
                    self.eval(or)
                } else {
                    self.depend(key);
                    Ok(value)
                }
            }
            Expr::Filter { inner, filters } => {
                let mut value = self.eval(inner)?;
                for filter in filters {
                    value = apply_filter(filter, &value)?;
                }
                Ok(value)
            }
            Expr::Template(parts) => parts.iter().map(|p| self.eval(p)).collect(),
            Expr::Call(name, args) => self.call(name, args),
        }
    }

    fn call(&mut self, name: &str, args: &[Expr]) -> Result<String, String> {
        // A function that answers true or false copies no value out of its
        // arguments: what it reads is control, not data.
        let decides = matches!(
            name,
            "eq" | "not" | "isEmpty" | "forEnv" | "and" | "or" | "startsWith" | "endsWith"
        );
        if decides {
            self.control += 1;
        }
        let out = self.call_inner(name, args);
        if decides {
            self.control -= 1;
        }
        out
    }

    fn deciding(&mut self, expr: &Expr) -> Result<String, String> {
        self.control += 1;
        let out = self.quietly(expr);
        self.control -= 1;
        out
    }

    fn call_inner(&mut self, name: &str, args: &[Expr]) -> Result<String, String> {
        let arity = |min: usize, max: usize| {
            if args.len() < min || args.len() > max {
                Err(format!(
                    "{name}() takes {} argument(s), not {}",
                    if min == max {
                        min.to_string()
                    } else {
                        format!("{min} to {max}")
                    },
                    args.len()
                ))
            } else {
                Ok(())
            }
        };
        match name {
            "ref" => {
                arity(1, 1)?;
                match &args[0] {
                    Expr::Text(key) | Expr::Ref(key) => self.eval(&Expr::Ref(key.clone())),
                    _ => Err("ref() takes a key name".into()),
                }
            }
            "concat" => args.iter().map(|a| self.eval(a)).collect(),
            "fallback" => {
                arity(1, usize::MAX)?;
                for arg in args {
                    let v = self.quietly(arg)?;
                    if !v.is_empty() {
                        return Ok(v);
                    }
                }
                Ok(String::new())
            }
            "if" => {
                arity(2, 3)?;
                if truthy(&self.deciding(&args[0])?) {
                    self.eval(&args[1])
                } else {
                    args.get(2).map_or(Ok(String::new()), |a| self.eval(a))
                }
            }
            "eq" => {
                arity(2, 2)?;
                Ok(flag(self.eval(&args[0])? == self.eval(&args[1])?))
            }
            "not" => {
                arity(1, 1)?;
                Ok(flag(!truthy(&self.eval(&args[0])?)))
            }
            "isEmpty" => {
                arity(1, 1)?;
                Ok(flag(self.quietly(&args[0])?.is_empty()))
            }
            "penv" => {
                arity(0, 1)?;
                let address = match args.first() {
                    Some(arg) => self.eval(arg)?,
                    None => self.stack.last().cloned().unwrap_or_default(),
                };
                let address = address.trim();
                self.depend(&format!("penv:{address}"));
                self.fetched.get(address).cloned().ok_or_else(|| {
                    format!(
                        "penv({address}) takes a written address, such as production/DATABASE_URL"
                    )
                })
            }
            "forEnv" => {
                arity(1, usize::MAX)?;
                let mut hit = false;
                for arg in args {
                    hit |= self.eval(arg)? == self.environment;
                }
                Ok(flag(hit))
            }
            "match" => {
                arity(2, usize::MAX)?;
                let subject = self.deciding(&args[0])?;
                let mut fallback = None;
                for arm in &args[1..] {
                    let Expr::Call(kind, parts) = arm else {
                        return Err("match() takes cases written label: value".into());
                    };
                    if kind != ":" {
                        return Err("match() takes cases written label: value".into());
                    }
                    let label = self.deciding(&parts[0])?;
                    if label == "_" {
                        fallback = Some(&parts[1]);
                    } else if label == subject {
                        return self.eval(&parts[1]);
                    }
                }
                match fallback {
                    Some(expr) => self.eval(expr),
                    None => Err("match() found no case for the value and has no _ case".into()),
                }
            }
            "and" | "or" => {
                arity(1, usize::MAX)?;
                let want = name == "and";
                for arg in args {
                    if truthy(&self.quietly(arg)?) != want {
                        return Ok(flag(!want));
                    }
                }
                Ok(flag(want))
            }
            "startsWith" | "endsWith" => {
                arity(2, 2)?;
                let value = self.eval(&args[0])?;
                let part = self.eval(&args[1])?;
                Ok(flag(if name == "startsWith" {
                    value.starts_with(&part)
                } else {
                    value.ends_with(&part)
                }))
            }
            "random" => {
                Err("random() is generated by penv run, which keeps the value in .env.local".into())
            }
            _ => Err(format!("{name}() is not a function penv runs")),
        }
    }
}

fn truthy(v: &str) -> bool {
    !v.is_empty() && v != "false" && v != "0"
}

fn flag(b: bool) -> String {
    if b { "true" } else { "false" }.to_string()
}

/// The function name when the whole text is one call: `name(...)` with a
/// lowercase identifier, the shape varlock gives every function, plugins' too.
fn call_name(text: &str) -> Option<&str> {
    let text = text.trim();
    let open = text.find('(')?;
    let name = &text[..open];
    let mut chars = name.chars();
    let shaped = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_alphanumeric());
    (shaped && text.ends_with(')')).then_some(name)
}

fn has_reference(text: &str) -> bool {
    if text.contains("\\$") {
        return true;
    }
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(i, b)| {
        *b == b'$'
            && bytes
                .get(i + 1)
                .is_some_and(|n| matches!(n, b'{' | b'_') || n.is_ascii_alphabetic())
    })
}

fn parse(text: &str) -> Result<Expr, String> {
    let text = text.trim();
    if let Some(name) = call_name(text) {
        if name == "exec" {
            return Err(
                "exec() runs a shell command, which penv never does from a schema; use penv run"
                    .into(),
            );
        }
        if !FUNCTIONS.contains(&name) {
            return Err(format!(
                "{name}() is not a function penv runs (a varlock plugin or builtin resolves it); single-quote the value to pass it as written"
            ));
        }
        let inner = &text[name.len() + 1..text.len() - 1];
        let args = split(inner)?
            .iter()
            .map(|a| arg(a))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Expr::Call(name.to_string(), args));
    }
    Ok(template(text))
}

fn arg(text: &str) -> Result<Expr, String> {
    let text = text.trim();
    // A `label: value` case for match(): the first `:` outside quotes.
    if let Some(colon) = case_colon(text) {
        let label = arg(&text[..colon])?;
        let value = arg(&text[colon + 1..])?;
        return Ok(Expr::Call(":".into(), vec![label, value]));
    }
    if let Some(q) = text.chars().next().filter(|c| *c == '"' || *c == '\'') {
        if text.len() >= 2 && text.ends_with(q) {
            let inner = &text[1..text.len() - 1];
            return Ok(if q == '"' {
                template(inner)
            } else {
                Expr::Text(inner.into())
            });
        }
        return Err("a quoted argument is never closed".into());
    }
    // `${KEY:-or}` and `${KEY | filter}` as an argument read like they do in text.
    if text.starts_with("${") {
        return Ok(template(text));
    }
    if let Some(name) = text.strip_prefix('$') {
        let name = name
            .strip_prefix('{')
            .and_then(|n| n.strip_suffix('}'))
            .unwrap_or(name);
        return Ok(Expr::Ref(name.to_string()));
    }
    if call_name(text).is_some() {
        return parse(text);
    }
    Ok(Expr::Text(text.to_string()))
}

/// The `:` of a `label: value` match case: after a bare label, before anything
/// quoted or bracketed, and followed by a space. `http://x` and `a:b` are not cases.
fn case_colon(text: &str) -> Option<usize> {
    let colon = text.find(": ")?;
    let label = &text[..colon];
    let bare = !label.is_empty()
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    let quoted = label.len() >= 2
        && (label.starts_with('"') && label.ends_with('"')
            || label.starts_with('\'') && label.ends_with('\''));
    (bare || quoted).then_some(colon)
}

fn apply_filter(filter: &str, value: &str) -> Result<String, String> {
    Ok(match filter {
        "urlencode" => value
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect(),
        "base64" => base64(value.as_bytes()),
        "lower" => value.to_lowercase(),
        "upper" => value.to_uppercase(),
        "trim" => value.trim().to_string(),
        other => {
            return Err(format!(
                "{other} is not a filter penv runs; filters are {}",
                FILTERS.join(", ")
            ));
        }
    })
}

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `${KEY}` and `$KEY` inside text. `\$` is a literal dollar, as in
/// dotenv-expand, and so is a `$` that no name follows.
fn template(text: &str) -> Expr {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && chars.get(i + 1) == Some(&'$') {
            buf.push('$');
            i += 2;
            continue;
        }
        if chars[i] != '$' {
            buf.push(chars[i]);
            i += 1;
            continue;
        }
        if chars.get(i + 1) == Some(&'{')
            && let Some(end) = closing_brace(&chars, i + 2)
        {
            let whole: String = chars[i + 2..end].iter().collect();
            let mut pieces = top_level_pipes(&whole).into_iter();
            let inner = pieces.next().unwrap_or_default().trim().to_string();
            let filters: Vec<String> = pieces.map(|f| f.trim().to_string()).collect();
            // A key name holds no `-`, so the first one is the operator.
            let split = inner
                .find('-')
                .map(|dash| match inner[..dash].strip_suffix(':') {
                    Some(key) => (key.len(), 2, true),
                    None => (dash, 1, false),
                });
            let expr = match split {
                Some((at, width, when_empty)) => Expr::Default {
                    key: inner[..at].to_string(),
                    or: Box::new(template(&inner[at + width..])),
                    when_empty,
                },
                None => Expr::Ref(inner),
            };
            let expr = if filters.is_empty() {
                expr
            } else {
                Expr::Filter {
                    inner: Box::new(expr),
                    filters,
                }
            };
            if !buf.is_empty() {
                parts.push(Expr::Text(std::mem::take(&mut buf)));
            }
            parts.push(expr);
            i = end + 1;
            continue;
        }
        let (name, next) = if chars.get(i + 1) == Some(&'{') {
            (String::new(), i)
        } else {
            let starts = chars
                .get(i + 1)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_');
            let end = chars[i + 1..]
                .iter()
                .position(|c| !(c.is_ascii_alphanumeric() || *c == '_'))
                .map_or(chars.len(), |p| i + 1 + p);
            if starts {
                (chars[i + 1..end].iter().collect(), end)
            } else {
                (String::new(), i)
            }
        };
        if name.is_empty() {
            buf.push('$');
            i += 1;
            continue;
        }
        if !buf.is_empty() {
            parts.push(Expr::Text(std::mem::take(&mut buf)));
        }
        parts.push(Expr::Ref(name));
        i = next;
    }
    if !buf.is_empty() {
        parts.push(Expr::Text(buf));
    }
    match parts.len() {
        0 => Expr::Text(String::new()),
        1 if matches!(parts[0], Expr::Text(_)) => parts.remove(0),
        _ => Expr::Template(parts),
    }
}

/// Split on `|` outside any nested `${...}`.
fn top_level_pipes(text: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => {
                out.push(String::new());
                continue;
            }
            _ => {}
        }
        out.last_mut().expect("never empty").push(c);
    }
    out
}

/// The `}` that closes a `${`, skipping nested `${...}` inside a default.
fn closing_brace(chars: &[char], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in chars.iter().enumerate().skip(from) {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Split call arguments on commas outside quotes and brackets.
fn split(inner: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in inner.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '(' => {
                    depth += 1;
                    cur.push(c);
                }
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        return Err("a closing bracket has no opening one".into());
                    }
                    cur.push(c);
                }
                ',' if depth == 0 => out.push(std::mem::take(&mut cur)),
                _ => cur.push(c),
            },
        }
    }
    if depth != 0 || quote.is_some() {
        return Err("a bracket or quote is never closed".into());
    }
    if !cur.trim().is_empty() || !out.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(pairs: &[(&str, Raw)], env: &str) -> (Values, Vec<ResolveError>) {
        let raw = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        resolve(&raw, &Values::new(), env, &Values::new())
    }

    #[test]
    fn expansion_reads_other_keys_and_single_quotes_stay_literal() {
        let (v, e) = run(
            &[
                ("HOST", Raw::literal("db.test")),
                ("URL", Raw::computed("postgres://${HOST}:5432/$NAME")),
                ("NAME", Raw::literal("app")),
                ("RAW", Raw::literal("${HOST}")),
                ("PRICE", Raw::computed("\\$5 and $ alone")),
            ],
            "development",
        );
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(v["URL"], "postgres://db.test:5432/app");
        assert_eq!(v["RAW"], "${HOST}");
        assert_eq!(v["PRICE"], "$5 and $ alone");
    }

    #[test]
    fn functions_compose() {
        let (v, e) = run(
            &[
                ("APP_ENV", Raw::literal("production")),
                (
                    "API",
                    Raw::computed("if(eq(ref(APP_ENV), production), api.test, staging.api.test)"),
                ),
                ("TAG", Raw::computed("concat(v, $APP_ENV, \"-${APP_ENV}\")")),
                ("FIRST", Raw::computed("fallback($MISSING, '', second)")),
                ("PROD", Raw::computed("forEnv(staging, production)")),
                ("EMPTY", Raw::computed("isEmpty($MISSING)")),
            ],
            "production",
        );
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(v["API"], "api.test");
        assert_eq!(v["TAG"], "vproduction-production");
        assert_eq!(v["FIRST"], "second");
        assert_eq!(v["PROD"], "true");
        assert_eq!(v["EMPTY"], "true");
    }

    #[test]
    fn a_cycle_and_exec_are_errors_that_name_the_key_only() {
        let (_, e) = run(
            &[
                ("A", Raw::computed("${B}")),
                ("B", Raw::computed("ref(A)")),
                ("C", Raw::computed("exec(`op read x`)")),
            ],
            "development",
        );
        let keys: Vec<&str> = e.iter().map(|e| e.key.as_str()).collect();
        assert!(keys.contains(&"A") && keys.contains(&"C"), "{e:?}");
        assert!(
            e.iter()
                .any(|e| e.message.contains("refers back to itself"))
        );
    }

    #[test]
    fn the_process_environment_fills_a_reference_nothing_else_does() {
        let raw = [("URL".to_string(), Raw::computed("http://${HOSTNAME}"))].into();
        let outside = [("HOSTNAME".to_string(), "box".to_string())].into();
        let (v, _) = resolve(&raw, &outside, "development", &Values::new());
        assert_eq!(v["URL"], "http://box");
    }

    #[test]
    fn shell_defaults_follow_dotenv_expand() {
        let (v, e) = run(
            &[
                ("EMPTY", Raw::literal("")),
                ("A", Raw::computed("${EMPTY:-fallback}")),
                ("B", Raw::computed("${EMPTY-kept}")),
                ("C", Raw::computed("${UNSET-${UNSET_TOO:-deep}}")),
            ],
            "development",
        );
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(v["A"], "fallback");
        assert_eq!(
            v["B"], "",
            "a set but empty key keeps its empty value without the colon"
        );
        assert_eq!(v["C"], "deep");
    }

    #[test]
    fn penv_references_are_listed_then_read_from_what_was_fetched() {
        let raw: BTreeMap<String, Raw> = [
            (
                "A".to_string(),
                Raw::computed("penv(production/DATABASE_URL)"),
            ),
            (
                "B".to_string(),
                Raw::computed("fallback(penv(STRIPE_KEY), none)"),
            ),
        ]
        .into();
        assert_eq!(
            penv_addresses(&raw),
            ["STRIPE_KEY", "production/DATABASE_URL"]
        );
        let fetched: Values = [
            (
                "production/DATABASE_URL".to_string(),
                "postgres://prod.test".to_string(),
            ),
            ("STRIPE_KEY".to_string(), String::new()),
        ]
        .into();
        let (v, e) = resolve(&raw, &Values::new(), "development", &fetched);
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(v["A"], "postgres://prod.test");
        assert_eq!(v["B"], "none");
    }
    #[test]
    fn an_unset_reference_is_a_soft_error_naming_both_keys() {
        let (v, e) = run(
            &[
                ("PASSWORD", Raw::computed("pa$word")),
                ("QUIET", Raw::computed("fallback($NOPE, ok)")),
                ("DEFAULTED", Raw::computed("${NOPE:-ok}")),
            ],
            "development",
        );
        assert_eq!(v["PASSWORD"], "pa");
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].soft && e[0].key == "PASSWORD");
        assert!(
            !e[0].message.contains("word"),
            "the name is part of the value"
        );
    }

    #[test]
    fn a_chain_deeper_than_the_limit_fails_instead_of_crashing() {
        let mut pairs: Vec<(String, Raw)> = (0..1000)
            .map(|i| (format!("K{i}"), Raw::computed(format!("${{K{}}}", i + 1))))
            .collect();
        pairs.push(("K1000".into(), Raw::literal("end")));
        let raw = pairs.into_iter().collect();
        let (_, e) = resolve(&raw, &Values::new(), "development", &Values::new());
        assert!(
            e.iter().any(|e| e.message.contains("nest more than")),
            "{e:?}"
        );
    }

    #[test]
    fn a_varlock_plugin_call_is_refused_by_name_not_passed_through() {
        let (v, e) = run(
            &[("DB", Raw::computed("op(\"op://vault/db\")"))],
            "development",
        );
        assert!(!v.contains_key("DB"));
        assert!(
            e[0].message.contains("op() is not a function penv runs"),
            "{e:?}"
        );
        let (v, e) = run(
            &[("DB", Raw::literal("op(\"op://vault/db\")"))],
            "development",
        );
        assert!(e.is_empty());
        assert_eq!(v["DB"], "op(\"op://vault/db\")");
    }

    #[test]
    fn filters_run_left_to_right_and_an_unknown_one_is_named() {
        let (v, e) = run(
            &[
                ("PASS", Raw::literal("p@ss:w/rd ")),
                ("USER", Raw::literal("  Admin ")),
                (
                    "URL",
                    Raw::computed(
                        "postgres://${USER | trim | lower}:${PASS | trim | urlencode}@db/app",
                    ),
                ),
                ("AUTH", Raw::computed("Basic ${USER | trim | base64}")),
                ("DEF", Raw::computed("${NOPE:-Fallback | upper}")),
                ("BAD", Raw::computed("${PASS | rot13}")),
            ],
            "development",
        );
        assert_eq!(v["URL"], "postgres://admin:p%40ss%3Aw%2Frd@db/app");
        assert_eq!(v["AUTH"], "Basic QWRtaW4=");
        assert_eq!(v["DEF"], "FALLBACK");
        assert!(
            e.iter()
                .any(|e| e.key == "BAD" && e.message.contains("rot13")),
            "{e:?}"
        );
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b""), "");
    }

    #[test]
    fn match_picks_a_case_falls_back_to_underscore_and_fails_without_one() {
        let (v, e) = run(
            &[
                ("APP_ENV", Raw::literal("staging")),
                (
                    "API",
                    Raw::computed(
                        "match($APP_ENV, production: api.acme.com, staging: stg.acme.com, _: localhost:3000)",
                    ),
                ),
                (
                    "DEV",
                    Raw::computed("match(qa, production: a, _: http://localhost:3000)"),
                ),
                ("NONE", Raw::computed("match($APP_ENV, production: a)")),
            ],
            "staging",
        );
        assert_eq!(v["API"], "stg.acme.com");
        assert_eq!(v["DEV"], "http://localhost:3000");
        assert!(
            e.iter()
                .any(|e| e.key == "NONE" && e.message.contains("no _ case")),
            "{e:?}"
        );
        assert!(
            !e.iter().any(|e| e.message.contains("staging")),
            "the value is never named"
        );
    }

    #[test]
    fn penv_with_no_argument_reads_the_key_it_sits_on() {
        let raw: BTreeMap<String, Raw> = [
            ("DATABASE_URL".to_string(), Raw::computed("penv()")),
            ("OTHER".to_string(), Raw::computed("penv(production/OTHER)")),
        ]
        .into();
        assert_eq!(penv_addresses(&raw), ["DATABASE_URL", "production/OTHER"]);
        let fetched: Values = [
            ("DATABASE_URL".to_string(), "postgres://x".to_string()),
            ("production/OTHER".to_string(), "y".to_string()),
        ]
        .into();
        let out = resolve_full(&raw, &Values::new(), "development", &fetched);
        assert_eq!(out.values["DATABASE_URL"], "postgres://x");
        assert_eq!(out.values["OTHER"], "y");
    }

    #[test]
    fn a_fallback_taints_only_when_the_key_it_falls_back_from_is_used() {
        let raw: BTreeMap<String, Raw> = [
            (
                "REGION".to_string(),
                Raw::computed("${AWS_REGION:-us-east-1}"),
            ),
            ("TOKEN_OR".to_string(), Raw::computed("${TOKEN:-none}")),
        ]
        .into();
        let outside: Values = [("TOKEN".to_string(), "sk_live_1".to_string())].into();
        let out = resolve_full(&raw, &outside, "development", &Values::new());
        let hot = tainted(&out.deps, |k| k == "AWS_REGION" || k == "TOKEN");
        assert_eq!(out.values["REGION"], "us-east-1");
        assert_eq!(
            hot.iter().map(String::as_str).collect::<Vec<_>>(),
            ["TOKEN_OR"],
            "the unset key was only looked at; the set one was used"
        );
    }

    #[test]
    fn values_built_from_a_sensitive_key_are_tainted_transitively() {
        let raw: BTreeMap<String, Raw> = [
            ("DB_PASS".to_string(), Raw::literal("hunter2")),
            (
                "DB_URL".to_string(),
                Raw::computed("postgres://app:${DB_PASS | urlencode}@db"),
            ),
            ("POOL_URL".to_string(), Raw::computed("${DB_URL}?pool=5")),
            ("PORT".to_string(), Raw::literal("5432")),
            ("HOST".to_string(), Raw::computed("db:${PORT}")),
            (
                "REMOTE".to_string(),
                Raw::computed("penv(production/STRIPE_KEY)"),
            ),
        ]
        .into();
        let fetched: Values = [("production/STRIPE_KEY".to_string(), "sk".to_string())].into();
        let out = resolve_full(&raw, &Values::new(), "development", &fetched);
        let hot = tainted(&out.deps, |k| k == "DB_PASS" || k == "STRIPE_KEY");
        assert_eq!(
            hot.iter().map(String::as_str).collect::<Vec<_>>(),
            ["DB_URL", "POOL_URL", "REMOTE"]
        );

        // A secret that only picks a branch does not taint the branch it picks.
        let raw: BTreeMap<String, Raw> = [
            ("KEY".to_string(), Raw::literal("sk_live_1")),
            (
                "MODE".to_string(),
                Raw::computed("if(startsWith($KEY, sk_live_), live, test)"),
            ),
            ("PICK".to_string(), Raw::computed("match($KEY, _: fixed)")),
            ("COPY".to_string(), Raw::computed("if(true, $KEY, x)")),
        ]
        .into();
        let out = resolve_full(&raw, &Values::new(), "development", &Values::new());
        let hot = tainted(&out.deps, |k| k == "KEY");
        assert_eq!(hot.iter().map(String::as_str).collect::<Vec<_>>(), ["COPY"]);

        // A key first computed inside a condition keeps its own taint.
        let raw: BTreeMap<String, Raw> = [
            ("KEY".to_string(), Raw::literal("sk_live_1")),
            (
                "A_MODE".to_string(),
                Raw::computed("if(isEmpty($URL), a, b)"),
            ),
            ("URL".to_string(), Raw::computed("https://x/${KEY}")),
        ]
        .into();
        let out = resolve_full(&raw, &Values::new(), "development", &Values::new());
        let hot = tainted(&out.deps, |k| k == "KEY");
        assert_eq!(hot.iter().map(String::as_str).collect::<Vec<_>>(), ["URL"]);
    }

    #[test]
    fn and_or_and_prefix_checks() {
        let (v, _) = run(
            &[
                ("K", Raw::literal("sk_live_1")),
                (
                    "A",
                    Raw::computed("and(startsWith($K, sk_live_), not(endsWith($K, _test)))"),
                ),
                ("O", Raw::computed("or($NOPE, false, 0)")),
            ],
            "development",
        );
        assert_eq!((v["A"].as_str(), v["O"].as_str()), ("true", "false"));
    }

    #[test]
    fn a_braced_reference_as_an_argument_keeps_its_filters_and_default() {
        let (v, e) = run(
            &[
                ("LEVEL", Raw::literal("info")),
                (
                    "TAG",
                    Raw::computed("concat(api-, ${LEVEL | upper}, -, ${NOPE:-x})"),
                ),
            ],
            "development",
        );
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(v["TAG"], "api-INFO-x");
    }

    #[test]
    fn plain_text_is_not_an_expression() {
        assert!(!is_expression("hello (world)"));
        assert!(!is_expression("price is $5"));
        assert!(is_expression("concat(a,b)"));
        assert!(is_expression("${A}"));
    }
}
