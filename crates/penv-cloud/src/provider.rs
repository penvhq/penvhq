//! The providers penv can read an environment from. Each is compiled in and
//! declares what it supports; `docs/PROVIDERS.md` is the contract a new one
//! implements, and the conformance tests are what it passes. penv never loads a
//! provider at runtime, so a schema can never make penv run code.

use crate::api::DEFAULT_BASE_URL;

/// What a provider can do. penv refuses a command the chosen provider does not
/// declare, by name, instead of sending a request it cannot answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Read the keys and values of one environment. Every provider has it.
    Read,
    /// Write and delete one value.
    Write,
    /// List projects and environments, create and rename them.
    Manage,
    /// Show one value only after a person approves (`penv reveal` under an agent).
    Approve,
    /// Record who read what, and when a value was last written (`@rotate`).
    Audit,
}

impl Capability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::Read => "read",
            Capability::Write => "write",
            Capability::Manage => "manage",
            Capability::Approve => "approve",
            Capability::Audit => "audit",
        }
    }
}

#[derive(Debug)]
pub struct Provider {
    /// The word a schema names it by: `@penv=<slug>:org/project`.
    pub slug: &'static str,
    pub name: &'static str,
    /// The API root, overridable with `[providers.<slug>] url`.
    pub default_url: &'static str,
    pub capabilities: &'static [Capability],
}

impl Provider {
    pub fn can(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// The provider a schema with no prefix uses.
pub const DEFAULT: &str = "penv";

pub const PROVIDERS: &[Provider] = &[Provider {
    slug: "penv",
    name: "penv.cloud",
    default_url: DEFAULT_BASE_URL,
    capabilities: &[
        Capability::Read,
        Capability::Write,
        Capability::Manage,
        Capability::Approve,
        Capability::Audit,
    ],
}];

pub fn find(slug: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.slug == slug)
}

pub fn slugs() -> Vec<&'static str> {
    PROVIDERS.iter().map(|p| p.slug).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn penv_is_a_provider_that_can_do_everything_and_is_the_default() {
        let penv = find(DEFAULT).expect("penv.cloud is a provider");
        for capability in [
            Capability::Read,
            Capability::Write,
            Capability::Manage,
            Capability::Approve,
            Capability::Audit,
        ] {
            assert!(penv.can(capability), "{}", capability.as_str());
        }
        assert!(find("doppler").is_none());
        let mut seen = slugs();
        seen.dedup();
        assert_eq!(seen.len(), PROVIDERS.len(), "slugs are unique");
    }
}
