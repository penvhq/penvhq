//! Where the binary meets penv.cloud: the client, the keychain, the cache
//! directory, and the one place a `CloudError` becomes an exit code.

use std::io::Write;
use std::path::{Path, PathBuf};

use penv_agent::Detection;
use penv_cloud::api::{Address, Api, Bearer};
use penv_cloud::cache::Cache;
use penv_cloud::error::CloudError;
use penv_cloud::keychain::{Keyring, NoKeychain};
use penv_cloud::{Clock, Keychain, SystemClock, credential};
use penv_schema::{Key, Schema};
use serde_json::Value;

use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::SCHEMA_FILE;

/// The environment every command reads when nothing else says otherwise.
pub const DEFAULT_ENVIRONMENT: &str = "development";

pub struct Cloud {
    pub api: Api,
    pub keychain: Box<dyn Keychain>,
    pub cache_dir: Option<PathBuf>,
    pub now: u64,
}

impl Cloud {
    pub fn open(env: &Env, detection: &Detection) -> Result<Cloud, CliError> {
        let api = Api::from_env(env.as_map())
            .map_err(|e| refuse(e, None))?
            .stamped(detection.name(), detection.session_id.as_deref());
        let keychain: Box<dyn Keychain> = match Keyring::open(api.base_url()) {
            Some(keyring) => Box::new(keyring),
            None => Box::new(NoKeychain),
        };
        Ok(Cloud {
            cache_dir: penv_cloud::cache_dir(env.as_map()),
            api,
            keychain,
            now: SystemClock.now(),
        })
    }

    /// The credential this host can prove, whichever kind that turns out to be.
    pub fn bearer(&self, env: &Env, org: Option<&str>) -> Result<Bearer, CliError> {
        credential::resolve(env.as_map(), self.keychain.as_ref(), org)
            .and_then(|kind| kind.obtain(&self.api, self.now))
            .map_err(|e| refuse(e, None))
    }

    /// The `pcu_` a person's login left behind, and nothing else.
    pub fn user(&self) -> Result<Option<Bearer>, CliError> {
        self.keychain
            .get(penv_cloud::keychain::USER)
            .map(|held| held.filter(|v| !v.is_empty()).map(Bearer::new))
            .map_err(|e| refuse(e, None))
    }

    /// The cache is sealed against the credential that filled it, so it opens
    /// for this bearer and no other.
    pub fn cache(&self, at: &Address, bearer: &Bearer) -> Option<Cache> {
        let dir = self.cache_dir.as_deref()?;
        Cache::open(dir, self.api.base_url(), at, bearer, self.keychain.as_ref())
            .ok()
            .flatten()
    }

    /// Everything this host kept for this server.
    pub fn forget_cache(&self) {
        if let Some(dir) = self.cache_dir.as_deref() {
            penv_cloud::cache::forget(dir, self.api.base_url());
        }
    }
}

/// `--env`, else `PENV_ENV`, else development.
pub fn environment(flag: Option<&str>, env: &Env) -> String {
    flag.filter(|v| !v.is_empty())
        .or_else(|| env.get("PENV_ENV").filter(|v| !v.is_empty()))
        .unwrap_or(DEFAULT_ENVIRONMENT)
        .to_string()
}

/// The address the schema names. A schema with no header is local, and says so.
pub fn address(schema: &Schema, environment: &str) -> Result<Address, CliError> {
    match (&schema.org, &schema.project) {
        (Some(org), Some(project)) => Ok(Address::new(org, project, environment)),
        _ => Err(CliError::new(
            "not_cloud",
            format!("{SCHEMA_FILE} is not linked to a cloud project: it has no # @penv=org/project line."),
            "Run penv push. It creates the project and writes that line.",
        )),
    }
}

/// The per-key schema the cloud stores: what `penv schema --json` emits for
/// that key, minus the name the route already carries.
pub fn key_schema(key: &Key) -> Value {
    let mut json = key.to_json();
    if let Some(object) = json.as_object_mut() {
        object.remove("name");
    }
    json
}

/// One refusal shape for every cloud failure, with the exit code the design
/// publishes for it.
pub fn refuse(error: CloudError, at: Option<&Address>) -> CliError {
    let where_ = at.map(|a| a.to_string()).unwrap_or_default();
    match &error {
        CloudError::NoCredential => CliError::new(
            "no_credential",
            "you are not signed in on this machine.".to_string(),
            "Run penv login. On a server or in CI, set PENV_TOKEN.",
        )
        .with_exit(Exit::NoCredential),

        CloudError::Offline { .. } => CliError::new(
            "offline",
            error.to_string(),
            "Check your internet connection. Offline, penv run works only for development, from a local .env or from values it saved earlier.",
        )
        .with_exit(Exit::NoCredential),

        CloudError::Api(api) => match (api.status, api.code.as_str()) {
            (_, "cloned") => CliError::new(
                "cloned",
                "this machine's identity was used from a second machine, so the server locked it.",
                "Create a new enrolment secret in the console, then run penv machine enroll <secret>.",
            )
            .with_exit(Exit::Auth),
            (_, "dynamic") => CliError::new(
                "dynamic",
                "the cloud generates that key's value on demand, so the CLI cannot write it.",
                "Edit it in the console, or store your own value under a different key name.",
            )
            .with_exit(Exit::Validation),
            (_, "quota_exceeded") => CliError::new(
                "quota_exceeded",
                "your plan allows no more projects.",
                "Delete a project in the console, or upgrade the plan.",
            ),
            (_, "ambiguous") => CliError::new(
                "ambiguous",
                format!("{} matches more than one project or environment.", if where_.is_empty() { "that name" } else { &where_ }),
                "Rename one of them in the console, then run this again.",
            ),
            (401, "expired") => CliError::new(
                "expired",
                "your login expired.",
                "Run penv login. On a server or in CI, set a fresh PENV_TOKEN.",
            )
            .with_exit(Exit::Auth),
            (401, _) => CliError::new(
                "unauthorized",
                "the server rejected your login or token.",
                "Run penv login again. If PENV_TOKEN is set, check it is current.",
            )
            .with_exit(Exit::Auth),
            (403, _) => CliError::new(
                "environment_refused",
                format!("your account has no access to {where_}."),
                "Ask an admin to give you a role on that environment in the console, or pick another with --env <name>.",
            )
            .with_exit(Exit::EnvironmentRefused),
            (404, _) => CliError::new(
                "not_found",
                format!("{where_} does not exist on the server."),
                "Compare the # @penv=org/project line in .env.schema and the --env name with the console.",
            ),
            (429, _) => CliError::new(
                "rate_limited",
                match api.retry_after {
                    Some(seconds) => format!("too many requests. The server will accept more in {seconds}s."),
                    None => "too many requests.".to_string(),
                },
                "Wait, then run this again.",
            ),
            // The client already tried a server error a second time.
            (status, _) if status >= 500 => CliError::new(
                "server_error",
                format!("the server failed twice with HTTP {status}. The problem is on penv.cloud, not in your project."),
                "Wait a minute, then run this again.",
            ),
            (status, code) => CliError::new(
                "server_refused",
                format!("the server refused the request with HTTP {status} ({code})."),
                "Run penv check to find problems in .env.schema, fix them, then run this again.",
            ),
        },

        CloudError::Keychain(_) => CliError::new(
            "keychain",
            error.to_string(),
            "Unlock your system's password store (Keychain, Credential Manager), or set PENV_TOKEN.",
        ),

        CloudError::Url(_) => CliError::new(
            "bad_url",
            error.to_string(),
            "Set PENV_URL to an https address, or unset it.",
        ),

        _ => CliError::new(
            "cloud_failed",
            error.to_string(),
            "Run this again. If it keeps failing, run penv check to test your setup.",
        ),
    }
}

/// Everything penv says while a command is still working goes to stderr, so
/// stdout stays the one object the output contract promises.
pub fn note(line: &str) {
    let _ = writeln!(std::io::stderr(), "penv: {line}");
}

/// Open the verification page. A session with no terminal only prints it.
pub fn open_browser(url: &str) -> bool {
    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("cmd", vec!["/C", "start", "", url])
    } else if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else {
        ("xdg-open", vec![url])
    };
    std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// The project name `push` offers for a directory.
pub fn project_name(dir: &Path) -> String {
    dir.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "app".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use penv_cloud::error::ApiError;
    use penv_schema::{BaseType, Type};

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs)
    }

    #[test]
    fn the_environment_falls_back_from_the_flag_to_the_variable_to_development() {
        assert_eq!(environment(None, &env(&[])), "development");
        assert_eq!(
            environment(None, &env(&[("PENV_ENV", "staging")])),
            "staging"
        );
        assert_eq!(
            environment(Some("production"), &env(&[("PENV_ENV", "staging")])),
            "production"
        );
        assert_eq!(environment(Some(""), &env(&[])), "development");
    }

    #[test]
    fn a_forbidden_environment_is_exit_six_and_names_itself() {
        let at = Address::new("acme", "api", "production");
        let error = refuse(ApiError::new(403, "forbidden").into(), Some(&at));
        assert_eq!(error.exit, Exit::EnvironmentRefused);
        assert!(
            error.message.contains("acme/api/production"),
            "{}",
            error.message
        );
    }

    #[test]
    fn the_refusals_carry_the_codes_the_design_publishes() {
        for (error, exit, code) in [
            (
                CloudError::NoCredential,
                Exit::NoCredential,
                "no_credential",
            ),
            (
                CloudError::Offline {
                    url: "https://penv.cloud".into(),
                    reason: "no route".into(),
                },
                Exit::NoCredential,
                "offline",
            ),
            (
                ApiError::new(401, "unauthorized").into(),
                Exit::Auth,
                "unauthorized",
            ),
            (ApiError::new(409, "cloned").into(), Exit::Auth, "cloned"),
        ] {
            let refused = refuse(error, None);
            assert_eq!(refused.exit, exit, "{}", refused.code);
            assert_eq!(refused.code, code);
        }
    }

    #[test]
    fn the_per_key_schema_drops_the_name_the_route_already_carries() {
        let key = Key {
            name: "PORT".into(),
            ty: Type::new(BaseType::Port),
            required: true,
            ..Key::default()
        };
        let json = key_schema(&key);
        assert_eq!(json.get("name"), None);
        assert_eq!(json["type"]["name"], "port");
        assert_eq!(json["required"], true);
    }

    #[test]
    fn a_schema_with_no_header_is_not_an_address() {
        assert!(address(&Schema::default(), "development").is_err());
        let cloud = Schema {
            org: Some("acme".into()),
            project: Some("api".into()),
            ..Schema::default()
        };
        assert_eq!(
            address(&cloud, "development").unwrap().to_string(),
            "acme/api/development"
        );
    }

    #[test]
    fn a_project_is_named_after_its_directory() {
        assert_eq!(project_name(Path::new("/src/api-gateway")), "api-gateway");
        assert_eq!(project_name(Path::new("")), "app");
    }
}
