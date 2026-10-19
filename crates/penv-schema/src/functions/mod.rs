//! The functions that take the argument form of docs/Functions.md: `url`, `json`,
//! `hash` and `random`. Pure: values in, a value or a failure out.

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
    match (name, address) {
        ("url", Some(part)) => {
            let always = vocabulary()
                .url_parts
                .iter()
                .any(|p| p.sensitive && p.names(part));
            if always {
                Taint::Always
            } else if subject_key.is_some() && policy.url_public.contains(part) {
                Taint::Public
            } else {
                Taint::Inputs
            }
        }
        ("json", Some(path)) => {
            let listed = subject_key
                .and_then(|k| policy.json_public.get(k))
                .is_some_and(|paths| paths.contains(path));
            if listed { Taint::Public } else { Taint::Inputs }
        }
        ("hash", _) if policy.hash_public.contains(key) => Taint::Public,
        _ => Taint::Inputs,
    }
}

/// Evaluate `url`, `json` or `hash`. `random` is never evaluated here.
pub fn eval(call: &Call) -> Result<String, String> {
    match call.name {
        "url" => url::eval(call),
        "json" => json::eval(call),
        "hash" => hash::eval(call),
        other => Err(format!("{other}() is not evaluated from a schema")),
    }
}
