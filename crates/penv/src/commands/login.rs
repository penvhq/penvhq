use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

use penv_cloud::Clock;
use penv_cloud::api::DevicePoll;
use serde_json::json;

use crate::agent::detect_here;
use crate::commands::cloud::{Cloud, note, open_browser, refuse};
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::output::{Output, Report};

/// Sign a person in with a device code. The credential goes to the OS keychain
/// and nowhere else.
pub fn run(out: &Output, _cwd: &Path, env: &Env, agent_flag: bool) -> Result<Report, CliError> {
    let tty = std::io::stdout().is_terminal();
    let detection = detect_here(env, tty);
    if detection.is_agent() || agent_flag {
        return Err(CliError::new(
            "agent_session",
            format!(
                "penv login signs a person in, and this session is {}.",
                detection.name().unwrap_or("an agent")
            ),
            "Sign in yourself, or give the agent a machine credential in PENV_TOKEN.",
        )
        .with_exit(Exit::Auth));
    }

    let cloud = Cloud::open(env, &detection)?;
    // A sign-in with nowhere to keep the credential would leave a live one orphaned.
    if !cloud.keychain.usable() {
        return Err(CliError::new(
            "no_keychain",
            "this host has no keychain, so the credential a sign-in makes would have nowhere to go.",
            "Sign in where the OS keychain opens, or give this host a PENV_TOKEN.",
        ));
    }
    let start = cloud
        .api
        .device_start(&penv_cloud::api::host_name())
        .map_err(|e| refuse(e, None))?;

    note(&format!("your code is {}", start.user_code));
    note(&format!("open {}", start.verification_uri));
    if tty && open_browser(&start.verification_uri, cloud.api.base_url()) {
        note("a browser was opened for you");
    }

    let grant = poll(&cloud, &start)?;
    cloud
        .keychain
        .set(penv_cloud::keychain::USER, &grant.credential)
        .map_err(|e| refuse(e, None))?;

    let email = grant.user.as_ref().and_then(|u| u.email.clone());
    let orgs: Vec<String> = grant.orgs.iter().map(|o| o.slug.clone()).collect();
    let style = out.style();
    let text = format!(
        "{} {}\n{} {}",
        style.green("signed in"),
        email.clone().unwrap_or_else(|| "this account".into()),
        style.dim("orgs"),
        if orgs.is_empty() {
            "none yet".to_string()
        } else {
            orgs.join(", ")
        }
    );

    Ok(Report::new(
        json!({
            "signedIn": true,
            "email": email,
            "orgs": grant.orgs.iter().map(|o| json!({ "slug": o.slug, "name": o.name })).collect::<Vec<_>>(),
            "server": cloud.api.base_url(),
        }),
        text,
    ))
}

/// The longest a poll waits, however long the server asks for.
const MAX_INTERVAL: u64 = 60;

/// Wait the interval the server named, and lengthen it whenever it says so.
fn poll(cloud: &Cloud, start: &penv_cloud::DeviceStart) -> Result<penv_cloud::Grant, CliError> {
    let (mut interval, deadline) = schedule(cloud.now, start);
    loop {
        std::thread::sleep(Duration::from_secs(interval));
        match cloud
            .api
            .device_poll(&start.device_code)
            .map_err(|e| refuse(e, None))?
        {
            DevicePoll::Granted(grant) => return Ok(grant),
            DevicePoll::Pending => {}
            // Any 429 is a wait, never a failure: the server says how long.
            DevicePoll::SlowDown(retry_after) => {
                interval = retry_after.unwrap_or(interval).clamp(1, MAX_INTERVAL)
            }
            DevicePoll::Denied => {
                return Err(CliError::new(
                    "denied",
                    "the sign-in was denied in the console.",
                    "Run penv login again and approve the code it shows.",
                )
                .with_exit(Exit::Auth));
            }
            DevicePoll::Expired => return Err(expired()),
        }
        if penv_cloud::SystemClock.now() >= deadline {
            return Err(expired());
        }
    }
}

/// The first wait and the moment to give up, whatever numbers the server sent.
fn schedule(now: u64, start: &penv_cloud::DeviceStart) -> (u64, u64) {
    (
        start.interval.clamp(1, MAX_INTERVAL),
        now.saturating_add(start.expires_in),
    )
}

fn expired() -> CliError {
    CliError::new(
        "expired",
        "the code expired before it was approved.",
        "Run penv login again.",
    )
    .with_exit(Exit::Auth)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(interval: u64, expires_in: u64) -> penv_cloud::DeviceStart {
        penv_cloud::DeviceStart {
            device_code: "dc_FAKE".into(),
            user_code: "WXYZ-1234".into(),
            verification_uri: "https://penv.cloud/device".into(),
            expires_in,
            interval,
        }
    }

    #[test]
    fn a_server_s_numbers_never_make_a_poll_wait_forever_or_overflow() {
        assert_eq!(schedule(1_000, &start(5, 600)), (5, 1_600));
        assert_eq!(schedule(1_000, &start(0, 600)).0, 1);
        assert_eq!(schedule(1_000, &start(u64::MAX, 600)).0, MAX_INTERVAL);
        assert_eq!(schedule(1_000, &start(5, u64::MAX)).1, u64::MAX);
    }
}
