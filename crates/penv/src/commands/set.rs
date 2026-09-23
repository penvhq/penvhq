use std::io::IsTerminal;
use std::path::Path;

use penv_cloud::api::CloudKey;
use penv_schema::{Key, is_public_prefixed, is_valid_key_name};
use serde_json::json;

use crate::agent::detect_here;
use crate::commands::cloud::{Cloud, address, environment, key_schema, refuse};
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::{read_file, show, write_file};
use crate::output::{Output, Report};
use crate::prompt;

/// Write one value. It is read from a pipe or typed with the echo off, and it
/// is never an argument: arguments land in shell history.
pub fn set(
    out: &Output,
    cwd: &Path,
    name: &str,
    env_flag: Option<&str>,
    value_flag: Option<&str>,
    env: &Env,
) -> Result<Report, CliError> {
    if value_flag.is_some() {
        return Err(CliError::new(
            "value_on_the_command_line",
            "penv set takes no --value, because the shell would keep it in its history.",
            format!("Pipe it in: printf %s \"$VALUE\" | penv set {name}."),
        ));
    }
    check_name(name)?;

    let (schema_path, schema) = super::load_schema(cwd)?;
    if !schema.is_cloud() {
        return set_local(out, &schema_path, &schema, name, env_flag, env);
    }
    let at = address(&schema, &environment(env_flag, env, &schema, &schema_path))?;
    let value = prompt::read_value(&format!("{name}: "))?;
    if value.is_empty() {
        return Err(CliError::new(
            "empty_value",
            format!("{name} was given no value."),
            format!("Pipe the value in, or run penv unset {name} to remove it."),
        )
        .with_exit(Exit::Validation));
    }

    let declared = schema.get(name).cloned();
    let key = declared.clone().unwrap_or_else(|| drafted(name, &value));
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let cloud = Cloud::open(env, &detection)?;
    let bearer = cloud.bearer(env, schema.org.as_deref())?;

    let mut sent = [CloudKey {
        name: name.to_string(),
        schema: Some(key_schema(&key)),
        value: Some(value),
        ..CloudKey::default()
    }];
    let result = super::cloud::with_hosts_fallback(&mut sent, |keys| {
        cloud.api.key_set(&bearer, &at, &keys[0])
    })
    .map_err(|e| refuse(e, Some(&at)))?;

    // A key the file never declared gets a block, with the type the value
    // implies and none of the value itself.
    let added = declared.is_none();
    if added {
        let source = read_file(&schema_path)?;
        write_file(&schema_path, &append(&source, &key))?;
    }

    let style = out.style();
    let mut lines = vec![format!(
        "{} {name} in {at}",
        style.green(&format!("set version {}", result.version))
    )];
    if added {
        lines.push(style.dim(&format!(
            "added a {} block for {name} to {}",
            key.ty,
            show(&schema_path)
        )));
    }

    Ok(Report::new(
        json!({
            "key": name,
            "address": at.to_string(),
            "version": result.version,
            "etag": result.etag,
            "schemaAdded": added,
        }),
        lines.join("\n"),
    ))
}

/// Remove one value. The key keeps its block: the schema says what may exist,
/// not what does.
pub fn unset(
    out: &Output,
    cwd: &Path,
    name: &str,
    env_flag: Option<&str>,
    env: &Env,
) -> Result<Report, CliError> {
    check_name(name)?;
    let (schema_path, schema) = super::load_schema(cwd)?;
    if !schema.is_cloud() {
        return unset_local(out, &schema_path, &schema, name, env_flag, env);
    }
    let at = address(&schema, &environment(env_flag, env, &schema, &schema_path))?;

    let detection = detect_here(env, std::io::stdout().is_terminal());
    let cloud = Cloud::open(env, &detection)?;
    let bearer = cloud.bearer(env, schema.org.as_deref())?;
    let result = cloud
        .api
        .key_unset(
            &bearer,
            &at,
            &CloudKey {
                name: name.to_string(),
                ..CloudKey::default()
            },
        )
        .map_err(|e| refuse(e, Some(&at)))?;

    Ok(Report::new(
        json!({ "key": name, "address": at.to_string(), "etag": result.etag }),
        format!("{} {name} from {at}", out.style().green("removed")),
    ))
}

/// The file local mode writes for an environment: `.env` for development,
/// `.env.<env>` for the rest.
fn local_file(
    schema_path: &Path,
    schema: &penv_schema::Schema,
    env_flag: Option<&str>,
    env: &Env,
) -> (std::path::PathBuf, String) {
    let dir = schema_path.parent().unwrap_or(Path::new("."));
    let name = crate::source::environment(env_flag, env, schema, dir);
    let file = if name == crate::source::DEFAULT_ENVIRONMENT {
        dir.join(crate::files::ENV_FILE)
    } else {
        dir.join(format!(".env.{name}"))
    };
    (file, name)
}

/// Local mode: the value goes into the environment's file, and a key with
/// `@rotate` has the moment recorded in `.penv/config.toml` for every clone.
fn set_local(
    out: &Output,
    schema_path: &Path,
    schema: &penv_schema::Schema,
    name: &str,
    env_flag: Option<&str>,
    env: &Env,
) -> Result<Report, CliError> {
    let (file, environment) = local_file(schema_path, schema, env_flag, env);
    crate::source::check_environment(&environment)?;
    let value = prompt::read_value(&format!("{name}: "))?;
    if value.is_empty() {
        return Err(CliError::new(
            "empty_value",
            format!("{name} was given no value."),
            format!("Pipe the value in, or run penv unset {name} to remove it."),
        )
        .with_exit(Exit::Validation));
    }
    let existing = if file.is_file() {
        read_file(&file)?
    } else {
        String::new()
    };
    let sensitive = schema.get(name).map(|k| k.sensitive).unwrap_or(true);
    let dir = schema_path.parent().unwrap_or(Path::new("."));
    let stored = crate::localcrypt::stored(dir, name, &value, sensitive)?;
    let written = penv_dotenv::upsert(&existing, name, &stored).map_err(|e| {
        CliError::new(
            "unwritable_value",
            e.to_string(),
            "Change the value and try again.",
        )
    })?;
    crate::files::write_private_file(&file, &written)?;
    // A secret just written to a file git would pick up: ignore the value files
    // now, before anyone runs git add. A file git already tracks is only named,
    // since ignoring it changes nothing.
    let exposure = crate::gitexposure::exposure(&file);
    let mut guarded = None;
    if exposure == Some(crate::gitexposure::Exposure::Unignored) {
        let root = file.parent().unwrap_or(Path::new("."));
        let ignore = root.join(crate::files::GITIGNORE_FILE);
        let existing = if ignore.is_file() {
            read_file(&ignore)?
        } else {
            String::new()
        };
        let update = penv_dotenv::ensure_ignored(&existing);
        if update.changed() {
            write_file(&ignore, &update.content)?;
            guarded = Some(ignore);
        }
    }

    let declared = schema.get(name).cloned();
    let key = declared.clone().unwrap_or_else(|| drafted(name, &value));
    let added = declared.is_none();
    if added {
        let source = read_file(schema_path)?;
        write_file(schema_path, &append(&source, &key))?;
    }

    let dir = schema_path.parent().unwrap_or(Path::new("."));
    let recorded = match key.rotate {
        Some(_) => {
            use penv_cloud::Clock as _;
            let now = penv_schema::rotate::instant(penv_cloud::SystemClock.now());
            let mut config = crate::config::Config::load(dir)?;
            config.set_rotated(name, &now);
            Some((config.save(dir)?, now))
        }
        None => None,
    };

    let style = out.style();
    let mut lines = vec![format!("{} {name} in {}", style.green("set"), show(&file))];
    if added {
        lines.push(style.dim(&format!(
            "added a {} block for {name} to {}",
            key.ty,
            show(schema_path)
        )));
    }
    if let Some(ignore) = &guarded {
        lines.push(style.dim(&format!(
            "added .env, .env.*, !.env.schema to {} so git leaves the value out",
            show(ignore)
        )));
    }
    if exposure == Some(crate::gitexposure::Exposure::Tracked) {
        crate::ui::warn(&source_message_tracked(&file, name));
    }
    if let Some((path, _)) = &recorded {
        lines.push(style.dim(&format!(
            "recorded the rotation in {}; commit it",
            show(path)
        )));
    }
    Ok(Report::new(
        json!({
            "key": name,
            "environment": environment,
            "file": show(&file),
            "schemaAdded": added,
            "rotatedAt": recorded.map(|(_, at)| at),
        }),
        lines.join("\n"),
    ))
}

fn unset_local(
    out: &Output,
    schema_path: &Path,
    schema: &penv_schema::Schema,
    name: &str,
    env_flag: Option<&str>,
    env: &Env,
) -> Result<Report, CliError> {
    let (file, environment) = local_file(schema_path, schema, env_flag, env);
    crate::source::check_environment(&environment)?;
    let existing = if file.is_file() {
        read_file(&file)?
    } else {
        String::new()
    };
    let (left, found) = penv_dotenv::remove(&existing, name);
    if found {
        crate::files::write_private_file(&file, &left)?;
    }
    let verb = if found { "removed" } else { "not set" };
    Ok(Report::new(
        json!({ "key": name, "environment": environment, "file": show(&file), "removed": found }),
        format!("{} {name} in {}", out.style().green(verb), show(&file)),
    ))
}

fn check_name(name: &str) -> Result<(), CliError> {
    if is_valid_key_name(name) && name == name.to_ascii_uppercase() {
        return Ok(());
    }
    Err(CliError::new(
        "invalid_key",
        format!("{name} is not a usable key name."),
        "Use upper snake case, such as DATABASE_URL.",
    )
    .with_exit(Exit::Validation))
}

/// The block a new key gets: `init`'s type inference, and never the value.
fn drafted(name: &str, value: &str) -> Key {
    Key {
        name: name.to_string(),
        ty: penv_dotenv::infer_type(name, value),
        required: true,
        sensitive: !is_public_prefixed(name),
        ..Key::default()
    }
}

/// Append one block, leaving every line already in the file alone.
fn append(source: &str, key: &Key) -> String {
    let mut out = source.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&penv_schema::render_key(key));
    out
}

fn source_message_tracked(file: &Path, name: &str) -> String {
    crate::source::exposure_message(
        file,
        crate::gitexposure::Exposure::Tracked,
        &[name.to_string()],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_block_takes_the_type_from_the_value_and_none_of_the_value() {
        let key = drafted("DATABASE_URL", "postgres://user:pw@host/db");
        assert_eq!(key.ty.to_string(), "url");
        assert_eq!(key.default, None, "the value is never copied");
        assert!(key.sensitive);

        let public = drafted("NEXT_PUBLIC_APP_URL", "http://localhost:3000");
        assert!(!public.sensitive, "a bundler prefix is public");
    }

    #[test]
    fn appending_a_block_leaves_the_file_above_it_untouched() {
        let source = "# @penv=acme/api @schema=1\n\n# @type=port\nPORT=3000\n";
        let out = append(source, &drafted("API_TOKEN", "sk_test_FAKE"));
        assert!(out.starts_with(source.trim_end()), "{out}");
        assert!(out.contains("API_TOKEN="), "{out}");
        assert!(
            !out.contains("sk_test_FAKE"),
            "the value never lands: {out}"
        );
    }

    #[test]
    fn a_key_name_the_writer_could_not_write_is_refused_early() {
        assert!(check_name("DATABASE_URL").is_ok());
        assert!(check_name("database_url").is_err());
        assert!(check_name("2FA").is_err());
    }
}
