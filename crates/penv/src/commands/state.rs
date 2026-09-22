use std::io::IsTerminal;
use std::path::Path;

use penv_agent::{Detection, Policy};
use serde_json::{Value, json};

use crate::agent::detect_here;
use crate::commands::cloud::{self, Cloud};
use crate::env::Env;
use crate::error::CliError;
use crate::files::{ENV_FILE, SCHEMA_FILE, find_schema, read_file, show, value_files};
use crate::output::{Output, Report};

/// Where values come from (local files or the cloud), and the one command that follows.
pub fn run(out: &Output, cwd: &Path, env: &Env) -> Result<Report, CliError> {
    let schema_path = find_schema(cwd);
    // Values sit beside the schema; with no schema yet, beside the caller.
    let dir = schema_path
        .as_deref()
        .and_then(Path::parent)
        .unwrap_or(cwd)
        .to_path_buf();
    let found = value_files(&dir);
    let listed = found.iter().map(|p| show(p)).collect::<Vec<_>>().join(", ");
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let policy = Policy::for_(&detection, false);

    let schema = match &schema_path {
        Some(path) => penv_schema::parse(&read_file(path)?).ok(),
        None => None,
    };
    let cloud = schema.as_ref().filter(|s| s.is_cloud()).map(|s| {
        (
            s.org.clone().unwrap_or_default(),
            s.project.clone().unwrap_or_default(),
        )
    });

    let environment = schema
        .as_ref()
        .map(|s| crate::source::environment(None, env, s, &dir))
        .unwrap_or_else(|| crate::source::DEFAULT_ENVIRONMENT.to_string());

    // Only the files this environment's cascade reads, in the order it reads them.
    let layered: Vec<String> = penv_dotenv::cascade(&environment)
        .iter()
        .map(|name| dir.join(name))
        .filter(|path| path.is_file())
        .map(|path| show(&path))
        .collect();

    let (location, next, note, credential, cache_age) = match (&schema_path, &cloud) {
        (None, _) if !found.is_empty() => (
            "local",
            "penv init",
            format!("There are values here ({listed}) and no schema for them yet."),
            None,
            None,
        ),
        (None, _) => (
            "local",
            "penv init",
            format!("Write a {ENV_FILE} first; penv init reads it and every .env.* beside it."),
            None,
            None,
        ),
        (Some(_), None) => (
            "local",
            "penv check",
            if layered.is_empty() {
                format!(
                    "No value files for {environment} next to the schema yet; write {ENV_FILE} or run penv set <KEY>."
                )
            } else {
                format!(
                    "Values for {environment} come from {}, later files winning.",
                    layered.join(", ")
                )
            },
            None,
            None,
        ),
        (Some(_), Some((org, project))) => {
            let (held, age) = cloud_state(env, &detection, org, project);
            let (next, note) = if held {
                (
                    "penv run",
                    format!("{SCHEMA_FILE} names {org}/{project}, and this host has a credential."),
                )
            } else {
                (
                    "penv login",
                    format!(
                        "{SCHEMA_FILE} names {org}/{project}, and this host has no credential."
                    ),
                )
            };
            ("cloud", next, note, Some(held), age)
        }
    };

    let style = out.style();
    // One label column, wide enough for the longest label, padded before styling.
    let row =
        |label: &str, value: String| format!("{} {value}", style.dim(&format!("{label:<10}")));
    let mut rows = vec![
        row("version", env!("CARGO_PKG_VERSION").to_string()),
        row("location", style.bold(location)),
        row("env", environment.clone()),
        row("next", next.to_string()),
    ];
    if let Some((org, project)) = &cloud {
        rows.push(row("project", format!("{org}/{project}")));
        rows.push(row(
            "credential",
            if credential == Some(true) {
                "present"
            } else {
                "none"
            }
            .to_string(),
        ));
        rows.push(row(
            "cache",
            match cache_age {
                Some(seconds) => format!("{seconds}s old"),
                None => "empty".to_string(),
            },
        ));
    }
    if let Some(name) = detection.name() {
        rows.push(row(
            "agent",
            format!("{} ({})", style.bold(name), detection.confidence.as_str()),
        ));
        rows.push(row(
            "masking",
            if policy.mask { "on" } else { "off" }.to_string(),
        ));
    }
    let mut text = rows.join("\n");
    text.push('\n');
    text.push_str(&style.dim(&note));

    Ok(Report::new(
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "location": location,
            "schema": schema_path.as_deref().map(show),
            "project": cloud.as_ref().map(|(org, project)| format!("{org}/{project}")),
            "org": cloud.as_ref().map(|(org, _)| org.clone()),
            "credential": credential,
            "cacheAge": cache_age,
            "environment": environment,
            "valueFiles": found.iter().map(|p| show(p)).collect::<Vec<_>>(),
            "layered": layered,
            "next": next,
            "note": note,
            "agent": agent_json(&detection),
            "masking": policy.mask,
        }),
        text,
    ))
}

/// Whether this host can prove itself, and how old its cached development
/// values are. Which credential it holds is never said.
fn cloud_state(env: &Env, detection: &Detection, org: &str, project: &str) -> (bool, Option<u64>) {
    let Ok(opened) = Cloud::open(env, detection) else {
        return (false, None);
    };
    let held = penv_cloud::credential::present(env.as_map(), opened.keychain.as_ref());
    let at = penv_cloud::Address::new(org, project, cloud::DEFAULT_ENVIRONMENT);
    // The cache opens for the credential that filled it, so only a person's own
    // login can say how old it is.
    let age = opened
        .user()
        .ok()
        .flatten()
        .and_then(|bearer| opened.cache(&at, &bearer))
        .and_then(|cache| cache.age(opened.now));
    (held, age)
}

fn agent_json(detection: &Detection) -> Value {
    match &detection.agent {
        None => Value::Null,
        Some(agent) => json!({
            "name": agent.name,
            "version": agent.version,
            "mode": agent.mode,
            "confidence": detection.confidence.as_str(),
            "session": detection.session_id,
            "markers": detection.markers,
        }),
    }
}
