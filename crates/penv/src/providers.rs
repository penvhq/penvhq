//! Which provider a command reads from, and at what address. `--provider` wins,
//! then the prefix of `@penv=` in the schema, then penv.cloud. The API root is
//! `PENV_URL` (penv.cloud only), then `[providers.<slug>] url` in
//! `.penv/config.toml`, then the provider's own default. Credentials from the
//! environment never go to a root that only `config.toml` names.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use penv_cloud::provider::{self, Provider};

use crate::env::Env;
use crate::error::CliError;

static FLAG: OnceLock<Option<String>> = OnceLock::new();
static CWD: OnceLock<PathBuf> = OnceLock::new();

/// Remember `--provider` and where the command runs, for the first cloud read.
pub fn remember(flag: Option<&str>, cwd: &Path) {
    let _ = FLAG.set(flag.map(str::to_string));
    let _ = CWD.set(cwd.to_path_buf());
}

pub struct Chosen {
    pub provider: &'static Provider,
    pub url: String,
    /// The root came from `.penv/config.toml`. A committed file can be changed
    /// in a pull request, so credentials read from the environment (a token,
    /// CI OIDC, an AWS proof) are not sent there; only a login this machine
    /// holds for that root is.
    pub from_config: bool,
}

pub fn chosen(env: &Env) -> Result<Chosen, CliError> {
    let flag = FLAG.get().cloned().flatten();
    let found = CWD
        .get()
        .and_then(|cwd| crate::source::load(cwd).ok())
        .map(|(path, schema)| (path.parent().map(Path::to_path_buf), schema));
    let named = found
        .as_ref()
        .and_then(|(_, schema)| schema.provider.clone());
    let slug = flag
        .or(named)
        .unwrap_or_else(|| provider::DEFAULT.to_string());
    let Some(provider) = provider::find(&slug) else {
        return Err(CliError::new(
            "unknown_provider",
            format!(
                "penv has no provider named {slug}. It has: {}.",
                provider::slugs().join(", ")
            ),
            "Name one of those in @penv= or --provider. docs/PROVIDERS.md says how a provider is added.",
        ));
    };
    let from_env = env
        .get(penv_cloud::api::URL_VAR)
        .filter(|v| !v.is_empty() && provider.slug == provider::DEFAULT)
        .map(str::to_string);
    let from_config = match found.as_ref().and_then(|(dir, _)| dir.clone()) {
        Some(dir) => crate::config::Config::load(&dir)?.provider_url(provider.slug),
        None => None,
    };
    let (url, from_config) = match (from_env, from_config) {
        (Some(url), _) => (url, false),
        (None, Some(url)) => (url, true),
        (None, None) => (provider.default_url.to_string(), false),
    };
    Ok(Chosen {
        provider,
        url,
        from_config,
    })
}
