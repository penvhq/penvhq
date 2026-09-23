use std::collections::BTreeMap;

/// The process environment as a value, so policy is testable without a process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env(BTreeMap<String, String>);

impl Env {
    pub fn from_process() -> Env {
        Env(std::env::vars().collect())
    }

    pub fn from_pairs(pairs: &[(&str, &str)]) -> Env {
        Env(pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect())
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    /// Take one variable out, returning it.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.0.remove(key)
    }
}
