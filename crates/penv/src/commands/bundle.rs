//! `penv bundle`: write one environment's values, encrypted, to
//! `.penv/<env>.bundle` for a deploy to read with `PENV_BUNDLE_KEY`.

use std::io::IsTerminal;
use std::path::Path;

use penv_agent::Policy;
use serde_json::json;

use crate::agent::detect_here;
use crate::commands::cloud::Fetcher;
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::{show, write_file_making_parents};
use crate::output::{Output, Report};
use crate::source;

pub fn run(
    out: &Output,
    cwd: &Path,
    env_flag: Option<&str>,
    env: &Env,
    agent_flag: bool,
) -> Result<Report, CliError> {
    let detection = detect_here(env, std::io::stdout().is_terminal());
    if !Policy::for_(&detection, agent_flag).pull_allowed {
        return Err(CliError::new(
            "agent_session",
            format!(
                "penv bundle prints the key that opens every value, and this session is {}.",
                detection.name().unwrap_or("an agent")
            ),
            "Run it yourself.",
        )
        .with_exit(Exit::Auth));
    }
    let (schema_path, schema) = super::load_schema(cwd)?;
    let dir = schema_path.parent().unwrap_or(cwd).to_path_buf();
    let environment = source::environment(env_flag, env, &schema, &dir);
    // The bundle is built from the files and the cloud, never from an older
    // bundle: PENV_BUNDLE_KEY is only the key to reuse here.
    let mut without_bundle = env.clone();
    let reused = without_bundle.remove(crate::bundle::KEY_VAR);
    let mut fetcher = Fetcher::new(&without_bundle, &detection);
    let resolved = source::values(&schema, &dir, &environment, &without_bundle, &mut fetcher)?;
    // A bundle without the withheld keys would deploy as if they had no value.
    if let Some(refused) = source::withheld(&resolved) {
        return Err(refused);
    }
    if !resolved.errors.is_empty() && source::failed(&resolved.errors) {
        return Err(source::unresolved(&resolved.errors));
    }
    let values: std::collections::BTreeMap<String, String> = resolved
        .values
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let (key, fresh) = match reused.as_deref().and_then(crate::bundle::parse_key) {
        Some(key) => (key, false),
        None => {
            let bytes = penv_cloud::random_bytes(32).map_err(|e| {
                CliError::new(
                    "random_unavailable",
                    format!("the system random number generator failed: {e}."),
                    "Try again.",
                )
            })?;
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            (key, true)
        }
    };
    let file = crate::bundle::path(&dir, &environment);
    write_file_making_parents(&file, &crate::bundle::seal(&key, &environment, &values)?)?;
    let shown = show(file.strip_prefix(&dir).unwrap_or(&file));
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();

    let style = out.style();
    let mut lines = vec![format!(
        "{} {shown}: {} value(s) for {environment}",
        style.green("wrote"),
        values.len()
    )];
    if fresh {
        lines.push(format!("{}={hex}", crate::bundle::KEY_VAR));
        lines.push(style.dim(&format!(
            "Shown once. Store it as a secret in the deploy platform; penv run there reads {shown} with it. To rebuild with the same key, run penv bundle with {} set.",
            crate::bundle::KEY_VAR
        )));
    } else {
        lines.push(style.dim(&format!(
            "encrypted with the {} already set",
            crate::bundle::KEY_VAR
        )));
    }
    if !resolved.pending.is_empty() {
        lines.push(style.dim(&format!(
            "left out, generated on first run where it is used: {}",
            resolved.pending.join(", ")
        )));
    }
    Ok(Report::new(
        json!({
            "file": shown,
            "environment": environment,
            "keys": values.len(),
            "key": if fresh { Some(hex) } else { None },
        }),
        lines.join("\n"),
    ))
}
