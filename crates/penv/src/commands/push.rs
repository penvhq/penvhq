use std::io::IsTerminal;
use std::path::Path;

use penv_cloud::provider::Capability;

use penv_cloud::api::{Address, Bearer, CloudKey};
use penv_schema::Schema;
use serde_json::json;

use crate::agent::detect_here;
use crate::commands::cloud::{
    Cloud, address, key_schema, note, pick_environment, project_name, refuse,
};
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::{SCHEMA_FILE, read_file, show, write_file};
use crate::output::{Output, Report};
use crate::source;

/// Move the local values to the cloud and take the file away. A schema with no
/// header gets a project first.
pub fn run(
    out: &Output,
    cwd: &Path,
    env_flag: Option<&str>,
    org_flag: Option<&str>,
    prune: bool,
    env: &Env,
) -> Result<Report, CliError> {
    let (schema_path, mut schema) = super::load_schema(cwd)?;
    let dir = schema_path.parent().unwrap_or(cwd).to_path_buf();
    let detection = detect_here(env, std::io::stdout().is_terminal());
    if prune && detection.is_agent() {
        return Err(CliError::new(
            "agent_session",
            format!(
                "--prune deletes cloud keys, and this session is {}.",
                detection.name().unwrap_or("an agent")
            ),
            "Run penv push --prune yourself, or push without it.",
        )
        .with_exit(Exit::Auth));
    }
    if let (true, Some(asked)) = (schema.is_cloud(), org_flag.filter(|v| !v.is_empty())) {
        let named = schema.org.as_deref().unwrap_or_default();
        if !named.eq_ignore_ascii_case(asked) {
            return Err(CliError::new(
                "org_mismatch",
                format!("the @penv header names {named}, not {asked}."),
                "Drop --org, or change the @penv header to move this project.",
            )
            .with_exit(Exit::Validation));
        }
    }

    // Nothing named and a person watching: they pick from the project's list,
    // which takes the cloud. Otherwise the files are read before the server
    // hears anything.
    let named = source::named_environment(env_flag, env, &schema, &dir);
    let asks = named.is_none() && schema.is_cloud() && super::interactive(out, env, false);
    let mut opened = None;
    let wanted = if asks {
        let cloud = Cloud::open(env, &detection)?;
        cloud.require(Capability::Write)?;
        let bearer = cloud.bearer(env, schema.org.as_deref())?;
        let wanted = pick_environment(&cloud, &bearer, &schema, &schema_path, env_flag, env, true)?;
        opened = Some((cloud, bearer));
        wanted
    } else {
        named.unwrap_or_else(|| source::DEFAULT_ENVIRONMENT.to_string())
    };

    // The shared layers are what gets pushed; `*.local` files stay on this machine.
    // They are read before the server hears anything: an empty push must not create a project.
    source::check_environment(&wanted)?;
    let layers = source::layers(&dir, &penv_dotenv::shared(&wanted))?;
    // A default is the schema's, not a value to store: only what a file wrote goes up.
    let pushed: Vec<String> = layers
        .raw
        .iter()
        // `random()` is this machine's own value: it is generated here, never sent.
        .filter(|(_, raw)| !raw.text.is_empty() && !raw.text.trim_start().starts_with("random("))
        .map(|(name, _)| name.clone())
        .collect();
    let read = layers.read.clone();
    let (mut values, mut errors) = source::finish(&schema, layers.raw, env, &wanted);
    // Only what is pushed has to compute; a default or a random() stays behind.
    errors.retain(|e| pushed.contains(&e.key));
    source::warn_unset(&errors);
    if source::failed(&errors) {
        return Err(source::unresolved(&errors));
    }
    values.retain(|name, _| pushed.contains(name));
    if !values.iter().any(|(_, value)| !value.is_empty()) {
        let files = penv_dotenv::shared(&wanted).join(" or ");
        return Err(CliError::new(
            "nothing_to_push",
            match read.is_empty() {
                false => format!(
                    "{} hold(s) no values.",
                    read.iter().map(|p| show(p)).collect::<Vec<_>>().join(", ")
                ),
                true => format!("there is no {files} for {wanted}, so there is nothing to push."),
            },
            format!("Write the values to {files} first, or run penv set <KEY> to push one key."),
        )
        .with_exit(Exit::Validation));
    }

    let (cloud, bearer) = match opened {
        Some(pair) => pair,
        None => {
            let cloud = Cloud::open(env, &detection)?;
            cloud.require(Capability::Write)?;
            let bearer = cloud.bearer(env, schema.org.as_deref())?;
            (cloud, bearer)
        }
    };

    let created = if schema.is_cloud() {
        ensure_environment(&cloud, &bearer, &address(&schema, &wanted)?)?;
        None
    } else {
        let org = pick_org(&cloud, &bearer, org_flag)?;
        let name = project_name(&dir);
        // A new project gets the environment this push is for, not only development.
        let mut environments = vec!["development".to_string()];
        if wanted != "development" {
            environments.push(wanted.clone());
        }
        note(&format!(
            "{SCHEMA_FILE} names no project, so penv will create {org}/{name} with environment {} and write the header.",
            environments.join(", ")
        ));
        // The header carries the slug the server derived, never the directory name.
        let project = cloud
            .api
            .create_project(&bearer, &org, &name, &environments)
            .map_err(|e| refuse(e, None))?;
        schema.org = Some(org.clone());
        schema.project = Some(project.slug.clone());
        let source = read_file(&schema_path)?;
        write_file(
            &schema_path,
            &penv_schema::set_header(&source, &org, &project.slug),
        )?;
        Some(format!("{org}/{}", project.slug))
    };

    let at = address(&schema, &wanted)?;

    let mut keys = payload(&schema, &values);
    let written = keys.iter().filter(|k| k.value.is_some()).count();
    let spinner = crate::ui::spinner(&format!("Sending {} key(s) to {at}", keys.len()));
    let result = super::cloud::with_hosts_fallback(&mut keys, |keys| {
        cloud.api.env_put(&bearer, &at, keys, prune)
    })
    .map_err(|e| refuse(e, Some(&at)))?;
    spinner.stop(&format!("Sent to {at}"));

    let removed: Vec<String> = read
        .iter()
        .filter(|path| std::fs::remove_file(path).is_ok())
        .map(|path| show(path))
        .collect();

    let style = out.style();
    let mut lines = Vec::new();
    if let Some(project) = &created {
        lines.push(style.dim(&format!("created {project}")));
    }
    lines.push(format!(
        "{} {} key(s) to {at}",
        style.green("pushed"),
        result.written
    ));
    if result.pruned > 0 {
        lines.push(style.dim(&format!("pruned {}", result.pruned)));
    }
    if !removed.is_empty() {
        lines.push(style.dim(&format!("removed {}", removed.join(", "))));
    }
    lines.push(style.dim(&format!("etag {}", result.etag)));

    Ok(Report::new(
        json!({
            "address": at.to_string(),
            "created": created,
            "sent": keys.len(),
            "withValues": written,
            "written": result.written,
            "unchanged": result.unchanged,
            "pruned": result.pruned,
            "etag": result.etag,
            "removed": removed,
        }),
        lines.join("\n"),
    ))
}

/// The envs route writes into an environment and never creates one, so a name
/// the project does not have is refused here with the names it does have.
fn ensure_environment(cloud: &Cloud, bearer: &Bearer, at: &Address) -> Result<(), CliError> {
    let projects = cloud
        .api
        .projects(bearer, &at.org)
        .map_err(|e| refuse(e, Some(at)))?;
    // A project the listing lacks is left to the PUT's own 404.
    let Some(project) = projects
        .iter()
        .find(|p| p.slug.eq_ignore_ascii_case(&at.project))
    else {
        return Ok(());
    };
    if project.environments.is_empty()
        || project
            .environments
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&at.environment))
    {
        return Ok(());
    }
    Err(CliError::new(
        "no_such_environment",
        format!(
            "{}/{} has no environment called {}.",
            at.org, at.project, at.environment
        ),
        format!(
            "Create it in the console, or push to one of: {}.",
            project.environments.join(", ")
        ),
    )
    .with_exit(Exit::Validation))
}

/// `--org`, else the one org this person has, as the listing spells it. Several
/// without a flag is a refusal that lists them.
pub(crate) fn pick_org(
    cloud: &Cloud,
    bearer: &Bearer,
    flag: Option<&str>,
) -> Result<String, CliError> {
    let orgs = cloud.api.orgs(bearer).map_err(|e| refuse(e, None))?;
    if let Some(asked) = flag.filter(|v| !v.is_empty()) {
        return orgs
            .iter()
            .find(|org| {
                org.slug.eq_ignore_ascii_case(asked) || org.name.eq_ignore_ascii_case(asked)
            })
            .map(|org| org.slug.clone())
            .ok_or_else(|| {
                CliError::new(
                    "no_such_org",
                    format!("this account is in no organisation called {asked}."),
                    match orgs.is_empty() {
                        true => "Create one in the console, then run this again.".to_string(),
                        false => format!(
                            "Pass one of: {}.",
                            orgs.iter()
                                .map(|o| o.slug.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    },
                )
            });
    }
    match orgs.len() {
        1 => Ok(orgs[0].slug.clone()),
        0 => Err(CliError::new(
            "no_org",
            "this account is in no organisation yet.",
            "Create one in the console, then run this again.",
        )),
        _ => Err(CliError::new(
            "org_required",
            format!(
                "this account is in {} organisations: {}.",
                orgs.len(),
                orgs.iter()
                    .map(|o| o.slug.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "Add --org <slug> to say which one you mean.",
        )),
    }
}

/// Every schema key with the value the file holds, then every value the schema
/// never declared. A key with no value updates the schema only.
fn payload(schema: &Schema, values: &penv_schema::Values) -> Vec<CloudKey> {
    let mut keys: Vec<CloudKey> = schema
        .keys
        .iter()
        .map(|key| CloudKey {
            name: key.name.clone(),
            schema: Some(key_schema(key)),
            value: values.get(&key.name).filter(|v| !v.is_empty()).cloned(),
            ..CloudKey::default()
        })
        .collect();
    keys.extend(
        values
            .iter()
            .filter(|(name, value)| schema.get(name).is_none() && !value.is_empty())
            .map(|(name, value)| CloudKey {
                name: name.clone(),
                value: Some(value.clone()),
                ..CloudKey::default()
            }),
    );
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use penv_schema::{BaseType, Key, Type};

    fn schema() -> Schema {
        Schema {
            keys: vec![
                Key {
                    name: "STRIPE_SECRET_KEY".into(),
                    ty: Type::new(BaseType::String),
                    required: true,
                    sensitive: true,
                    ..Key::default()
                },
                Key {
                    name: "PORT".into(),
                    ty: Type::new(BaseType::Port),
                    default: Some("3000".into()),
                    ..Key::default()
                },
            ],
            ..Schema::default()
        }
    }

    #[test]
    fn a_key_with_no_value_still_carries_its_schema() {
        let values: penv_schema::Values = [("STRIPE_SECRET_KEY".to_string(), String::new())]
            .into_iter()
            .collect();
        let keys = payload(&schema(), &values);
        assert_eq!(keys.len(), 2);
        assert!(keys[0].value.is_none(), "an empty value is not a value");
        assert!(keys[0].schema.is_some());
    }

    #[test]
    fn a_value_the_schema_never_declared_still_moves() {
        let values: penv_schema::Values = [
            ("STRIPE_SECRET_KEY".to_string(), "sk_test_FAKE".to_string()),
            ("LEGACY_API_KEY".to_string(), "left_over_FAKE".to_string()),
        ]
        .into_iter()
        .collect();
        let keys = payload(&schema(), &values);
        let extra = keys
            .iter()
            .find(|k| k.name == "LEGACY_API_KEY")
            .expect("drift");
        assert_eq!(extra.value.as_deref(), Some("left_over_FAKE"));
        assert!(extra.schema.is_none(), "penv invents no schema for drift");
    }
}
