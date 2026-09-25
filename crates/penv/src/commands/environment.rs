use std::io::IsTerminal;
use std::path::Path;

use penv_cloud::provider::Capability;

use penv_cloud::api::Bearer;
use serde_json::json;

use crate::agent::detect_here;
use crate::cli::EnvCommand;
use crate::commands::cloud::{Cloud, refuse};
use crate::commands::project::confirm_erase;
use crate::commands::push::pick_org;
use crate::env::Env;
use crate::error::CliError;
use crate::files::{SCHEMA_FILE, find_schema, read_file};
use crate::output::{Output, Report, table};
use crate::ui;

pub fn run(
    out: &Output,
    cwd: &Path,
    project_flag: Option<&str>,
    command: &EnvCommand,
    env: &Env,
    agent_flag: bool,
) -> Result<Report, CliError> {
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let cloud = Cloud::open(env, &detection)?;
    cloud.require(Capability::Manage)?;
    let bearer = cloud.bearer(env, None)?;
    let agent = detection.is_agent() || agent_flag;
    let (org, project) = which_project(cwd, project_flag, &cloud, &bearer)?;
    let home = format!("{org}/{project}");

    match command {
        EnvCommand::Ls => {
            let spinner = ui::spinner(&format!("Reading {home}"));
            let projects = cloud
                .api
                .projects(&bearer, &org)
                .map_err(|e| refuse(e, None))?;
            spinner.stop(&format!("Read {home}"));
            let found = projects
                .into_iter()
                .find(|p| p.slug.eq_ignore_ascii_case(&project))
                .ok_or_else(|| {
                    CliError::new(
                        "not_found",
                        format!("{home} does not exist on the server."),
                        "Run penv project ls to see your projects.",
                    )
                })?;
            let rows: Vec<Vec<String>> = found
                .environments
                .iter()
                .map(|name| vec![name.clone()])
                .collect();
            let text = match rows.is_empty() {
                true => format!("{home} has no environments. Create one with penv env new <name>."),
                false => table(&[&format!("ENVIRONMENTS IN {home}")], &rows, &out.style()),
            };
            Ok(Report::new(
                json!({ "project": home, "environments": found.environments }),
                text,
            )
            .listing())
        }
        EnvCommand::New { name } => create(&cloud, &bearer, &org, &project, name, None),
        EnvCommand::Copy { from, to } => create(&cloud, &bearer, &org, &project, to, Some(from)),
        EnvCommand::Rename { old, new } => {
            let spinner = ui::spinner(&format!("Renaming {home}/{old}"));
            cloud
                .api
                .rename_environment(&bearer, &org, &project, old, new)
                .map_err(|e| refuse(e, None))?;
            spinner.stop("Renamed");
            Ok(Report::new(
                json!({ "project": home, "renamed": new, "from": old }),
                format!(
                    "renamed {home}/{old} to {home}/{new}\nScripts and CI that pass --env {old} need --env {new} now."
                ),
            ))
        }
        EnvCommand::Rm { name } => {
            let replay = match project_flag {
                Some(flag) => format!("penv env rm {name} -p {flag}"),
                None => format!("penv env rm {name}"),
            };
            confirm_erase(&replay, name, agent)?;
            let spinner = ui::spinner(&format!("Deleting {home}/{name}"));
            let gone = cloud
                .api
                .delete_environment(&bearer, &org, &project, name)
                .map_err(|e| refuse(e, None))?;
            spinner.stop("Deleted");
            Ok(Report::new(
                json!({ "project": home, "deleted": name, "keys": gone.parameters }),
                format!(
                    "deleted {home}/{name}: {} key(s) are gone for good",
                    gone.parameters
                ),
            ))
        }
    }
}

fn create(
    cloud: &Cloud,
    bearer: &Bearer,
    org: &str,
    project: &str,
    name: &str,
    from: Option<&str>,
) -> Result<Report, CliError> {
    let spinner = ui::spinner(&format!("Creating {org}/{project}/{name}"));
    let made = cloud
        .api
        .create_environment(bearer, org, project, name, from)
        .map_err(|e| refuse(e, None))?;
    spinner.stop("Created");
    let text = match from {
        Some(from) => format!(
            "created {org}/{project}/{name} with {} key(s) from {from}\nValues are never copied. Fill them with penv set <KEY> --env {name}.",
            made.copied
        ),
        None => {
            format!("created {org}/{project}/{name}\nAdd values with penv set <KEY> --env {name}.")
        }
    };
    Ok(Report::new(
        json!({ "project": format!("{org}/{project}"), "created": made.name, "copied": made.copied, "from": from }),
        text,
    ))
}

/// `-p org/name`, or `-p name` in the one organisation this person has, or the
/// project this folder is linked to.
fn which_project(
    cwd: &Path,
    flag: Option<&str>,
    cloud: &Cloud,
    bearer: &Bearer,
) -> Result<(String, String), CliError> {
    if let Some(flag) = flag.filter(|v| !v.is_empty()) {
        return match flag.split_once('/') {
            Some((org, project)) => Ok((org.to_string(), project.to_string())),
            None => Ok((pick_org(cloud, bearer, None)?, flag.to_string())),
        };
    }
    let linked = find_schema(cwd)
        .and_then(|path| read_file(&path).ok())
        .and_then(|source| penv_schema::parse(&source).ok())
        .and_then(|schema| Some((schema.org?, schema.project?)));
    linked.ok_or_else(|| {
        CliError::new(
            "project_required",
            format!("this folder is not linked to a project: {SCHEMA_FILE} is missing or has no # @penv=org/project line."),
            "Add -p <project>, or run penv pull here to link the folder first.",
        )
    })
}
