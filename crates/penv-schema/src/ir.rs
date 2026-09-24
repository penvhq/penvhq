use std::fmt;

use serde_json::{Value, json};

/// Key prefixes a framework sends to the browser, so the value is public:
/// Next.js, Vite (and Remix, SolidStart, TanStack Start on it), SvelteKit and
/// Astro and Rsbuild, Expo, Nuxt's public runtime config, Create React App,
/// Gatsby, Vue CLI, Storybook. `.penv/config.toml` `[public] prefixes` adds more,
/// for a custom Vite `envPrefix`.
pub const PUBLIC_PREFIXES: [&str; 9] = [
    "NEXT_PUBLIC_",
    "VITE_",
    "PUBLIC_",
    "EXPO_PUBLIC_",
    "NUXT_PUBLIC_",
    "REACT_APP_",
    "GATSBY_",
    "VUE_APP_",
    "STORYBOOK_",
];

pub fn is_public_prefixed(name: &str) -> bool {
    PUBLIC_PREFIXES.iter().any(|p| name.starts_with(p))
}

pub fn is_valid_key_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The grammar version this build writes and understands.
pub const SCHEMA_VERSION: u32 = 1;

/// `@defaultRequired`: `infer` makes a key required only when the schema gives it a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RequiredDefault {
    #[default]
    Yes,
    No,
    Infer,
}

impl RequiredDefault {
    pub fn to_json(self) -> Value {
        match self {
            RequiredDefault::Yes => Value::Bool(true),
            RequiredDefault::No => Value::Bool(false),
            RequiredDefault::Infer => Value::String("infer".into()),
        }
    }
}

/// `@assert(expression, "message")`: a check across keys, written in any block.
/// The expression uses the value functions; the message is shown when it is false.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assert {
    pub expr: String,
    pub message: String,
    pub line: u32,
}

/// `@import(path, KEY, ...)`: another schema's keys, all of them when none are named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub path: String,
    pub keys: Vec<String>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    /// The provider `@penv=` names before a colon (`doppler:acme/api`); none
    /// means penv.cloud.
    pub provider: Option<String>,
    pub org: Option<String>,
    pub project: Option<String>,
    pub schema_version: u32,
    pub default_sensitive: bool,
    pub default_required: RequiredDefault,
    /// `@currentEnv=$KEY`: the key whose value names the environment.
    pub current_env: Option<String>,
    pub imports: Vec<Import>,
    pub asserts: Vec<Assert>,
    pub keys: Vec<Key>,
    /// Lines penv read past: varlock-only or unknown decorators, types and constraints.
    pub warnings: Vec<Diagnostic>,
    /// Public prefixes beyond [`PUBLIC_PREFIXES`], from `.penv/config.toml`.
    pub public_prefixes: Vec<String>,
}

impl Default for Schema {
    fn default() -> Self {
        Schema {
            provider: None,
            org: None,
            project: None,
            schema_version: SCHEMA_VERSION,
            default_sensitive: true,
            default_required: RequiredDefault::Yes,
            current_env: None,
            imports: Vec::new(),
            asserts: Vec::new(),
            public_prefixes: Vec::new(),
            keys: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

impl Schema {
    pub fn get(&self, name: &str) -> Option<&Key> {
        self.keys.iter().find(|k| k.name == name)
    }

    /// True when a framework sends this key to the browser.
    pub fn is_public(&self, name: &str) -> bool {
        is_public_prefixed(name)
            || self
                .public_prefixes
                .iter()
                .any(|p| name.starts_with(p.as_str()))
    }

    /// The prefix that makes this key public, for messages.
    pub fn public_prefix(&self, name: &str) -> Option<String> {
        PUBLIC_PREFIXES
            .iter()
            .map(|p| p.to_string())
            .chain(self.public_prefixes.iter().cloned())
            .filter(|p| name.starts_with(p.as_str()))
            .max_by_key(String::len)
    }

    /// The schema as one environment sees it: `forEnv` requirements settled.
    pub fn for_environment(&self, environment: &str) -> Schema {
        let mut out = self.clone();
        for key in &mut out.keys {
            if let Some((envs, required)) = &key.required_in {
                let listed = envs.iter().any(|e| e == environment);
                key.required = if listed { *required } else { !*required };
            }
        }
        out
    }

    /// True when the file names a cloud project.
    pub fn is_cloud(&self) -> bool {
        self.org.is_some() && self.project.is_some()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "schemaVersion": self.schema_version,
                        "provider": self.provider,
            "org": self.org,
            "project": self.project,
            "defaultSensitive": self.default_sensitive,
            "defaultRequired": self.default_required.to_json(),
            "currentEnv": self.current_env,
            "imports": self.imports.iter().map(|i| json!({ "path": i.path, "keys": i.keys })).collect::<Vec<_>>(),
                        "asserts": self.asserts.iter().map(|a| json!({ "expr": a.expr, "message": a.message })).collect::<Vec<_>>(),
            "keys": self.keys.iter().map(Key::to_json).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Key {
    pub name: String,
    pub description: Option<String>,
    pub ty: Type,
    pub required: bool,
    /// `@required` / `@optional` when written; `None` means required was inferred.
    pub required_decorator: Option<bool>,
    /// `@required=forEnv(a, b)` (true) or `@optional=forEnv(a, b)` (false): the
    /// environments the rule holds in. [`Schema::for_environment`] applies it.
    pub required_in: Option<(Vec<String>, bool)>,
    pub sensitive: bool,
    /// `@sensitive` / `@sensitive=false` when written.
    pub sensitive_decorator: Option<bool>,
    pub default: Option<String>,
    /// The default is a function call or holds `${KEY}`, resolved against the other values.
    pub default_expr: bool,
    pub example: Option<String>,
    pub docs: Option<String>,
    pub deprecated: Option<String>,
    /// `@rotate`: how long a value may live before `check` reminds you to rotate it.
    pub rotate: Option<String>,
    /// The spec's `@dynamic` / `@static` pair: preserved, never acted on.
    pub dynamic: Option<bool>,
    /// `@hosts`: the only hosts this value may be sent to. When set, an agent's
    /// process holds a placeholder and penv puts the value in on the way out.
    pub hosts: Vec<String>,
}

impl Key {
    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "name": self.name,
            "description": self.description,
            "type": self.ty.to_json(),
                        "required": self.required,
            "requiredIn": self.required_in.as_ref().map(|(envs, required)| json!({ "environments": envs, "required": required })),
            "sensitive": self.sensitive,
            "default": self.default,
            "defaultExpr": self.default_expr,
            "example": self.example,
            "docs": self.docs,
            "deprecated": self.deprecated,
            "rotate": self.rotate,
                                    "dynamic": self.dynamic,
        });
        // Written only when set: penv.cloud stores this object per key and
        // refuses fields it does not know.
        if !self.hosts.is_empty() {
            out["hosts"] = json!(self.hosts);
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BaseType {
    #[default]
    String,
    Number,
    Boolean,
    Url,
    Email,
    Port,
    Enum,
}

impl BaseType {
    pub fn as_str(&self) -> &'static str {
        match self {
            BaseType::String => "string",
            BaseType::Number => "number",
            BaseType::Boolean => "boolean",
            BaseType::Url => "url",
            BaseType::Email => "email",
            BaseType::Port => "port",
            BaseType::Enum => "enum",
        }
    }

    pub fn from_name(s: &str) -> Option<BaseType> {
        Some(match s {
            "string" => BaseType::String,
            "number" => BaseType::Number,
            "boolean" => BaseType::Boolean,
            "url" => BaseType::Url,
            "email" => BaseType::Email,
            "port" => BaseType::Port,
            "enum" => BaseType::Enum,
            _ => return None,
        })
    }

    /// Constraint names this type accepts inside its call parentheses.
    pub fn constraints(&self) -> &'static [&'static str] {
        match self {
            BaseType::String => &[
                "startsWith",
                "endsWith",
                "minLength",
                "maxLength",
                "matches",
            ],
            BaseType::Number => &["min", "max", "isInt", "precision"],
            BaseType::Port => &["min", "max"],
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Type {
    pub base: BaseType,
    /// Enum members, in the order written.
    pub members: Vec<String>,
    /// Named constraints, sorted by name so rendering is canonical.
    pub constraints: Vec<(String, String)>,
}

impl Type {
    pub fn new(base: BaseType) -> Type {
        Type {
            base,
            members: Vec::new(),
            constraints: Vec::new(),
        }
    }

    pub fn constraint(&self, name: &str) -> Option<&str> {
        self.constraints
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn to_json(&self) -> Value {
        let constraints: serde_json::Map<String, Value> = self
            .constraints
            .iter()
            .map(|(n, v)| (n.clone(), Value::String(v.clone())))
            .collect();
        json!({
            "name": self.base.as_str(),
            "raw": self.to_string(),
            "members": self.members,
            "constraints": constraints,
        })
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.base.as_str())?;
        if self.members.is_empty() && self.constraints.is_empty() {
            return Ok(());
        }
        // A bare member holding `=` would read back as a constraint.
        let mut args: Vec<String> = self
            .members
            .iter()
            .map(|m| if m.contains('=') { quoted(m) } else { quote(m) })
            .collect();
        args.extend(
            self.constraints
                .iter()
                .map(|(n, v)| format!("{n}={}", quote(v))),
        );
        write!(f, "({})", args.join(","))
    }
}

/// The one quoting rule: decorator values, type arguments and defaults all use it.
pub(crate) fn quote(v: &str) -> String {
    if v.is_empty() || v.contains([' ', '\t', ',', '(', ')', '#', '"', '\'', '`']) {
        quoted(v)
    } else {
        v.to_string()
    }
}

fn quoted(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A problem in the schema file itself, located for the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: u32,
    pub column: u32,
    pub code: String,
    pub message: String,
}

impl Diagnostic {
    pub fn new(line: u32, column: u32, code: &str, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            line,
            column,
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "line": self.line,
            "column": self.column,
            "code": self.code,
            "message": self.message,
        })
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{} {}", self.line, self.column, self.message)
    }
}
