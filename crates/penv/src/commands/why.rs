//! `penv why KEY`: where a value comes from and what penv does with it, never
//! the value itself.

use std::io::IsTerminal;
use std::path::Path;

use serde_json::json;

use crate::agent::detect_here;
use crate::commands::cloud::Fetcher;
use crate::commands::load_schema;
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::show;
use crate::output::{Output, Report};
use crate::source;

pub fn run(
    out: &Output,
    cwd: &Path,
    name: &str,
    env_flag: Option<&str>,
    env: &Env,
) -> Result<Report, CliError> {
    let (_schema_path, schema) = load_schema(cwd)?;
    let dir = schema_path_dir(cwd, &_schema_path);
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let environment = source::environment(env_flag, env, &schema, &dir);
    let mut fetcher = Fetcher::new(env, &detection);
    let resolved = source::values(&schema, &dir, &environment, env, &mut fetcher)?;
    let key = schema.get(name);
    let raw = resolved.layers.raw.get(name);
    let redacted = resolved.redacted.iter().any(|r| r == name);
    if key.is_none() && raw.is_none() && !redacted {
        return Err(CliError::new(
            "unknown_key",
            format!("{name} is not in .env.schema and no value file or cloud environment sets it."),
            "Run penv ls to see the keys.",
        )
        .with_exit(Exit::Validation));
    }

    // Where the value came from.
    let relative = |p: &Path| show(p.strip_prefix(&dir).unwrap_or(p));
    let origin = resolved.layers.origin.get(name).map(|p| relative(p));
    let from = match (&origin, raw) {
        _ if redacted => match (&resolved.cloud, resolved.layers.redacted.get(name)) {
            (Some(c), _) => format!(
                "penv.cloud {c}, withheld: the environment is write-only and only workload identities read it"
            ),
            (None, Some(file)) => format!(
                "a # penv:redacted marker in {}: penv.cloud holds the value write-only",
                relative(file)
            ),
            (None, None) => "penv.cloud, withheld: the environment is write-only".to_string(),
        },
        (Some(o), Some(_)) if o == "the process environment" => {
            "the process environment".to_string()
        }
        (Some(o), Some(_)) => o.clone(),
        (None, Some(_)) => resolved
            .cloud
            .clone()
            .map(|c| format!("penv.cloud {c}"))
            .unwrap_or_else(|| "a value file".into()),
        (_, None) => match key.and_then(|k| k.default.as_ref()) {
            Some(_) => ".env.schema (the default)".to_string(),
            None => "nowhere: no file, cloud environment or default sets it".to_string(),
        },
    };
    let expression = raw
        .filter(|r| r.computed)
        .map(|r| r.text.clone())
        .or_else(|| {
            key.and_then(|k| k.default.clone())
                .filter(|_| raw.is_none())
        });
    let built_from: Vec<String> = expression
        .as_deref()
        .map(|text| {
            schema
                .keys
                .iter()
                .filter(|k| k.name != name && crate::usage::mentions(text, &k.name))
                .map(|k| k.name.clone())
                .collect()
        })
        .unwrap_or_default();

    // Files lower in the cascade that set it too, and lost.
    let shadowed: Vec<String> = resolved
        .layers
        .read
        .iter()
        .filter(|p| Some(relative(p)) != origin)
        .filter(|p| {
            std::fs::read_to_string(p)
                .map(|t| penv_dotenv::read(&t).raw().iter().any(|(k, _)| k == name))
                .unwrap_or(false)
        })
        .map(|p| relative(p))
        .collect();

    let value = resolved.values.get(name);
    let state = match value {
        _ if redacted => "redacted",
        Some(v) if !v.is_empty() => "set",
        Some(_) => "empty",
        None => "unset",
    };
    let sensitive = source::is_sensitive(&schema, &resolved.tainted, name);
    let tainted = resolved.tainted.contains(name) && key.is_some_and(|k| !k.sensitive);
    let public = schema.is_public(name);
    let hosts = key.map(|k| k.hosts.clone()).unwrap_or_default();

    let mut handling = Vec::new();
    if sensitive {
        handling.push("masked in penv run's output and, through the preload and the generated env file, in the app's logs and responses".to_string());
    }
    if tainted {
        handling.push(
            "sensitive because it is built from a sensitive key, though marked @sensitive=false"
                .into(),
        );
    }
    if public {
        handling.push("public: bundlers inline it into browser code".into());
    }
    if !hosts.is_empty() {
        handling.push(format!(
            "sealed for an agent or penv run --sealed: the command gets a placeholder; the value goes only to {}",
            hosts.join(", ")
        ));
    }
    if key.is_none() {
        handling.push(
            "not declared in .env.schema: penv run still passes it, hidden in the output".into(),
        );
    }

    let style = out.style();
    let mut lines = vec![format!("{} in {environment}: {state}", style.bold(name))];
    lines.push(format!("  from      {from}"));
    if !built_from.is_empty() {
        lines.push(format!("  built on  {}", built_from.join(", ")));
    }
    for s in &shadowed {
        lines.push(format!("  overrides {s}"));
    }
    if let Some(k) = key {
        lines.push(format!(
            "  type      {}{}",
            k.ty,
            if k.required { ", required" } else { "" }
        ));
    }
    for h in &handling {
        lines.push(format!("  {h}"));
    }
    Ok(Report::new(
        json!({
            "key": name,
            "env": environment,
            "state": state,
            "from": from,
            "computed": !built_from.is_empty(),
            "builtFrom": built_from,
            "overrides": shadowed,
            "declared": key.is_some(),
            "type": key.map(|k| k.ty.to_string()),
            "required": key.is_some_and(|k| k.required),
            "sensitive": sensitive,
            "tainted": tainted,
            "public": public,
            "hosts": hosts,
        }),
        lines.join("\n"),
    )
    .listing())
}

fn schema_path_dir(cwd: &Path, schema_path: &Path) -> std::path::PathBuf {
    schema_path.parent().unwrap_or(cwd).to_path_buf()
}
