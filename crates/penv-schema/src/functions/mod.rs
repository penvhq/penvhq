//! The functions that take the argument form of docs/Functions.md, one file each,
//! listed in [`REGISTRY`]. Pure: values in, a value or a failure out.

pub mod args;
pub mod hash;
pub mod json;
pub mod random;
pub mod url;

use std::collections::{BTreeMap, BTreeSet};

pub use args::check_args;

use crate::editor::vocabulary;
use crate::resolve::Expr;

/// The `public` lists of `.penv/config.toml`. Each default is the safe side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub url_public: BTreeSet<String>,
    /// Subject key -> read paths.
    pub json_public: BTreeMap<String, BTreeSet<String>>,
    /// Keys whose `hash()` result may be shown.
    pub hash_public: BTreeSet<String>,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            url_public: vocabulary()
                .url_parts
                .iter()
                .filter(|p| p.public)
                .map(|p| p.name.clone())
                .collect(),
            json_public: BTreeMap::new(),
            hash_public: BTreeSet::new(),
        }
    }
}

/// One parsed argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    Subject(Expr),
    Address(String),
    Named(String, Expr),
}

/// A call after its arguments are evaluated. `raw` holds option names given as `raw()`,
/// `quoted` those whose value was a quoted literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call<'a> {
    pub name: &'a str,
    pub subject: Option<String>,
    pub address: Option<&'a str>,
    pub options: Vec<(&'a str, String)>,
    pub raw: BTreeSet<&'a str>,
    pub quoted: BTreeSet<&'a str>,
}

/// How a result's taint is decided, before evaluating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taint {
    /// Sensitive when any input is.
    Inputs,
    /// Declassified by a `public` list: the inputs only decide the result.
    Public,
    /// Sensitive whatever the inputs are.
    Always,
}

pub type Check = fn(&[Arg]) -> Result<(), String>;
pub type Lookup = fn(&Call) -> Result<Option<String>, String>;

/// One argument-form function. Adding one is a file beside this one, its entry in
/// [`REGISTRY`] and its word in `vocabulary.json`.
pub struct Function {
    pub name: &'static str,
    pub eval: fn(&Call) -> Result<String, String>,
    /// The result's taint from the address, the subject's stored key, the key being
    /// computed and the policy.
    pub taint: fn(Option<&str>, Option<&str>, &str, &Policy) -> Taint,
    /// A parse-time check past the vocabulary's shape checks.
    pub check: Option<Check>,
    /// With an address it reads a part of its subject, so a chain of reads keeps the
    /// subject's stored key.
    pub reads: bool,
    /// Its `public` list names the stored key at the end of a chain of reads, not
    /// only a direct subject.
    pub through_reads: bool,
    /// Option values may be `raw()` and keep their quotes.
    pub embeds: bool,
    /// The lookup a `@default` fallback runs instead: `None` when nothing is there.
    pub lookup: Option<Lookup>,
    /// Soft warnings about an evaluated subject.
    pub warnings: Option<fn(&str) -> Vec<String>>,
}

pub const REGISTRY: &[Function] = &[
    Function {
        name: "url",
        eval: url::eval,
        taint: url::taint,
        check: Some(url::check_password),
        reads: true,
        through_reads: true,
        embeds: false,
        lookup: None,
        warnings: None,
    },
    Function {
        name: "json",
        eval: json::eval,
        taint: json::taint,
        check: None,
        reads: true,
        through_reads: false,
        embeds: true,
        lookup: Some(json::lookup),
        warnings: Some(json::warnings),
    },
    Function {
        name: "hash",
        eval: hash::eval,
        taint: hash::taint,
        check: None,
        reads: false,
        through_reads: false,
        embeds: false,
        lookup: None,
        warnings: None,
    },
    Function {
        name: "random",
        eval: random::eval,
        taint: |_, _, _, _| Taint::Inputs,
        check: Some(random::check),
        reads: false,
        through_reads: false,
        embeds: false,
        lookup: None,
        warnings: None,
    },
];

/// The registered function called `name`.
pub fn find(name: &str) -> Option<&'static Function> {
    REGISTRY.iter().find(|f| f.name == name)
}

/// `key` is the key being computed. `subject_key` is the stored `$KEY` the read
/// chain ends at, `None` when an expression built the subject: only a stored value
/// is declassified, never one the expression could have put a secret in.
pub fn taint(
    name: &str,
    address: Option<&str>,
    subject_key: Option<&str>,
    key: &str,
    policy: &Policy,
) -> Taint {
    find(name).map_or(Taint::Inputs, |f| {
        (f.taint)(address, subject_key, key, policy)
    })
}

pub fn eval(call: &Call) -> Result<String, String> {
    match find(call.name) {
        Some(f) => (f.eval)(call),
        None => Err(format!("{}() is not evaluated from a schema", call.name)),
    }
}
