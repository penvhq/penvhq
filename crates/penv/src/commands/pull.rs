use std::io::IsTerminal;
use std::path::Path;

use penv_agent::Policy;
use penv_cloud::api::Fetched;
use penv_dotenv::ensure_ignored;
use serde_json::json;

use crate::agent::detect_here;
use crate::commands::cloud::{Cloud, address, link, refuse};
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::{ENV_FILE, GITIGNORE_FILE, read_file, show, write_file, write_private_file};
use crate::output::{Output, Report};

/// Write a plain `.env` from the cloud. Only a person asks for this.
pub fn run(
    out: &Output,
    cwd: &Path,
    env_flag: Option<&str>,
    i_am_human: bool,
    env: &Env,
    agent_flag: bool,
) -> Result<Report, CliError> {
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let policy = Policy::for_(&detection, agent_flag);
    // The flag speaks for a person only where one is typing: an agent's pipes
    // are not a terminal, and --agent says outright that no person is there.
    let person = i_am_human
        && !agent_flag
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();
    if !policy.pull_allowed && !person {
        return Err(CliError::new(
            "agent_session",
            format!(
                "penv pull writes every value to disk, and this session is {}.",
                detection.name().unwrap_or("an agent")
            ),
            "Run it yourself in a terminal with --i-am-human, or let penv run inject the values instead.",
        )
        .with_exit(Exit::Auth));
    }

    let (schema_path, mut schema) = super::load_schema(cwd)?;
    let dir = schema_path.parent().unwrap_or(cwd).to_path_buf();

    // Signing in comes first: a folder with no header still has an account to look in.
    let cloud = Cloud::open(env, &detection)?;
    let bearer = cloud.bearer(env, schema.org.as_deref())?;
    if !schema.is_cloud() {
        let may_ask = !detection.is_agent() && !agent_flag && std::io::stdin().is_terminal();
        link(&cloud, &bearer, &schema_path, &mut schema, may_ask)?;
    }
    let wanted = super::cloud::pick_environment(
        &cloud,
        &bearer,
        &schema,
        &schema_path,
        env_flag,
        env,
        super::interactive(out, env, agent_flag),
    )?;
    crate::source::check_environment(&wanted)?;
    let at = address(&schema, &wanted)?;
    let spinner = crate::ui::spinner(&format!("Reading {at}"));
    let Fetched::Body { body, .. } = cloud
        .api
        .env_get(&bearer, &at, None, true)
        .map_err(|e| refuse(e, Some(&at)))?
    else {
        return Err(CliError::new(
            "unexpected_answer",
            "the server sent back no values.",
            "Run penv pull again.",
        ));
    };
    spinner.stop(&format!("Read {at}"));

    // One value the file cannot hold must not cost the rest of the pull.
    let mut stored: Vec<(String, String)> = Vec::new();
    let mut left_out: Vec<String> = Vec::new();
    // Present in the cloud and withheld from this identity: a marker, never a value.
    let mut redacted: Vec<&str> = Vec::new();
    let encrypt = crate::config::Config::load(&dir)
        .map(|c| c.encrypt())
        .unwrap_or(false);
    for key in &body.keys {
        if key.redacted {
            match penv_dotenv::write_redacted(&[], &[key.name.as_str()]) {
                Ok(_) if !redacted.contains(&key.name.as_str()) => redacted.push(&key.name),
                Ok(_) => {}
                Err(e) => left_out.push(e.to_string()),
            }
            continue;
        }
        let Some(value) = key.value.as_deref() else {
            continue;
        };
        match penv_dotenv::write(&[(key.name.as_str(), value)]) {
            Ok(_) => {
                let sensitive = schema.get(&key.name).map(|k| k.sensitive).unwrap_or(true);
                stored.push((
                    key.name.clone(),
                    crate::localcrypt::stored_as(encrypt, &key.name, value, sensitive)?,
                ));
            }
            Err(e) => left_out.push(e.to_string()),
        }
    }
    let pairs: Vec<(&str, &str)> = stored
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let contents = penv_dotenv::write_redacted(&pairs, &redacted)
        .map_err(|e| CliError::new("unwritable_value", e.to_string(), "Run penv pull again."))?;

    // Development is `.env`; every other environment is its own layer, which
    // run and push read back.
    let env_path = if wanted == crate::source::DEFAULT_ENVIRONMENT {
        dir.join(ENV_FILE)
    } else {
        dir.join(format!(".env.{wanted}"))
    };
    write_private_file(&env_path, &contents)?;

    let ignore_path = dir.join(GITIGNORE_FILE);
    let existing = if ignore_path.is_file() {
        read_file(&ignore_path)?
    } else {
        String::new()
    };
    let update = ensure_ignored(&existing);
    if update.changed() {
        write_file(&ignore_path, &update.content)?;
    }

    let style = out.style();
    let mut lines = vec![format!(
        "{} {} key(s) into {}",
        style.green("wrote"),
        pairs.len(),
        show(&env_path)
    )];
    for reason in &left_out {
        lines.push(format!("{} {reason}", style.yellow("left out")));
    }
    if !left_out.is_empty() {
        lines.push(style.dim(
            "Left-out keys stay in the cloud, and penv run still passes them to your command.",
        ));
    }
    if !redacted.is_empty() {
        lines.push(style.dim(&format!(
            "Write-only in penv-cloud, written as a redacted marker: {}",
            redacted.join(", ")
        )));
    }
    let skipped: Vec<&String> = body
        .skipped
        .iter()
        .filter(|name| !redacted.contains(&name.as_str()))
        .collect();
    if !skipped.is_empty() {
        lines.push(style.dim(&format!(
            "Not written, as the cloud generates each on demand or holds no value for it: {}. Give a missing one a value with penv set <KEY>.",
            skipped.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        )));
    }

    Ok(Report::new(
        json!({
            "address": at.to_string(),
            "env": show(&env_path),
            "keys": pairs.len(),
            "skipped": skipped,
            "redacted": redacted,
            "left_out": left_out,
            "gitignore": { "path": show(&ignore_path), "added": update.added },
        }),
        lines.join("\n"),
    ))
}
