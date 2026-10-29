use std::io::IsTerminal;

use penv_cloud::CloudError;
use penv_cloud::provider::Capability;
use serde_json::json;

use crate::agent::detect_here;
use crate::cli::AppCommand;
use crate::commands::cloud::{Cloud, refuse};
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::output::{Output, Report, table};
use crate::ui;

pub fn run(
    out: &Output,
    command: &AppCommand,
    env: &Env,
    agent_flag: bool,
) -> Result<Report, CliError> {
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let cloud = Cloud::open(env, &detection)?;
    cloud.require(Capability::Manage)?;
    let bearer = cloud.bearer(env, None)?;
    let agent = detection.is_agent() || agent_flag;

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
            let listed = serde_json::to_value(&found)
                .map_err(|e| CliError::new("internal", e.to_string(), "Report this as a bug."))?;
            Ok(Report::new(json!({ "connections": listed }), text).listing())
        }
        AppCommand::Revoke { id } => {
            if agent || !std::io::stdin().is_terminal() {
                let replay = format!("penv app revoke {id}");
                return Err(CliError::new(
                    "confirmation_required",
                    format!(
                        "revoking {id} cuts that app off for good, so a person has to confirm it."
                    ),
                    format!("Run it yourself in a terminal: {replay}"),
                )
                .with_exit(Exit::Confirmation)
                .with("replay", json!(replay)));
            }
            let spinner = ui::spinner(&format!("Revoking {id}"));
            cloud
                .api
                .revoke_app_connection(&bearer, id)
                .map_err(|e| revoke_refusal(e, id))?;
            spinner.stop("Revoked");
            Ok(Report::new(
                json!({ "revoked": id }),
                format!("revoked {id}: that app can no longer read your keys"),
            ))
        }
    }
}

/// A 404 on revoke names the connection; every other refusal is the shared one.
fn revoke_refusal(error: CloudError, id: &str) -> CliError {
    match error {
        CloudError::Api(ref api) if api.status == 404 => CliError::new(
            "not_found",
            format!("no connection {id} exists, or it is already revoked."),
            "Run `penv app ls` for the ids you can revoke.",
        ),
        other => refuse(other, None),
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
    use super::{day, revoke_refusal};
    use penv_cloud::CloudError;
    use penv_cloud::error::ApiError;

    #[test]
    fn last_used_is_a_day_or_never() {
        assert_eq!(day(Some("2026-10-27T08:15:00.000Z")), "2026-10-27");
        assert_eq!(day(None), "never");
    }

    #[test]
    fn a_missing_connection_names_the_id_and_the_fix() {
        let api = ApiError {
            status: 404,
            code: "not_found".into(),
            retry_after: None,
            message: None,
            detail: None,
            exchange: None,
        };
        let error = revoke_refusal(CloudError::Api(api), "conn_9");
        assert_eq!(error.code, "not_found");
        assert_eq!(
            error.message,
            "no connection conn_9 exists, or it is already revoked."
        );
        assert_eq!(error.fix, "Run `penv app ls` for the ids you can revoke.");
    }
}
