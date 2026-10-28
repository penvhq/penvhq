use std::io::IsTerminal;

use penv_cloud::provider::Capability;
use serde_json::json;

use crate::agent::detect_here;
use crate::cli::AppCommand;
use crate::commands::cloud::{Cloud, refuse};
use crate::env::Env;
use crate::error::CliError;
use crate::output::{Output, Report, table};
use crate::ui;

pub fn run(out: &Output, command: &AppCommand, env: &Env) -> Result<Report, CliError> {
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let cloud = Cloud::open(env, &detection)?;
    cloud.require(Capability::Manage)?;
    let bearer = cloud.bearer(env, None)?;

    match command {
        AppCommand::Ls => {
            let spinner = ui::spinner("Looking for your connected apps");
            let found = cloud
                .api
                .app_connections(&bearer)
                .map_err(|e| refuse(e, None))?;
            spinner.stop(&format!("Found {} connection(s)", found.len()));
            let rows: Vec<Vec<String>> = found
                .iter()
                .map(|c| {
                    vec![
                        c.id.clone(),
                        c.app.name.clone(),
                        format!("{}/{}/{}", c.org, c.project, c.environment),
                        c.keys.join(", "),
                        day(c.last_used_at.as_deref()).to_string(),
                    ]
                })
                .collect();
            let text = match rows.is_empty() {
                true => "No apps are connected to your environments.".to_string(),
                false => table(
                    &["ID", "APP", "ENVIRONMENT", "KEYS", "LAST USED"],
                    &rows,
                    &out.style(),
                ),
            };
            let listed: Vec<_> = found
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "app": { "clientId": c.app.client_id, "name": c.app.name },
                        "org": c.org,
                        "project": c.project,
                        "environment": c.environment,
                        "keys": c.keys,
                        "createdAt": c.created_at,
                        "lastUsedAt": c.last_used_at,
                    })
                })
                .collect();
            Ok(Report::new(json!({ "connections": listed }), text).listing())
        }
        AppCommand::Revoke { id } => {
            let spinner = ui::spinner(&format!("Revoking {id}"));
            cloud
                .api
                .revoke_app_connection(&bearer, id)
                .map_err(|e| refuse(e, None))?;
            spinner.stop("Revoked");
            Ok(Report::new(
                json!({ "revoked": id }),
                format!("revoked {id}: that app can no longer read your keys"),
            ))
        }
    }
}

/// The calendar day of an ISO timestamp, or `never`.
fn day(stamp: Option<&str>) -> &str {
    match stamp {
        Some(s) => s.get(..10).unwrap_or(s),
        None => "never",
    }
}

#[cfg(test)]
mod tests {
    use super::day;

    #[test]
    fn last_used_is_a_day_or_never() {
        assert_eq!(day(Some("2026-10-27T08:15:00.000Z")), "2026-10-27");
        assert_eq!(day(None), "never");
    }
}
