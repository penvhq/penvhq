//! Values that are computed: `${KEY}` expansion and the varlock functions penv
//! runs. Pure: the caller hands in every raw value and the process environment.

use std::collections::{BTreeMap, BTreeSet};

use crate::functions::{self, Arg, Call, Policy, Taint, check_args};
use crate::ir::is_valid_key_name;
use crate::validate::Values;

/// The functions penv evaluates. `exec` is refused by name: a schema never
/// starts a process.
pub const FUNCTIONS: [&str; 19] = [
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
    "url",
    "json",
    "hash",
    "raw",
];

/// Filters a `${KEY | filter}` reference runs, left to right.
pub const FILTERS: [&str; 6] = ["urlencode", "base64", "lower", "upper", "trim", "b64d"];

/// A dependency [`tainted`] always treats as sensitive: a URL's password, say.
pub const ALWAYS_SENSITIVE: &str = "\0sensitive";

/// How deep references may chain, and calls or defaults nest inside one value,
/// before penv stops rather than exhaust the stack.
pub const MAX_DEPTH: usize = 128;

/// Nested evaluations across a whole chain of references, for the same reason.
const MAX_NESTING: usize = 4 * MAX_DEPTH;

/// The stack the depth limits are sized for, in every build on every platform.
/// Resolve on a thread this large: Windows gives its main thread 1 MiB.
pub const STACK: usize = 16 << 20;

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
    policy: &Policy,
) -> (Values, Vec<ResolveError>) {
    let out = resolve_full(raw, outside, environment, fetched, policy);
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
    policy: &Policy,
) -> Resolution {
    let mut run = Run {
        raw,
        outside,
        environment,
        fetched,
        policy,
        done: Values::new(),
        failed: BTreeSet::new(),
        stack: Vec::new(),
        errors: Vec::new(),
        quiet: 0,
        deps: BTreeMap::new(),
        control: 0,
        nesting: 0,
        too_deep: None,
    };
    for name in raw.keys() {
        run.too_deep = None;
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
        // `penv(env/KEY)` inherits KEY's sensitivity; another project's key is
        // one penv cannot judge, so it is sensitive.
        Some(address) => {
            address.split('/').count() > 2
                || sensitive(address.rsplit('/').next().unwrap_or(address))
        }
        None => dep == ALWAYS_SENSITIVE || sensitive(dep),
    };
    let mut readers: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<&str> = Vec::new();
    for (key, from) in deps {
        for dep in from {
            readers.entry(dep.as_str()).or_default().push(key.as_str());
        }
        if from.iter().any(|d| source(d)) && out.insert(key.clone()) {
            queue.push(key.as_str());
        }
    }
    while let Some(key) = queue.pop() {
        for &reader in readers.get(key).into_iter().flatten() {
            if out.insert(reader.to_string()) {
                queue.push(reader);
            }
        }
    }
    out
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
                (true, [Arg::Subject(Expr::Text(address))]) => {
                    out.push(address.trim().to_string());
                }
                (true, []) => out.push(key.to_string()),
                _ => {}
            }
            collect_args(args, key, out);
        }
        Expr::Template(parts) => parts.iter().for_each(|p| collect(p, key, out)),
        Expr::Default { or, .. } => collect(or, key, out),
        Expr::Filter { inner, filters } => {
            collect(inner, key, out);
            filters
                .iter()
                .for_each(|(_, args)| collect_args(args, key, out));
        }
        Expr::Text(_) | Expr::Ref(_) => {}
    }
}

fn collect_args(args: &[Arg], key: &str, out: &mut Vec<String>) {
    for arg in args {
        match arg {
            Arg::Subject(e) | Arg::Named(_, e) => collect(e, key, out),
            Arg::Address(_) => {}
        }
    }
}

/// A parsed value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Text(String),
    Ref(String),
    /// `${KEY:-or}` (empty or unset) and `${KEY-or}` (unset only).
    Default {
        key: String,
        or: Box<Expr>,
        when_empty: bool,
    },
    /// `${KEY | urlencode | json(path)}`; a filter with arguments has its subject implied.
    Filter {
        inner: Box<Expr>,
        filters: Vec<(String, Vec<Arg>)>,
    },
    Template(Vec<Expr>),
    Call(String, Vec<Arg>),
}

struct Run<'a> {
    raw: &'a BTreeMap<String, Raw>,
    outside: &'a Values,
    environment: &'a str,
    fetched: &'a Values,
    policy: &'a Policy,
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
    nesting: usize,
    /// A chain ran too deep: only the key it started from fails, whatever the order.
    too_deep: Option<String>,
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
            self.too_deep = Some(format!("references nest more than {MAX_DEPTH} deep"));
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
            Err(_) if self.too_deep.is_some() && !self.stack.is_empty() => None,
            Err(message) => {
                let message = self.too_deep.take().unwrap_or(message);
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

    // One small function per step: a debug build gives every local its own slot.
    fn eval(&mut self, expr: &Expr) -> Result<String, String> {
        if self.nesting >= MAX_NESTING {
            return Err(self.nested_too_deep());
        }
        self.nesting += 1;
        let out = match expr {
            Expr::Text(t) => Ok(t.clone()),
            Expr::Ref(name) => self.reference(name),
            Expr::Default {
                key,
                or,
                when_empty,
            } => self.default(key, or, *when_empty),
            Expr::Filter { inner, filters } => self.filter(inner, filters),
            Expr::Template(parts) => self.join(parts),
            Expr::Call(name, args) => self.call(name, args),
        };
        self.nesting -= 1;
        out
    }

    #[cold]
    fn nested_too_deep(&mut self) -> String {
        let message = format!("expressions nest more than {MAX_NESTING} deep");
        self.too_deep = Some(message.clone());
        message
    }

    fn reference(&mut self, name: &str) -> Result<String, String> {
        let found = self.value(name);
        if found.is_none() && self.raw.contains_key(name) {
            return Err(format!("{name} could not be computed"));
        }
        self.depend(name);
        if found.is_none() && self.quiet == 0 {
            self.unset(name);
        }
        Ok(found.unwrap_or_default())
    }

    fn default(&mut self, key: &str, or: &Expr, when_empty: bool) -> Result<String, String> {
        let set = self.raw.contains_key(key) || self.outside.contains_key(key);
        // Looking is control: only a value that is used is a dependency,
        // so a fallback taken does not inherit the key's sensitivity.
        self.control += 1;
        let value = self.quietly(&Expr::Ref(key.to_string()));
        self.control -= 1;
        let value = value?;
        if !set || (when_empty && value.is_empty()) {
            self.eval(or)
        } else {
            self.depend(key);
            Ok(value)
        }
    }

    fn filter(&mut self, inner: &Expr, filters: &[(String, Vec<Arg>)]) -> Result<String, String> {
        let key = self.stack.last().cloned().unwrap_or_default();
        let mut subject_key = match inner {
            Expr::Ref(k) => Some(k.as_str()),
            _ => None,
        };
        let mut public = 0;
        for (i, (name, args)) in filters.iter().enumerate() {
            if word_takes_args(name) {
                match functions::taint(name, address(args), subject_key, &key, self.policy) {
                    Taint::Public => public = i + 1,
                    Taint::Always => self.depend(ALWAYS_SENSITIVE),
                    Taint::Inputs => {}
                }
            }
            subject_key = None;
        }
        // A public step declassifies what it reads: everything up to it is control.
        self.control += usize::from(public > 0);
        let mut value = self.eval(inner);
        for (i, (name, args)) in filters.iter().enumerate() {
            value = value.and_then(|v| {
                if word_takes_args(name) {
                    self.function(name, args, Some(v))
                } else {
                    apply_filter(name, &v)
                }
            });
            if i + 1 == public {
                self.control -= 1;
            }
        }
        value
    }

    fn join(&mut self, parts: &[Expr]) -> Result<String, String> {
        let mut out = String::new();
        for part in parts {
            out.push_str(&self.eval(part)?);
        }
        Ok(out)
    }

    fn call(&mut self, name: &str, args: &[Arg]) -> Result<String, String> {
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

    fn call_inner(&mut self, name: &str, args: &[Arg]) -> Result<String, String> {
        if matches!(name, "url" | "json" | "hash") {
            return self.tainting(name, args);
        }
        let args = plain(args)?;
        let args = args.as_slice();
        match name {
            "ref" => self.ref_key(args),
            "concat" => self.concat(args),
            "fallback" => self.fallback(args),
            "if" => self.branch(args),
            "eq" => self.equal(args),
            "not" => self.negate(args),
            "isEmpty" => self.empty(args),
            "penv" => self.penv(args),
            "forEnv" => self.for_env(args),
            "match" => self.match_case(args),
            "and" | "or" => self.all_or_any(name == "and", args),
            "startsWith" | "endsWith" => self.affix(name == "startsWith", args),
            "random" => {
                Err("random() is generated by penv run, which keeps the value in .env.local".into())
            }
            ":" => Err(CASE_OUTSIDE_MATCH.into()),
            _ => Err(format!("{name}() is not a function penv runs")),
        }
    }

    /// `url`, `json` or `hash` written as a call, its taint decided first.
    fn tainting(&mut self, name: &str, args: &[Arg]) -> Result<String, String> {
        let subject_key = match args.first() {
            Some(Arg::Subject(Expr::Ref(k))) => Some(k.as_str()),
            _ => None,
        };
        let key = self.stack.last().cloned().unwrap_or_default();
        let taint = functions::taint(name, address(args), subject_key, &key, self.policy);
        match taint {
            Taint::Always => self.depend(ALWAYS_SENSITIVE),
            Taint::Public => self.control += 1,
            Taint::Inputs => {}
        }
        let out = self.function(name, args, None);
        if taint == Taint::Public {
            self.control -= 1;
        }
        out
    }

    /// Evaluate the arguments, then the function. `piped` is a filter's implied subject.
    fn function(
        &mut self,
        name: &str,
        args: &[Arg],
        piped: Option<String>,
    ) -> Result<String, String> {
        let mut call = Call {
            name,
            subject: piped,
            address: address(args),
            options: Vec::new(),
            raw: BTreeSet::new(),
            quoted: BTreeSet::new(),
        };
        for arg in args {
            match arg {
                Arg::Subject(e) => call.subject = Some(self.eval(e)?),
                Arg::Named(label, Expr::Call(raw, inner)) if raw == "raw" => {
                    let [Arg::Subject(e)] = inner.as_slice() else {
                        return Err(RAW_TAKES_ONE.into());
                    };
                    call.options.push((label, self.eval(e)?));
                    call.raw.insert(label);
                }
                Arg::Named(label, Expr::Call(q, inner)) if q == QUOTED => {
                    let [Arg::Subject(e)] = inner.as_slice() else {
                        unreachable!("a quoted option holds one value");
                    };
                    call.options.push((label, self.eval(e)?));
                    call.quoted.insert(label);
                }
                Arg::Named(label, e) => call.options.push((label, self.eval(e)?)),
                Arg::Address(_) => {}
            }
        }
        functions::eval(&call)
    }

    fn ref_key(&mut self, args: &[&Expr]) -> Result<String, String> {
        arity("ref", args, 1, 1)?;
        match args[0] {
            Expr::Text(key) | Expr::Ref(key) => self.reference(key),
            _ => Err("ref() takes a key name".into()),
        }
    }

    fn concat(&mut self, args: &[&Expr]) -> Result<String, String> {
        let mut out = String::new();
        for arg in args {
            out.push_str(&self.eval(arg)?);
        }
        Ok(out)
    }

    fn fallback(&mut self, args: &[&Expr]) -> Result<String, String> {
        arity("fallback", args, 1, usize::MAX)?;
        for arg in args {
            let v = self.quietly(arg)?;
            if !v.is_empty() {
                return Ok(v);
            }
        }
        Ok(String::new())
    }

    fn branch(&mut self, args: &[&Expr]) -> Result<String, String> {
        arity("if", args, 2, 3)?;
        if truthy(&self.deciding(args[0])?) {
            self.eval(args[1])
        } else {
            args.get(2).map_or(Ok(String::new()), |a| self.eval(a))
        }
    }

    fn equal(&mut self, args: &[&Expr]) -> Result<String, String> {
        arity("eq", args, 2, 2)?;
        let left = self.eval(args[0])?;
        Ok(flag(left == self.eval(args[1])?))
    }

    fn negate(&mut self, args: &[&Expr]) -> Result<String, String> {
        arity("not", args, 1, 1)?;
        Ok(flag(!truthy(&self.eval(args[0])?)))
    }

    fn empty(&mut self, args: &[&Expr]) -> Result<String, String> {
        arity("isEmpty", args, 1, 1)?;
        Ok(flag(self.quietly(args[0])?.is_empty()))
    }

    fn penv(&mut self, args: &[&Expr]) -> Result<String, String> {
        arity("penv", args, 0, 1)?;
        // A computed address is a value, and the message below names it.
        let address = match args.first() {
            Some(Expr::Text(address)) => address.trim().to_string(),
            Some(_) => {
                return Err(
                    "penv() takes a written address, such as production/DATABASE_URL, not a computed one"
                        .into(),
                );
            }
            None => self.stack.last().cloned().unwrap_or_default(),
        };
        self.depend(&format!("penv:{address}"));
        self.fetched.get(&address).cloned().ok_or_else(|| {
            format!("penv({address}) takes a written address, such as production/DATABASE_URL")
        })
    }

    fn for_env(&mut self, args: &[&Expr]) -> Result<String, String> {
        arity("forEnv", args, 1, usize::MAX)?;
        let mut hit = false;
        for arg in args {
            hit |= self.eval(arg)? == self.environment;
        }
        Ok(flag(hit))
    }

    fn match_case(&mut self, args: &[&Expr]) -> Result<String, String> {
        arity("match", args, 2, usize::MAX)?;
        let subject = self.deciding(args[0])?;
        let mut fallback = None;
        for arm in &args[1..] {
            let Expr::Call(kind, parts) = arm else {
                return Err("match() takes cases written label: value".into());
            };
            let (":", [Arg::Subject(label), Arg::Subject(value)]) =
                (kind.as_str(), parts.as_slice())
            else {
                return Err("match() takes cases written label: value".into());
            };
            let label = self.deciding(label)?;
            if label == "_" {
                fallback = Some(value);
            } else if label == subject {
                return self.eval(value);
            }
        }
        match fallback {
            Some(expr) => self.eval(expr),
            None => Err("match() found no case for the value and has no _ case".into()),
        }
    }

    fn all_or_any(&mut self, want: bool, args: &[&Expr]) -> Result<String, String> {
        arity(if want { "and" } else { "or" }, args, 1, usize::MAX)?;
        for arg in args {
            if truthy(&self.quietly(arg)?) != want {
                return Ok(flag(!want));
            }
        }
        Ok(flag(want))
    }

    fn affix(&mut self, starts: bool, args: &[&Expr]) -> Result<String, String> {
        arity(if starts { "startsWith" } else { "endsWith" }, args, 2, 2)?;
        let value = self.eval(args[0])?;
        let part = self.eval(args[1])?;
        Ok(flag(if starts {
            value.starts_with(&part)
        } else {
            value.ends_with(&part)
        }))
    }
}

fn arity(name: &str, args: &[&Expr], min: usize, max: usize) -> Result<(), String> {
    if (min..=max).contains(&args.len()) {
        return Ok(());
    }
    let count = if min == max {
        min.to_string()
    } else {
        format!("{min} to {max}")
    };
    Err(format!(
        "{name}() takes {count} argument(s), not {}",
        args.len()
    ))
}

/// The arguments of a function that takes them by position.
fn plain(args: &[Arg]) -> Result<Vec<&Expr>, String> {
    args.iter()
        .map(|a| match a {
            Arg::Subject(e) => Ok(e),
            Arg::Address(_) | Arg::Named(..) => {
                Err("this function takes its arguments by position".to_string())
            }
        })
        .collect()
}

/// Whether `name` takes the argument form: subject, address, `name=value`.
fn word_takes_args(name: &str) -> bool {
    functions::args::word(name).is_some()
}

fn address(args: &[Arg]) -> Option<&str> {
    args.iter().find_map(|a| match a {
        Arg::Address(at) => Some(at.as_str()),
        _ => None,
    })
}

/// Wraps a quoted json() option value, so writing keeps it a string.
const QUOTED: &str = "'";
const RAW_TAKES_ONE: &str = "raw() takes the one value to embed, such as raw($INNER)";

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

const CASE_OUTSIDE_MATCH: &str =
    "label: value cases belong in match(); quote the text to pass it as written";

fn parse(text: &str) -> Result<Expr, String> {
    parse_at(text, 0)
}

fn too_deep(depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        Err(format!("expressions nest more than {MAX_DEPTH} deep"))
    } else {
        Ok(())
    }
}

fn parse_at(text: &str, depth: usize) -> Result<Expr, String> {
    too_deep(depth)?;
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
        if name == "raw" {
            return Err(
                "raw() embeds JSON only as a json() option value, such as json(db=raw($INNER))"
                    .into(),
            );
        }
        let inner = &text[name.len() + 1..text.len() - 1];
        let args = call_args(name, inner, depth)?;
        if functions::args::word(name).is_some() {
            check_args(name, &args)?;
        }
        return Ok(Expr::Call(name.to_string(), args));
    }
    template(text, depth)
}

/// A call's arguments. The argument-form functions read `name=value`, a subject
/// and an address; the others take theirs by position.
fn call_args(name: &str, inner: &str, depth: usize) -> Result<Vec<Arg>, String> {
    let named_form = functions::args::word(name);
    let mut args = Vec::new();
    for text in split(inner)? {
        let text = text.trim();
        if text.is_empty() {
            return Err(format!(
                "{name}() has an empty argument; remove the extra comma"
            ));
        }
        let Some(word) = named_form else {
            args.push(Arg::Subject(arg(text, depth + 1)?));
            continue;
        };
        if let Some((label, value)) = functions::args::named(text) {
            if !functions::args::is_label(label) {
                return Err(format!(
                    "{name}() argument names are letters, digits, dots and [index]; quote text that holds an ="
                ));
            }
            let value = match call_name(value) {
                Some("raw") if name == "json" => {
                    let inner = &value[4..value.len() - 1];
                    let raw = call_args("raw", inner, depth + 1)?;
                    if !matches!(raw.as_slice(), [Arg::Subject(_)]) {
                        return Err(RAW_TAKES_ONE.into());
                    }
                    Expr::Call("raw".into(), raw)
                }
                _ if name == "json" && value.starts_with(['"', '\'']) => {
                    Expr::Call(QUOTED.into(), vec![Arg::Subject(arg(value, depth + 1)?)])
                }
                _ => arg(value, depth + 1)?,
            };
            args.push(Arg::Named(label.to_string(), value));
            continue;
        }
        let bare = !text.starts_with(['$', '"', '\'']) && call_name(text).is_none();
        if bare && word.address.is_some() {
            args.push(Arg::Address(text.to_string()));
        } else {
            args.push(Arg::Subject(arg(text, depth + 1)?));
        }
    }
    let case = |e: &Expr| matches!(e, Expr::Call(c, _) if c == ":");
    let stray_case = args.iter().any(|a| match a {
        Arg::Subject(e) | Arg::Named(_, e) => case(e),
        Arg::Address(_) => false,
    });
    if name != "match" && stray_case {
        return Err(CASE_OUTSIDE_MATCH.into());
    }
    Ok(args)
}

/// One `| step` of a braced reference: a filter, or a function with its subject implied.
fn filter_step(text: &str, depth: usize) -> Result<(String, Vec<Arg>), String> {
    let text = text.trim();
    if FILTERS.contains(&text) {
        return Ok((text.to_string(), Vec::new()));
    }
    let name = call_name(text).ok_or_else(|| unknown_filter(text))?;
    let piped = functions::args::word(name).is_some_and(|w| w.subject != "none");
    if !piped {
        return Err(unknown_filter(name));
    }
    let args = call_args(name, &text[name.len() + 1..text.len() - 1], depth)?;
    let mut implied = vec![Arg::Subject(Expr::Text(String::new()))];
    implied.extend(args.iter().cloned());
    check_args(name, &implied)?;
    Ok((name.to_string(), args))
}

fn arg(text: &str, depth: usize) -> Result<Expr, String> {
    too_deep(depth)?;
    let text = text.trim();
    // A `label: value` case for match(): the first `:` outside quotes.
    if let Some(colon) = case_colon(text) {
        let label = arg(&text[..colon], depth + 1)?;
        let value = arg(&text[colon + 1..], depth + 1)?;
        return Ok(Expr::Call(
            ":".into(),
            vec![Arg::Subject(label), Arg::Subject(value)],
        ));
    }
    if let Some(q) = text.chars().next().filter(|c| *c == '"' || *c == '\'') {
        if text.len() >= 2 && text.ends_with(q) {
            let inner = &text[1..text.len() - 1];
            return if q == '"' {
                template(inner, depth)
            } else {
                Ok(Expr::Text(inner.into()))
            };
        }
        return Err("a quoted argument is never closed".into());
    }
    if let Some(name) = text.strip_prefix('$')
        && is_valid_key_name(name)
    {
        return Ok(Expr::Ref(name.to_string()));
    }
    if call_name(text).is_some() {
        return parse_at(text, depth);
    }
    template(text, depth)
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
        "b64d" => base64_decode(value)?,
        other => return Err(unknown_filter(other)),
    })
}

fn unknown_filter(name: &str) -> String {
    format!(
        "{name} is not a filter penv runs; filters are {}",
        FILTERS.join(", ")
    )
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

/// Standard base64, padded or not. The failure never quotes the value.
fn base64_decode(text: &str) -> Result<String, String> {
    const BAD: &str = "b64d: the value is not standard base64";
    let text = text.trim().trim_end_matches('=');
    let mut bytes = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(BAD.into()),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    if bits >= 6 {
        return Err(BAD.into());
    }
    String::from_utf8(bytes).map_err(|_| "b64d: the decoded value is not text".into())
}

/// `${KEY}` and `$KEY` inside text. `\$` is a literal dollar, as in
/// dotenv-expand, and so is a `$` that no name follows.
fn template(text: &str, depth: usize) -> Result<Expr, String> {
    too_deep(depth)?;
    let chars: Vec<char> = text.chars().collect();
    // Each `{`'s matching `}`, found in one pass rather than a scan per `${`.
    let mut closes = vec![None; chars.len()];
    let mut open = Vec::new();
    for (i, c) in chars.iter().enumerate() {
        match c {
            '{' => open.push(i),
            '}' => {
                if let Some(o) = open.pop() {
                    closes[o] = Some(i);
                }
            }
            _ => {}
        }
    }
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
            && let Some(end) = closes[i + 1]
        {
            let whole: String = chars[i + 2..end].iter().collect();
            let mut pieces = top_level_pipes(&whole).into_iter();
            let inner = pieces.next().unwrap_or_default().trim().to_string();
            let filters = pieces
                .map(|f| filter_step(&f, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
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
                    or: Box::new(template(&inner[at + width..], depth + 1)?),
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
    Ok(match parts.len() {
        0 => Expr::Text(String::new()),
        1 if matches!(parts[0], Expr::Text(_)) => parts.remove(0),
        _ => Expr::Template(parts),
    })
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

/// Split call arguments on commas outside quotes and brackets. A quote right
/// after a letter or digit is an apostrophe (`it's`), not an opening quote.
fn split(inner: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut prev: Option<char> = None;
    for c in inner.chars() {
        let after_word = prev.is_some_and(char::is_alphanumeric);
        prev = Some(c);
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' if !after_word => {
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
        resolve(
            &raw,
            &Values::new(),
            env,
            &Values::new(),
            &Policy::default(),
        )
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
        let (v, _) = resolve(
            &raw,
            &outside,
            "development",
            &Values::new(),
            &Policy::default(),
        );
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
        let (v, e) = resolve(
            &raw,
            &Values::new(),
            "development",
            &fetched,
            &Policy::default(),
        );
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
        let (_, e) = resolve(
            &raw,
            &Values::new(),
            "development",
            &Values::new(),
            &Policy::default(),
        );
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
        let out = resolve_full(
            &raw,
            &Values::new(),
            "development",
            &fetched,
            &Policy::default(),
        );
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
        let out = resolve_full(
            &raw,
            &outside,
            "development",
            &Values::new(),
            &Policy::default(),
        );
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
        let out = resolve_full(
            &raw,
            &Values::new(),
            "development",
            &fetched,
            &Policy::default(),
        );
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
        let out = resolve_full(
            &raw,
            &Values::new(),
            "development",
            &Values::new(),
            &Policy::default(),
        );
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
        let out = resolve_full(
            &raw,
            &Values::new(),
            "development",
            &Values::new(),
            &Policy::default(),
        );
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
    fn a_reference_inside_an_unquoted_argument_is_expanded() {
        let (v, e) = run(
            &[
                ("APP_ENV", Raw::literal("production")),
                ("API_HOST", Raw::literal("api.test")),
                (
                    "URL",
                    Raw::computed(
                        "if(eq($APP_ENV, production), https://${API_HOST}/v1, http://localhost)",
                    ),
                ),
                ("JOINED", Raw::computed("concat(http://$API_HOST, /x)")),
                ("DOTTED", Raw::computed("concat($API_HOST.v1)")),
            ],
            "production",
        );
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(v["URL"], "https://api.test/v1");
        assert_eq!(v["JOINED"], "http://api.test/x");
        assert_eq!(v["DOTTED"], "api.test.v1");
    }

    #[test]
    fn a_computed_penv_address_is_refused_without_its_value() {
        let raw: BTreeMap<String, Raw> = [
            ("SECRET".to_string(), Raw::literal("fake-secret-value")),
            ("COPY".to_string(), Raw::computed("penv($SECRET)")),
        ]
        .into();
        assert!(penv_addresses(&raw).is_empty());
        let (v, e) = resolve(
            &raw,
            &Values::new(),
            "development",
            &Values::new(),
            &Policy::default(),
        );
        assert!(!v.contains_key("COPY"));
        let error = e.iter().find(|e| e.key == "COPY").expect("COPY fails");
        assert!(!error.message.contains("fake-secret-value"), "{error:?}");
    }

    #[test]
    fn deep_nesting_in_one_value_fails_instead_of_overflowing() {
        let calls = format!("{}x{}", "not(".repeat(20_000), ")".repeat(20_000));
        let defaults = format!("{}x{}", "${A:-".repeat(20_000), "}".repeat(20_000));
        let (v, e) = std::thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                run(
                    &[
                        ("CALLS", Raw::computed(calls)),
                        ("DEFAULTS", Raw::computed(defaults)),
                        (
                            "FINE",
                            Raw::computed(format!("{}x{}", "not(".repeat(100), ")".repeat(100))),
                        ),
                    ],
                    "development",
                )
            })
            .unwrap()
            .join()
            .expect("no stack overflow");
        for key in ["CALLS", "DEFAULTS"] {
            let error = e.iter().find(|e| e.key == key).expect(key);
            assert!(error.message.contains("nest more than"), "{error:?}");
        }
        assert_eq!(v["FINE"], "true");
    }

    #[test]
    fn a_long_chain_of_deep_values_fails_instead_of_overflowing() {
        let nested = |next: &str| format!("{}${next}{}", "concat(".repeat(120), ")".repeat(120));
        let mut pairs: Vec<(String, Raw)> = (0..200)
            .map(|i| {
                (
                    format!("K{i:03}"),
                    Raw::computed(nested(&format!("K{:03}", i + 1))),
                )
            })
            .collect();
        pairs.push(("K200".into(), Raw::literal("end")));
        let raw = pairs.into_iter().collect();
        let (_, e) = std::thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                resolve(
                    &raw,
                    &Values::new(),
                    "development",
                    &Values::new(),
                    &Policy::default(),
                )
            })
            .unwrap()
            .join()
            .expect("no stack overflow");
        assert!(e.iter().any(|e| e.key == "K000"), "{e:?}");
    }

    #[test]
    fn whether_a_chain_is_too_deep_does_not_depend_on_the_order_keys_are_read() {
        let mut pairs: Vec<(String, Raw)> = (0..50)
            .map(|i| {
                (
                    format!("A{i:02}"),
                    Raw::computed(format!("${{A{:02}}}", i + 1)),
                )
            })
            .collect();
        pairs.push(("A50".into(), Raw::computed("${B001}")));
        pairs.extend((1..100).map(|i| {
            (
                format!("B{i:03}"),
                Raw::computed(format!("${{B{:03}}}", i + 1)),
            )
        }));
        pairs.push(("B100".into(), Raw::literal("end")));
        let raw = pairs.into_iter().collect();
        let (v, e) = resolve(
            &raw,
            &Values::new(),
            "development",
            &Values::new(),
            &Policy::default(),
        );
        assert_eq!(v.get("B001").map(String::as_str), Some("end"), "{e:?}");
        assert!(!e.iter().any(|e| e.key.starts_with('B')), "{e:?}");
        assert!(e.iter().any(|e| e.key == "A00"), "{e:?}");
    }

    #[test]
    fn an_unknown_filter_fails_even_on_a_branch_not_taken() {
        let (_, e) = run(
            &[
                ("A", Raw::literal("x")),
                ("B", Raw::computed("if(false, ${A | rot13}, x)")),
            ],
            "development",
        );
        assert!(
            e.iter()
                .any(|e| e.key == "B" && e.message.contains("rot13")),
            "{e:?}"
        );
    }

    #[test]
    fn an_address_penv_cannot_judge_is_sensitive() {
        let deps: BTreeMap<String, BTreeSet<String>> = [
            ("FAR", "penv:acme/api/production/PORT"),
            ("PROJECT", "penv:api/production/PORT"),
            ("NEAR", "penv:production/PORT"),
            ("SAME", "penv:PORT"),
        ]
        .into_iter()
        .map(|(k, d)| (k.to_string(), [d.to_string()].into()))
        .collect();
        let hot = tainted(&deps, |k| k != "PORT");
        assert_eq!(
            hot.iter().map(String::as_str).collect::<Vec<_>>(),
            ["FAR", "PROJECT"]
        );
    }

    #[test]
    fn taint_follows_a_long_chain() {
        let mut deps: BTreeMap<String, BTreeSet<String>> = (0..5_000)
            .map(|i| (format!("K{i}"), [format!("K{}", i + 1)].into()))
            .collect();
        deps.insert("K5000".into(), ["SECRET".to_string()].into());
        deps.insert("CLEAN".into(), ["PORT".to_string()].into());
        let hot = tainted(&deps, |k| k == "SECRET");
        assert_eq!(hot.len(), 5_001);
        assert!(!hot.contains("CLEAN"));
    }

    #[test]
    fn a_case_outside_match_and_an_apostrophe_in_a_word_read_sensibly() {
        let (v, e) = run(
            &[
                ("CASE", Raw::computed("if(false, x, Error: none)")),
                ("WORD", Raw::computed("concat(it's, \" fine\")")),
            ],
            "development",
        );
        let error = e.iter().find(|e| e.key == "CASE").expect("CASE fails");
        assert!(error.message.contains("belong in match()"), "{error:?}");
        assert_eq!(v["WORD"], "it's fine", "{e:?}");
    }

    #[test]
    fn plain_text_is_not_an_expression() {
        assert!(!is_expression("hello (world)"));
        assert!(!is_expression("price is $5"));
        assert!(is_expression("concat(a,b)"));
        assert!(is_expression("${A}"));
    }

    fn text(t: &str) -> Expr {
        Expr::Text(t.into())
    }

    fn key(k: &str) -> Expr {
        Expr::Ref(k.into())
    }

    fn named(label: &str, value: Expr) -> Arg {
        Arg::Named(label.into(), value)
    }

    #[test]
    fn calls_take_a_subject_an_address_and_named_options() {
        let call = |name: &str, args: Vec<Arg>| Expr::Call(name.into(), args);
        assert_eq!(
            parse("url($X, host)").unwrap(),
            call(
                "url",
                vec![Arg::Subject(key("X")), Arg::Address("host".into())]
            )
        );
        assert_eq!(
            parse("url($X, host=h, port=5433)").unwrap(),
            call(
                "url",
                vec![
                    Arg::Subject(key("X")),
                    named("host", text("h")),
                    named("port", text("5433")),
                ]
            )
        );
        assert_eq!(
            parse("json(a.b=1, c=raw($I))").unwrap(),
            call(
                "json",
                vec![
                    named("a.b", text("1")),
                    named("c", call("raw", vec![Arg::Subject(key("I"))])),
                ]
            )
        );
        assert_eq!(
            parse("hash($K, algo=sha256)").unwrap(),
            call(
                "hash",
                vec![Arg::Subject(key("K")), named("algo", text("sha256"))]
            )
        );
        assert_eq!(
            parse(r#"json($S, ["a.b"]=x)"#).unwrap(),
            call(
                "json",
                vec![Arg::Subject(key("S")), named(r#"["a.b"]"#, text("x"))]
            )
        );
        assert_eq!(
            parse("concat(a=b, c)").unwrap(),
            call(
                "concat",
                vec![Arg::Subject(text("a=b")), Arg::Subject(text("c"))]
            )
        );
    }

    #[test]
    fn a_filter_with_arguments_implies_its_subject() {
        assert_eq!(
            parse("${K | json(a) | url(host)}").unwrap(),
            Expr::Template(vec![Expr::Filter {
                inner: Box::new(key("K")),
                filters: vec![
                    ("json".into(), vec![Arg::Address("a".into())]),
                    ("url".into(), vec![Arg::Address("host".into())]),
                ],
            }])
        );
        assert!(
            parse("${K | url(nope)}")
                .unwrap_err()
                .contains("no part named by the bare argument")
        );
        assert!(parse("${K | random(length=8)}").is_err());
    }

    #[test]
    fn a_repeated_or_unknown_option_fails_naming_option_and_key() {
        let fake = Raw::literal("fake_api_key_123");
        for (value, says) in [
            ("hash($K, algo=sha256, algo=sha512)", "algo twice"),
            ("hash($K, algoo=sha256)", "no option algoo"),
            ("hash($K)", "needs algo="),
            ("hash($K, algo=md5)", "option algo takes one of"),
            ("url($K, host, port=1)", "never both"),
            ("url($K, origin=x)", "cannot write"),
            ("url(host)", "needs a subject"),
            ("random(32)", "name=value"),
            ("raw($K)", "json() option value"),
        ] {
            let (_, errors) = run(&[("K", fake.clone()), ("OUT", Raw::computed(value))], "dev");
            let error = errors.iter().find(|e| e.key == "OUT").expect(value);
            assert!(error.message.starts_with("OUT: "), "{}", error.message);
            assert!(error.message.contains(says), "{value}: {}", error.message);
            assert!(!error.message.contains("fake_api_key"), "{}", error.message);
        }
    }

    #[test]
    fn argument_shape_fails_at_parse_without_naming_a_value() {
        let fake = Raw::literal("fake_api_key_123");
        for (value, says) in [
            (
                r#"json({"password":"fakepw"})"#,
                "reads the bare argument from a subject",
            ),
            (
                "url($K, fakepw)",
                "no part named by the bare argument; parts are",
            ),
            (
                "json($K, )",
                "json() has an empty argument; remove the extra comma",
            ),
            ("url($K, )", "url() has an empty argument"),
            ("json(, a=1)", "json() has an empty argument"),
            (
                "hash($K, algo=hmac-sha256)",
                "hash() algo hmac-sha256 needs key=",
            ),
            ("json(a=raw())", RAW_TAKES_ONE),
            ("json(a=raw($K, $K))", RAW_TAKES_ONE),
        ] {
            let (_, errors) = run(&[("K", fake.clone()), ("OUT", Raw::computed(value))], "dev");
            let error = errors.iter().find(|e| e.key == "OUT").expect(value);
            assert!(error.message.contains(says), "{value}: {}", error.message);
            assert!(!error.message.contains("fakepw"), "{}", error.message);
            assert!(!error.message.contains("fake_api_key"), "{}", error.message);
        }
        assert!(parse("hash($K, algo=hmac-sha256, key=$K)").is_ok());
    }

    #[test]
    fn a_quoted_json_option_is_marked_quoted() {
        assert_eq!(
            parse("json(a='25')").unwrap(),
            Expr::Call(
                "json".into(),
                vec![named(
                    "a",
                    Expr::Call(QUOTED.into(), vec![Arg::Subject(text("25"))])
                )]
            )
        );
        assert_eq!(
            parse("json(a=25)").unwrap(),
            Expr::Call("json".into(), vec![named("a", text("25"))])
        );
    }

    #[test]
    fn b64d_decodes_and_names_the_key_when_it_cannot() {
        let (v, e) = run(
            &[
                ("A", Raw::literal("ZmFrZS12YWx1ZQ==")),
                ("B", Raw::computed("${A | b64d}")),
                ("C", Raw::literal("not base64!")),
                ("D", Raw::computed("${C | b64d}")),
            ],
            "dev",
        );
        assert_eq!(v["B"], "fake-value");
        assert_eq!(e.len(), 1);
        assert!(e[0].message.starts_with("D: b64d"), "{}", e[0].message);
        assert_eq!(
            base64_decode(&base64(b"fake")).as_deref(),
            Ok("fake"),
            "round trip"
        );
    }

    #[test]
    fn always_sensitive_taints_and_a_public_read_does_not() {
        let deps = BTreeMap::from([
            (
                "PASS".to_string(),
                BTreeSet::from([ALWAYS_SENSITIVE.to_string()]),
            ),
            ("PLAIN".to_string(), BTreeSet::from(["OTHER".to_string()])),
        ]);
        let hot = tainted(&deps, |_| false);
        assert_eq!(hot, BTreeSet::from(["PASS".to_string()]));

        let policy = Policy::default();
        assert_eq!(
            functions::taint("url", Some("host"), Some("U"), "H", &policy),
            Taint::Public
        );
        assert_eq!(
            functions::taint("url", Some("password"), Some("U"), "P", &policy),
            Taint::Always
        );
        assert_eq!(
            functions::taint("url", Some("user"), Some("U"), "N", &policy),
            Taint::Inputs
        );
        let raw = BTreeMap::from([
            (
                "U".to_string(),
                Raw::literal("postgres://app:fakepass@db.test/x"),
            ),
            ("H".to_string(), Raw::computed("url($U, host)")),
            ("P".to_string(), Raw::computed("url($U, password)")),
            ("F".to_string(), Raw::computed("${U | url(host)}")),
        ]);
        let out = resolve_full(&raw, &Values::new(), "dev", &Values::new(), &policy);
        assert!(
            out.deps.get("H").is_none_or(BTreeSet::is_empty),
            "{:?}",
            out.deps
        );
        assert!(
            out.deps.get("F").is_none_or(BTreeSet::is_empty),
            "{:?}",
            out.deps
        );
        assert!(out.deps["P"].contains(ALWAYS_SENSITIVE));
        assert!(out.deps["P"].contains("U"));
    }
}
