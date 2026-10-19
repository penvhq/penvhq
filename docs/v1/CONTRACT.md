# v1 contract (types and signatures only; T1 implements, later tasks consume)

```rust
// crates/penv-schema/src/functions/mod.rs
pub mod args; pub mod url; pub mod json; pub mod hash; pub mod random;

/// The `public` lists of .penv/config.toml (docs/Functions.md:113-131). Default = the doc's defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub url_public: BTreeSet<String>,                    // default: scheme host port origin path
    pub json_public: BTreeMap<String, BTreeSet<String>>, // subject key -> read paths
    pub hash_public: BTreeSet<String>,                   // result keys
}
impl Default for Policy { /* ... */ }

/// One parsed argument (docs/Functions.md:7-19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg { Subject(Expr), Address(String), Named(String, Expr) }

/// A call after its arguments are evaluated. `raw` holds option names given as raw(),
/// `quoted` those whose value was a quoted literal (json() writes them as strings).
pub struct Call<'a> {
    pub name: &'a str,
    pub subject: Option<String>,
    pub address: Option<&'a str>,
    pub options: Vec<(&'a str, String)>,
    pub raw: BTreeSet<&'a str>,
    pub quoted: BTreeSet<&'a str>,
}

/// Shape checks before evaluation, read from vocabulary.json: unknown, repeated or missing
/// option, address where none is allowed, positional where a name is needed.
pub fn check_args(name: &str, args: &[Arg]) -> Result<(), String>;

/// How the result's taint is decided, before evaluating. `key` is the key being computed,
/// `subject_key` the `$KEY` the subject names, if any.
pub enum Taint { Inputs, Public, Always }
pub fn taint(name: &str, address: Option<&str>, subject_key: Option<&str>, key: &str, policy: &Policy) -> Taint;

/// Evaluate url/json/hash. random is never evaluated here (resolve.rs keeps its error).
pub fn eval(call: &Call) -> Result<String, String>;
```

```rust
// crates/penv-schema/src/resolve.rs
pub const ALWAYS_SENSITIVE: &str = "\0sensitive";   // a dep tainted() always treats as sensitive
pub const FUNCTIONS: [&str; 19];                       // + url, json, hash, raw
pub const FILTERS: [&str; 6];                          // + b64d
pub fn resolve_full(raw, outside, environment, fetched, policy: &Policy) -> Resolution;
pub fn resolve(raw, outside, environment, fetched, policy: &Policy) -> (Values, Vec<ResolveError>);
// Expr::Call(String, Vec<Arg>); Expr::Filter { inner, filters: Vec<(String, Vec<Arg>)> }
```

```rust
// crates/penv-schema/src/functions/random.rs (T5 fills)
pub enum RandomKind { Alnum, Hex, Base64Url, Uuid }
pub struct RandomSpec { pub kind: RandomKind, pub length: Option<usize> }
pub fn spec(text: &str) -> Result<Option<RandomSpec>, String>; // None: not a random() value
```

```rust
// crates/penv-schema/src/ir.rs (T7 fills)
pub struct Key { /* existing */ pub hosts_expr: Option<String> } // computed @hosts
```

Vocabulary (vocabulary.json, Functions.md:133-149): function entries gain `args` and `address`;
new top-level sections `algorithms`, `encodings`, `url-parts`, `random-kinds`, each entry with
`name`, `doc`, and where relevant `needs`, `public`. `editor::Vocabulary` gains matching fields.
