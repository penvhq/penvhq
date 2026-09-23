use crate::ir::{
    Assert, BaseType, Diagnostic, Import, Key, RequiredDefault, SCHEMA_VERSION, Schema, Type,
    is_public_prefixed, is_valid_key_name,
};
use crate::resolve::is_expression;
use crate::rotate::Span;

/// Header decorators varlock reads and penv does not act on. They are named so a
/// header block is still recognised as one, and each is reported once.
const VARLOCK_HEADER: [&str; 7] = [
    "generateTypes",
    "plugin",
    "redactLogs",
    "preventLeaks",
    "envFlag",
    "setValuesBulk",
    "disable",
];

/// Parse a `.env.schema`. Every problem in the file is reported at once.
pub fn parse(input: &str) -> Result<Schema, Vec<Diagnostic>> {
    let mut p = Parser {
        schema: Schema::default(),
        diags: Vec::new(),
    };
    p.run(input);
    if p.diags.is_empty() {
        Ok(p.schema)
    } else {
        Err(p.diags)
    }
}

struct Parser {
    schema: Schema,
    diags: Vec<Diagnostic>,
}

#[derive(Debug)]
struct Decorator {
    name: String,
    value: Option<String>,
    /// Written as `@name(args)` rather than `@name=value`.
    call: bool,
    line: u32,
    column: u32,
}

#[derive(Debug, Default)]
struct Block {
    description: Vec<String>,
    decorators: Vec<Decorator>,
}

impl Block {
    fn is_empty(&self) -> bool {
        self.description.is_empty() && self.decorators.is_empty()
    }

    fn has_header_decorator(&self) -> bool {
        self.decorators.iter().any(|d| {
            matches!(
                d.name.as_str(),
                "penv"
                    | "schema"
                    | "defaultSensitive"
                    | "defaultRequired"
                    | "currentEnv"
                    | "import"
                    | "assert"
            ) || VARLOCK_HEADER.contains(&d.name.as_str())
        })
    }
}

impl Parser {
    fn error(&mut self, line: u32, column: u32, code: &str, message: impl Into<String>) {
        self.diags
            .push(Diagnostic::new(line, column, code, message));
    }

    /// `@assert(expression, "message")`, from a header or a key block alike. The
    /// message is the last argument; the expression may hold commas of its own.
    fn read_assert(&mut self, d: &Decorator) {
        let text = d.value.as_deref().filter(|_| d.call).unwrap_or_default();
        let parsed = last_top_level_comma(text)
            .map(|at| (text[..at].trim(), unquote(text[at + 1..].trim())));
        match parsed {
            Some((expr, message)) if !expr.is_empty() && !message.is_empty() => {
                self.schema.asserts.push(Assert {
                    expr: expr.to_string(),
                    message,
                    line: d.line,
                });
            }
            _ => self.error(
                d.line,
                d.column,
                "invalid_decorator",
                format!(
                    "line {}: @assert takes an expression and a message, such as @assert(not(eq($PORT, $ADMIN_PORT)), \"PORT and ADMIN_PORT collide\")",
                    d.line
                ),
            ),
        }
    }

    /// Read past, reported, never fatal: a varlock schema must parse here.
    fn warn(&mut self, d: &Decorator, code: &str, message: impl Into<String>) {
        self.schema
            .warnings
            .push(Diagnostic::new(d.line, d.column, code, message));
    }

    fn run(&mut self, input: &str) {
        let body = input.strip_prefix('\u{feff}').unwrap_or(input);
        let mut block = Block::default();
        let mut first_block_done = false;

        for (idx, raw) in body.split('\n').enumerate() {
            let line_no = idx as u32 + 1;
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            let trimmed = line.trim();

            if trimmed.is_empty() {
                if !block.is_empty() {
                    if !first_block_done {
                        self.take_header(&block);
                    }
                    first_block_done = true;
                }
                block = Block::default();
                continue;
            }

            if is_divider(trimmed) {
                if !block.is_empty() {
                    if !first_block_done {
                        self.take_header(&block);
                    }
                    first_block_done = true;
                }
                block = Block::default();
                continue;
            }

            if let Some(rest) = trimmed.strip_prefix('#') {
                let hash_col = (line.len() - line.trim_start().len()) as u32 + 1;
                self.read_comment(rest, line_no, hash_col + 1, &mut block);
                continue;
            }

            if !first_block_done && block.has_header_decorator() {
                self.take_header(&block);
                block = Block::default();
            }
            self.read_key(line, line_no, &block);
            block = Block::default();
            first_block_done = true;
        }

        if !block.is_empty() && !first_block_done {
            self.take_header(&block);
        }
    }

    fn read_comment(&mut self, rest: &str, line_no: u32, base_col: u32, block: &mut Block) {
        let lead = rest.len() - rest.trim_start().len();
        let body = rest.trim();
        if !body.starts_with('@') {
            if !body.is_empty() {
                block.description.push(body.to_string());
            }
            return;
        }
        self.read_decorators(body, line_no, base_col + lead as u32, block);
    }

    fn read_decorators(&mut self, body: &str, line_no: u32, base_col: u32, block: &mut Block) {
        let chars: Vec<char> = body.chars().collect();
        let mut i = 0usize;
        while i < chars.len() {
            if chars[i].is_whitespace() {
                i += 1;
                continue;
            }
            let column = base_col + i as u32;
            if chars[i] != '@' {
                self.error(
                    line_no,
                    column,
                    "invalid_decorator",
                    format!("line {line_no}: expected a decorator starting with @"),
                );
                return;
            }
            i += 1;
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            if name.is_empty() {
                self.error(
                    line_no,
                    column,
                    "invalid_decorator",
                    format!("line {line_no}: @ with no decorator name"),
                );
                return;
            }
            let mut value = None;
            let mut call = false;
            if i < chars.len() && chars[i] == '(' {
                let open = i;
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
                if i >= chars.len() {
                    self.error(
                        line_no,
                        column,
                        "invalid_decorator",
                        format!("line {line_no}: @{name}( is never closed"),
                    );
                    return;
                }
                value = Some(chars[open + 1..i].iter().collect());
                call = true;
                i += 1;
            } else if i < chars.len() && chars[i] == '=' {
                i += 1;
                match self.read_value(&chars, &mut i, line_no, base_col) {
                    Some(v) => value = Some(v),
                    None => return,
                }
            }
            block.decorators.push(Decorator {
                name,
                value,
                call,
                line: line_no,
                column,
            });
        }
    }

    fn read_value(
        &mut self,
        chars: &[char],
        i: &mut usize,
        line_no: u32,
        base_col: u32,
    ) -> Option<String> {
        if *i >= chars.len() {
            return Some(String::new());
        }
        let quote = chars[*i];
        if quote == '"' || quote == '\'' {
            let open = *i;
            *i += 1;
            let mut out = String::new();
            while *i < chars.len() {
                let c = chars[*i];
                if c == '\\' && quote == '"' && *i + 1 < chars.len() {
                    out.push(chars[*i + 1]);
                    *i += 2;
                    continue;
                }
                if c == quote {
                    *i += 1;
                    return Some(out);
                }
                out.push(c);
                *i += 1;
            }
            self.error(
                line_no,
                base_col + open as u32,
                "unterminated_quote",
                format!("line {line_no}: a quoted decorator value is never closed"),
            );
            return None;
        }
        let start = *i;
        let mut depth = 0usize;
        let mut quoted: Option<char> = None;
        while *i < chars.len() {
            let c = chars[*i];
            match quoted {
                Some(q) if c == q => quoted = None,
                Some(_) => {}
                None if c == '"' || c == '\'' => quoted = Some(c),
                None if c == '(' => depth += 1,
                None if c == ')' => depth = depth.saturating_sub(1),
                None if depth == 0 && c.is_whitespace() => break,
                None => {}
            }
            *i += 1;
        }
        Some(chars[start..*i].iter().collect())
    }

    fn take_header(&mut self, block: &Block) {
        for d in &block.decorators {
            match d.name.as_str() {
                "penv" => match d.value.as_deref().and_then(split_address) {
                    Some((provider, org, project)) => {
                        self.schema.provider = provider;
                        self.schema.org = Some(org);
                        self.schema.project = Some(project);
                    }
                    None => self.error(
                        d.line,
                        d.column,
                        "invalid_header",
                        format!(
                            "line {}: @penv takes org/project, or provider:org/project",
                            d.line
                        ),
                    ),
                },
                "schema" => match d.value.as_deref().and_then(|v| v.parse::<u32>().ok()) {
                    Some(n) if n > 0 => self.schema.schema_version = n,
                    _ => self.error(
                        d.line,
                        d.column,
                        "invalid_header",
                        format!("line {}: @schema takes a version number, such as 1", d.line),
                    ),
                },
                "defaultSensitive" => {
                    if let Some(v) = self.flag(d) {
                        self.schema.default_sensitive = v;
                    }
                }
                "defaultRequired" => {
                    if d.value.as_deref() == Some("infer") {
                        self.schema.default_required = RequiredDefault::Infer;
                    } else if let Some(v) = self.flag(d) {
                        self.schema.default_required = if v {
                            RequiredDefault::Yes
                        } else {
                            RequiredDefault::No
                        };
                    }
                }
                "currentEnv" => match d.value.as_deref().map(|v| v.trim_start_matches('$')) {
                    Some(key) if is_valid_key_name(key) => {
                        self.schema.current_env = Some(key.to_string());
                    }
                    _ => self.error(
                        d.line,
                        d.column,
                        "invalid_header",
                        format!("line {}: @currentEnv takes a key, such as $APP_ENV", d.line),
                    ),
                },
                "import" => {
                    // varlock's current form is `pick=[A, B]`; the older
                    // positional keys still read.
                    let text = d.value.clone().unwrap_or_default();
                    let (text, picked) = take_pick(&text);
                    let mut args: Vec<String> = split_args(&text)
                        .iter()
                        .map(|a| unquote(a.trim()))
                        .filter(|a| !a.is_empty())
                        .collect();
                    args.extend(picked);
                    match args.split_first() {
                        Some((path, keys)) if d.call => self.schema.imports.push(Import {
                            path: path.clone(),
                            keys: keys.to_vec(),
                            line: d.line,
                        }),
                        _ => self.error(
                            d.line,
                            d.column,
                            "invalid_header",
                            format!(
                                "line {}: @import takes a path and optional keys, such as @import(../.env.schema, API_KEY)",
                                d.line
                            ),
                        ),
                    }
                }
                "assert" => self.read_assert(d),
                "plugin" => self.warn(
                    d,
                    "varlock_only",
                    format!(
                        "line {}: @plugin is varlock's; penv loads no plugins and ignored it",
                        d.line
                    ),
                ),
                other => self.warn(
                    d,
                    "unknown_decorator",
                    format!(
                        "line {}: penv ignored @{other}, which it does not act on",
                        d.line
                    ),
                ),
            }
        }
        if self.schema.schema_version > SCHEMA_VERSION {
            let d = block
                .decorators
                .iter()
                .find(|d| d.name == "schema")
                .map(|d| (d.line, d.column))
                .unwrap_or((1, 1));
            self.error(
                d.0,
                d.1,
                "schema_too_new",
                format!(
                    "line {}: this schema is version {} and this penv reads up to {SCHEMA_VERSION}; run penv upgrade",
                    d.0, self.schema.schema_version
                ),
            );
        }
    }

    /// A boolean decorator: bare means true, otherwise `true` or `false`.
    fn flag(&mut self, d: &Decorator) -> Option<bool> {
        match d.value.as_deref() {
            None | Some("true") => Some(true),
            Some("false") => Some(false),
            Some(_) => {
                self.error(
                    d.line,
                    d.column,
                    "invalid_decorator_value",
                    format!("line {}: @{} takes true or false", d.line, d.name),
                );
                None
            }
        }
    }

    fn require_value(&mut self, d: &Decorator) -> Option<String> {
        match d.value.as_deref() {
            Some(v) if !v.is_empty() => Some(v.to_string()),
            _ => {
                self.error(
                    d.line,
                    d.column,
                    "missing_decorator_value",
                    format!("line {}: @{} needs a value", d.line, d.name),
                );
                None
            }
        }
    }

    fn read_key(&mut self, line: &str, line_no: u32, block: &Block) {
        let Some(eq) = line.find('=') else {
            self.error(
                line_no,
                1,
                "invalid_line",
                format!("line {line_no}: expected KEY=value or a # comment"),
            );
            return;
        };
        let name = line[..eq].trim().to_string();
        if !is_valid_key_name(&name) {
            self.error(
                line_no,
                1,
                "invalid_key_name",
                format!("line {line_no}: {name:?} is not a usable key name"),
            );
            return;
        }
        if self.schema.get(&name).is_some() {
            self.error(
                line_no,
                1,
                "duplicate_key",
                format!("line {line_no}: {name} is declared twice; keep one block"),
            );
            return;
        }

        let written = line[eq + 1..].trim();
        let single = written.len() >= 2 && written.starts_with('\'') && written.ends_with('\'');
        let raw_default = unquote(written);
        let default_expr = !single && is_expression(&raw_default);
        let default = if raw_default.is_empty() {
            None
        } else {
            Some(raw_default)
        };

        let mut key = Key {
            name,
            default,
            default_expr,
            ..Key::default()
        };
        let mut type_seen = false;

        for d in &block.decorators {
            match d.name.as_str() {
                "type" => {
                    if let Some(v) = self.require_value(d) {
                        type_seen = true;
                        key.ty = self.read_type(&v, d);
                    }
                }
                "required" | "optional" => {
                    let written = match d.value.as_deref() {
                        None | Some("true") => Some(true),
                        Some("false") => Some(false),
                        Some(_) => None,
                    };
                    let per_env = d
                        .value
                        .as_deref()
                        .and_then(|v| v.strip_prefix("forEnv("))
                        .and_then(|v| v.strip_suffix(')'))
                        .map(|inner| {
                            split_args(inner)
                                .iter()
                                .map(|a| unquote(a.trim()))
                                .filter(|a| !a.is_empty())
                                .collect::<Vec<_>>()
                        });
                    match (written, per_env) {
                        (Some(v), _) => key.required_decorator = Some(v == (d.name == "required")),
                        (None, Some(envs)) if !envs.is_empty() => {
                            key.required_in = Some((envs, d.name == "required"));
                        }
                        (None, _) => self.warn(
                            d,
                            "varlock_only",
                            format!(
                                "line {}: penv ignored @{}={}; it takes true, false or forEnv(...)",
                                d.line,
                                d.name,
                                d.value.as_deref().unwrap_or_default()
                            ),
                        ),
                    }
                }
                "sensitive" => match d.value.as_deref() {
                    None | Some("true") | Some("false") => key.sensitive_decorator = self.flag(d),
                    Some(other) => self.warn(
                        d,
                        "varlock_only",
                        format!(
                            "line {}: penv ignored @sensitive={other}; it takes true or false",
                            d.line
                        ),
                    ),
                },
                "example" => key.example = self.require_value(d),
                "docs" => key.docs = self.require_value(d),
                "deprecated" => key.deprecated = Some(d.value.clone().unwrap_or_default()),
                "rotate" => {
                    if let Some(v) = self.require_value(d) {
                        if Span::parse(&v).is_some() {
                            key.rotate = Some(v);
                        } else {
                            self.error(
                                d.line,
                                d.column,
                                "invalid_decorator_value",
                                format!(
                                    "line {}: @rotate takes y, m (months), w, d, h, min and s, largest first, such as 1y6m, 90d, 12h or 30min",
                                    d.line
                                ),
                            );
                        }
                    }
                }
                "dynamic" | "static" => {
                    if d.value.is_some() {
                        self.error(
                            d.line,
                            d.column,
                            "invalid_decorator_value",
                            format!(
                                "line {}: @{} takes no value; write @dynamic or @static",
                                d.line, d.name
                            ),
                        );
                    } else {
                        key.dynamic = Some(d.name == "dynamic");
                    }
                }
                "assert" => self.read_assert(d),
                other => self.warn(
                    d,
                    "unknown_decorator",
                    format!(
                        "line {}: penv ignored @{other}, which it does not act on",
                        d.line
                    ),
                ),
            }
        }

        if !type_seen {
            key.ty = Type::new(BaseType::String);
        }
        if !block.description.is_empty() {
            key.description = Some(block.description.join(" "));
        }

        let prefixed = is_public_prefixed(&key.name);
        key.sensitive = match key.sensitive_decorator {
            Some(true) => {
                if prefixed {
                    self.error(
                        line_no,
                        1,
                        "sensitive_public_key",
                        format!(
                            "line {line_no}: {} carries a bundler prefix, so its value ships to the browser. Drop @sensitive or rename the key.",
                            key.name
                        ),
                    );
                }
                true
            }
            Some(false) => false,
            None => !prefixed && self.schema.default_sensitive,
        };
        key.required = match (key.required_decorator, self.schema.default_required) {
            (Some(v), _) => v,
            (None, RequiredDefault::Yes) => key.default.is_none(),
            (None, RequiredDefault::No) => false,
            (None, RequiredDefault::Infer) => key.default.is_some(),
        };

        self.schema.keys.push(key);
    }

    fn read_type(&mut self, raw: &str, d: &Decorator) -> Type {
        let (name, args) = match raw.find('(') {
            Some(open) => {
                if !raw.ends_with(')') {
                    self.error(
                        d.line,
                        d.column,
                        "invalid_type",
                        format!("line {}: @type is missing its closing bracket", d.line),
                    );
                    return Type::new(BaseType::String);
                }
                (&raw[..open], Some(&raw[open + 1..raw.len() - 1]))
            }
            None => (raw, None),
        };

        let Some(base) = BaseType::from_name(name) else {
            self.warn(
                d,
                "unknown_type",
                format!(
                    "line {}: penv checks {name} as a string; its types are string, number, boolean, url, email, port and enum, and a whole number is number(isInt=true)",
                    d.line
                ),
            );
            return Type::new(BaseType::String);
        };

        let mut ty = Type::new(base);
        let Some(args) = args else { return ty };
        for arg in split_args(args) {
            let arg = arg.trim();
            if arg.is_empty() {
                continue;
            }
            match arg.split_once('=') {
                Some((k, v)) => {
                    let k = k.trim();
                    let v = unquote(v.trim());
                    if !base.constraints().contains(&k) {
                        self.warn(
                            d,
                            "unknown_constraint",
                            format!(
                                "line {}: penv ignored the {k} constraint, which {} does not take",
                                d.line,
                                base.as_str()
                            ),
                        );
                        continue;
                    }
                    if ty.constraint(k).is_some() {
                        self.error(
                            d.line,
                            d.column,
                            "duplicate_constraint",
                            format!("line {}: {k} is given twice", d.line),
                        );
                        continue;
                    }
                    ty.constraints.push((k.to_string(), v));
                }
                None => {
                    if base != BaseType::Enum {
                        self.error(
                            d.line,
                            d.column,
                            "invalid_type",
                            format!(
                                "line {}: {} takes name=value constraints, not bare arguments",
                                d.line,
                                base.as_str()
                            ),
                        );
                        continue;
                    }
                    ty.members.push(unquote(arg));
                }
            }
        }
        if base == BaseType::Enum && ty.members.is_empty() {
            self.error(
                d.line,
                d.column,
                "invalid_type",
                format!("line {}: enum needs at least one member", d.line),
            );
        }
        ty.constraints.sort_by(|a, b| a.0.cmp(&b.0));
        ty
    }
}

/// `org/project` or `provider:org/project`. A provider is a lowercase word.
fn split_address(v: &str) -> Option<(Option<String>, String, String)> {
    let (provider, rest) = match v.split_once(':') {
        Some((provider, rest)) if is_provider(provider) => (Some(provider.to_string()), rest),
        Some(_) => return None,
        None => (None, v),
    };
    if rest.contains(':') {
        return None;
    }
    let (org, project) = split_project(rest)?;
    Some((provider, org, project))
}

pub(crate) fn is_provider(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn split_project(v: &str) -> Option<(String, String)> {
    let (org, project) = v.split_once('/')?;
    if org.is_empty() || project.is_empty() || project.contains('/') {
        return None;
    }
    Some((org.to_string(), project.to_string()))
}

/// Split on commas that are not inside quotes.
fn split_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in args.chars() {
        match quote {
            Some(q) => {
                current.push(c);
                if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                current.push(c);
            }
            None if c == ',' => out.push(std::mem::take(&mut current)),
            None => current.push(c),
        }
    }
    out.push(current);
    out
}

fn unquote(v: &str) -> String {
    let bytes = v.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            let inner = &v[1..v.len() - 1];
            return if first == b'"' {
                inner.replace("\\\"", "\"").replace("\\\\", "\\")
            } else {
                inner.to_string()
            };
        }
    }
    v.to_string()
}

/// `# ---`, or a labelled `# --- api ---`: a block boundary, the way varlock ends its header.
fn is_divider(trimmed: &str) -> bool {
    trimmed
        .strip_prefix('#')
        .is_some_and(|rest| rest.trim_start().starts_with("---"))
}

/// The last `,` outside quotes and brackets.
fn last_top_level_comma(text: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut last = None;
    for (i, c) in text.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => last = Some(i),
                _ => {}
            },
        }
    }
    last
}

/// Pull `pick=[A, B]` out of `@import(...)` arguments, returning the rest and the keys.
fn take_pick(text: &str) -> (String, Vec<String>) {
    let Some(start) = text.find("pick=[") else {
        return (text.to_string(), Vec::new());
    };
    let Some(len) = text[start..].find(']') else {
        return (text.to_string(), Vec::new());
    };
    let inner = &text[start + "pick=[".len()..start + len];
    let keys = inner
        .split(',')
        .map(|k| unquote(k.trim()))
        .filter(|k| !k.is_empty())
        .collect();
    let rest = format!("{}{}", &text[..start], &text[start + len + 1..]);
    (rest, keys)
}
