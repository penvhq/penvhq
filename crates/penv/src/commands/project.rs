use std::io::IsTerminal;
use std::path::Path;

use penv_cloud::api::Bearer;
use serde_json::json;

use crate::agent::detect_here;
use crate::cli::ProjectCommand;
use crate::commands::cloud::{Cloud, cancelled, note, refuse};
use crate::commands::push::pick_org;
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::{SCHEMA_FILE, find_schema, read_file, write_file};
use crate::output::{Output, Report, table};
use crate::{prompt, ui};

pub fn run(
    out: &Output,
    cwd: &Path,
    command: &ProjectCommand,
    env: &Env,
    agent_flag: bool,
) -> Result<Report, CliError> {
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let cloud = Cloud::open(env, &detection)?;
    let bearer = cloud.bearer(env, None)?;
    let agent = detection.is_agent() || agent_flag;

    match command {
        ProjectCommand::Ls { org } => list(out, &cloud, &bearer, org.as_deref()),
        ProjectCommand::New { name, org } => {
            let org = pick_org(&cloud, &bearer, org.as_deref())?;
            let spinner = ui::spinner(&format!("Creating {org}/{name}"));
            let project = cloud
                .api
                .create_project(&bearer, &org, name, &["development".to_string()])
                .map_err(|e| refuse(e, None))?;
            spinner.stop("Created");
            Ok(Report::new(
                json!({ "created": format!("{org}/{}", project.slug), "environments": project.environments }),
                format!(
                    "created {org}/{} with environment development\nLink a folder to it: run penv pull there.",
                    project.slug
                ),
            ))
        }
        ProjectCommand::Rename { old, new, org } => {
            let org = pick_org(&cloud, &bearer, org.as_deref())?;
            let spinner = ui::spinner(&format!("Renaming {org}/{old}"));
            let project = cloud
                .api
                .rename_project(&bearer, &org, old, new)
                .map_err(|e| refuse(e, None))?;
            spinner.stop("Renamed");
            let relinked = relink(cwd, &org, old, &project.slug)?;
            let mut text = format!("renamed {org}/{old} to {org}/{}", project.slug);
            if relinked {
                text.push_str(&format!("\nupdated the @penv line in {SCHEMA_FILE}"));
            } else {
                text.push_str(&format!(
                    "\nOther folders linked to it need this first line in {SCHEMA_FILE}: # @penv={org}/{}",
                    project.slug
                ));
            }
            Ok(Report::new(
                json!({ "renamed": format!("{org}/{}", project.slug), "from": old, "relinked": relinked }),
                text,
            ))
        }
        ProjectCommand::Rm { name, org } => {
            let org = pick_org(&cloud, &bearer, org.as_deref())?;
            confirm_erase(&format!("penv project rm {name}"), name, agent)?;
            let spinner = ui::spinner(&format!("Deleting {org}/{name}"));
            let gone = cloud
                .api
                .delete_project(&bearer, &org, name)
                .map_err(|e| refuse(e, None))?;
            spinner.stop("Deleted");
            Ok(Report::new(
                json!({ "deleted": format!("{org}/{name}"), "environments": gone.environments, "keys": gone.parameters }),
                format!(
                    "deleted {org}/{name}: {} environment(s) and {} key(s) are gone for good",
                    gone.environments, gone.parameters
                ),
            ))
        }
    }
}

fn list(
    out: &Output,
    cloud: &Cloud,
    bearer: &Bearer,
    org: Option<&str>,
) -> Result<Report, CliError> {
    let spinner = ui::spinner("Looking for your projects");
    let orgs: Vec<String> = match org {
        Some(_) => vec![pick_org(cloud, bearer, org)?],
        None => cloud
            .api
            .orgs(bearer)
            .map_err(|e| refuse(e, None))?
            .into_iter()
            .map(|o| o.slug)
            .collect(),
    };
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut listed = Vec::new();
    for org in &orgs {
        for project in cloud
            .api
            .projects(bearer, org)
            .map_err(|e| refuse(e, None))?
        {
            rows.push(vec![
                format!("{org}/{}", project.slug),
                project.environments.join(", "),
            ]);
            listed.push(json!({ "org": org, "project": project.slug, "environments": project.environments }));
        }
    }
    spinner.stop(&format!("Found {} project(s)", rows.len()));

    let text = match rows.is_empty() {
        true => "You have no projects yet. Create one with penv project new <name>.".to_string(),
        false => table(&["PROJECT", "ENVIRONMENTS"], &rows, &out.style()),
    };
    Ok(Report::new(json!({ "projects": listed }), text).listing())
}

/// Rewrite the header when the folder is linked to the project that was renamed.
fn relink(cwd: &Path, org: &str, old: &str, slug: &str) -> Result<bool, CliError> {
    let Some(path) = find_schema(cwd) else {
        return Ok(false);
    };
    let source = read_file(&path)?;
    let Ok(schema) = penv_schema::parse(&source) else {
        return Ok(false);
    };
    let linked = schema
        .org
        .as_deref()
        .is_some_and(|o| o.eq_ignore_ascii_case(org))
        && schema
            .project
            .as_deref()
            .is_some_and(|p| p.eq_ignore_ascii_case(old));
    if !linked {
        return Ok(false);
    }
    write_file(&path, &penv_schema::set_header(&source, org, slug))?;
    Ok(true)
}

/// A delete erases every value and version, so a person types the name first.
pub(crate) fn confirm_erase(replay: &str, name: &str, agent: bool) -> Result<(), CliError> {
    if agent || !std::io::stdin().is_terminal() {
        return Err(CliError::new(
            "confirmation_required",
            format!(
                "deleting {name} erases every value in it for good, so a person has to confirm it."
            ),
            format!("Run it yourself in a terminal: {replay}"),
        )
        .with_exit(Exit::Confirmation)
        .with("replay", json!(replay)));
    }
    note(&format!(
        "This deletes {name} and every value in it. It cannot be undone."
    ));
    let title = format!("Type {name} to confirm");
    let typed = match ui::input(&title) {
        Some(answer) => answer.map_err(cancelled)?,
        None => prompt::read_line(&format!("{title}: "))?,
    };
    if typed.trim() == name {
        return Ok(());
    }
    Err(CliError::new(
        "cancelled",
        format!(
            "you typed {:?}, not {name}, so nothing was deleted.",
            typed.trim()
        ),
        format!("Run {replay} again to retry."),
    ))
}
