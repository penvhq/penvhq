use std::io::IsTerminal;
use std::path::Path;

use penv_agent::Policy;
use penv_cloud::api::{Address, Approval, Bearer, Fetched, Requested, host_name};
use penv_cloud::error::CloudError;
use serde_json::json;

use crate::agent::detect_here;
use crate::commands::cloud::{Cloud, address, environment, refuse};
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::output::{Output, Report};

/// Print one value. A person reads it straight; an agent session gets an
/// approval id instead, and prints the value only once a person has approved it.
pub fn run(
    _out: &Output,
    cwd: &Path,
    name: &str,
    env_flag: Option<&str>,
    env: &Env,
    agent_flag: bool,
    approval: Option<&str>,
) -> Result<Report, CliError> {
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let policy = Policy::for_(&detection, agent_flag);

    let (schema_path, schema) = super::load_schema(cwd)?;
    let at = address(&schema, &environment(env_flag, env, &schema, &schema_path))?;
    let cloud = Cloud::open(env, &detection)?;
    let bearer = cloud.bearer(env, schema.org.as_deref())?;

    match approval.map(str::trim).filter(|id| !id.is_empty()) {
        Some(id) => redeem(&cloud, &bearer, name, id),
        None if policy.reveal_allowed => read(&cloud, &bearer, &at, name),
        None => Err(ask(&cloud, &bearer, &at, name)),
    }
}

/// The value the environment holds, for a person nobody has to ask.
fn read(cloud: &Cloud, bearer: &Bearer, at: &Address, name: &str) -> Result<Report, CliError> {
    let Fetched::Body { body, .. } = cloud
        .api
        .env_get(bearer, at, None, true)
        .map_err(|e| refuse(e, Some(at)))?
    else {
        return Err(CliError::new(
            "unexpected_answer",
            "the server sent back no values.",
            "Run penv reveal again.",
        ));
    };

    let value = body
        .keys
        .iter()
        .find(|key| key.name == name)
        .and_then(|key| key.value.clone())
        .ok_or_else(|| {
            CliError::new(
                "no_value",
                format!("{name} has no value in {at}."),
                format!("Run penv ls to see the keys, or penv set {name} to give it one."),
            )
            .with_exit(Exit::Validation)
        })?;

    Ok(Report::new(json!({ "key": name, "value": value }), value))
}

/// Ask the console. The answer is the id and the page, never a value, and the
/// same request already open for this key and session is reused.
fn ask(cloud: &Cloud, bearer: &Bearer, at: &Address, name: &str) -> CliError {
    match cloud.api.approval_create(bearer, at, name, &host_name()) {
        Ok(Requested::Created(approval)) => waiting(
            name,
            &approval,
            format!("{name} needs a person to approve the reveal."),
        ),
        // Asking twice in one session is the same request, so the answer says so
        // rather than reading as a second one nobody has seen.
        Ok(Requested::Pending(approval)) => waiting(
            name,
            &approval,
            format!("{name} already has an open approval."),
        ),
        Err(e) => refuse(e, Some(at)),
    }
}

/// Redeem what a person approved. Every refusal says whether the answer is to
/// wait, to give up, or to ask again.
fn redeem(cloud: &Cloud, bearer: &Bearer, name: &str, id: &str) -> Result<Report, CliError> {
    // Which key the approval is for is read before it is spent: redeeming one
    // that was asked for another key would print a value nobody approved.
    let asked = cloud.api.approval(bearer, id).ok();
    if let Some(key) = asked
        .as_ref()
        .and_then(|approval| approval.key.as_deref())
        .filter(|key| *key != name)
    {
        return Err(mismatch(name, key, id));
    }

    let revealed = match cloud.api.approval_redeem(bearer, id) {
        Ok(revealed) => revealed,
        Err(CloudError::Api(api)) => {
            return Err(match api.code.as_str() {
                "approval_pending" => pending(name, id, asked.as_ref()),
                "approval_denied" => CliError::new(
                    "approval_denied",
                    format!("the console refused the reveal of {name}."),
                    format!("Ask whoever denied it, then run penv reveal {name} to ask again."),
                )
                .with_exit(Exit::Auth),
                "approval_expired" => again(
                    "approval_expired",
                    name,
                    id,
                    format!("approval {id} expired before it was redeemed."),
                ),
                "approval_redeemed" => again(
                    "approval_redeemed",
                    name,
                    id,
                    format!(
                        "approval {id} was redeemed already, and each one prints a value once."
                    ),
                ),
                "not_found" => again(
                    "no_approval",
                    name,
                    id,
                    format!("approval {id} is not on this server."),
                ),
                _ => refuse(CloudError::Api(api), None),
            });
        }
        Err(other) => return Err(refuse(other, None)),
    };

    // A server that named no key on the status route is caught here instead.
    if revealed.key != name {
        return Err(mismatch(name, &revealed.key, id));
    }
    Ok(Report::new(
        json!({ "key": revealed.key, "value": revealed.value }),
        revealed.value,
    ))
}

/// A request nobody has answered yet. The status route says where the page is;
/// without it the id is all there is to hand back.
fn pending(name: &str, id: &str, approval: Option<&Approval>) -> CliError {
    let message = format!("approval {id} for {name} is not yet approved.");
    match approval {
        Some(approval) => waiting(name, approval, message),
        None => CliError::new("approval_required", message, replay(name, id))
            .with_exit(Exit::Confirmation)
            .with("approval", json!(id)),
    }
}

/// An approval belongs to the key it was asked for. Spending it on another key
/// would print a value nobody released, so it is left unspent.
fn mismatch(name: &str, key: &str, id: &str) -> CliError {
    CliError::new(
        "approval_mismatch",
        format!("approval {id} is for {key}, not {name}."),
        format!("Run penv reveal {name} for its own approval."),
    )
    .with_exit(Exit::Confirmation)
    .with("approval", json!(id))
}

/// Exit 4 and the whole answer an agent gets: which request, which page, how
/// long it lasts, and the command that prints the value once it is approved.
fn waiting(name: &str, approval: &Approval, message: String) -> CliError {
    let fix = if approval.url.is_empty() {
        replay(name, &approval.id)
    } else {
        format!(
            "A person approves at {}, then run penv reveal {name} --approval {}.",
            approval.url, approval.id
        )
    };
    CliError::new("approval_required", message, fix)
        .with_exit(Exit::Confirmation)
        .with("approval", json!(approval.id))
        .with("url", json!(none_if_empty(&approval.url)))
        .with(
            "expiresAt",
            approval
                .expires_at
                .clone()
                .unwrap_or(serde_json::Value::Null),
        )
}

/// An id that will never print a value: the fix is a new request.
fn again(code: &'static str, name: &str, id: &str, message: String) -> CliError {
    CliError::new(
        code,
        message,
        format!("Run penv reveal {name} for a new approval."),
    )
    .with_exit(Exit::Confirmation)
    .with("approval", json!(id))
}

fn none_if_empty(text: &str) -> Option<&str> {
    Some(text).filter(|text| !text.is_empty())
}

fn replay(name: &str, id: &str) -> String {
    format!("A person approves it in the console, then run penv reveal {name} --approval {id}.")
}
