//! `penv encrypt` and `penv decrypt`: convert the value files beside
//! .env.schema, sensitive keys only, keeping every other line as it is.

use std::io::IsTerminal;
use std::path::Path;

use penv_agent::Policy;
use serde_json::json;

use crate::agent::detect_here;
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::{read_file, show, write_private_file};
use crate::output::{Output, Report};

pub fn run(
    out: &Output,
    cwd: &Path,
    encrypt: bool,
    env: &Env,
    agent_flag: bool,
) -> Result<Report, CliError> {
    if !encrypt {
        let detection = detect_here(env, std::io::stdout().is_terminal());
        if !Policy::for_(&detection, agent_flag).pull_allowed {
            return Err(CliError::new(
                "agent_session",
                format!(
                    "penv decrypt writes values to disk in plain text, and this session is {}.",
                    detection.name().unwrap_or("an agent")
                ),
                "Run it yourself.",
            )
            .with_exit(Exit::Auth));
        }
    }
    let (schema_path, schema) = super::load_schema(cwd)?;
    let dir = schema_path.parent().unwrap_or(cwd).to_path_buf();
    let key = crate::localcrypt::key(encrypt)?;
    let Some(key) = key else {
        return Err(CliError::new(
            "no_local_key",
            "this machine holds no penv key, so nothing here was encrypted by it.",
            format!(
                "Supply the key the files were written with in {}.",
                crate::localcrypt::KEY_VAR
            ),
        )
        .with_exit(Exit::Validation));
    };
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && penv_dotenv::is_value_file(&p.file_name().unwrap_or_default().to_string_lossy())
        })
        .collect();
    files.sort();
    let mut changed: Vec<(String, usize)> = Vec::new();
    for file in files {
        let text = read_file(&file)?;
        let shown = show(file.strip_prefix(&dir).unwrap_or(&file));
        let mut updated = text.clone();
        let mut count = 0;
        for (name, raw) in penv_dotenv::read(&text).raw() {
            let sensitive = schema.get(&name).map(|k| k.sensitive).unwrap_or(true);
            let is_enc = crate::localcrypt::is_encrypted(&raw.text);
            let next = if encrypt && sensitive && !is_enc && !raw.text.is_empty() {
                // A value that computes from others stays readable: it holds
                // references, and the keys it names are encrypted themselves.
                if raw.computed && raw.text.contains('$') {
                    continue;
                }
                crate::localcrypt::encrypt(&key, &name, &raw.text)?
            } else if !encrypt && is_enc {
                crate::localcrypt::decrypt(&key, &name, &raw.text, &shown)?
            } else {
                continue;
            };
            updated = penv_dotenv::upsert(&updated, &name, &next).map_err(|e| {
                CliError::new("unwritable_value", e.to_string(), "Edit that line by hand.")
            })?;
            count += 1;
        }
        if count > 0 {
            write_private_file(&file, &updated)?;
            changed.push((shown, count));
        }
    }
    let verb = if encrypt { "encrypted" } else { "decrypted" };
    let style = out.style();
    let text = if changed.is_empty() {
        format!("nothing to {}", if encrypt { "encrypt" } else { "decrypt" })
    } else {
        changed
            .iter()
            .map(|(f, n)| format!("{} {n} value(s) in {f}", style.green(verb)))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(Report::new(
        json!({ "action": verb, "files": changed.iter().map(|(f, n)| json!({ "file": f, "values": n })).collect::<Vec<_>>() }),
        text,
    ))
}
