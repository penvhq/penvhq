//! Where the binary meets penv.cloud: the client, the keychain, the cache
//! directory, and the one place a `CloudError` becomes an exit code.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use penv_agent::Detection;
use penv_cloud::api::{Address, Api, Bearer};
use penv_cloud::cache::Cache;
use penv_cloud::credential::Obtain;
use penv_cloud::error::CloudError;
use penv_cloud::keychain::{Keyring, NoKeychain, Remembering};
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
    /// The root came from config.toml: environment credentials stay home.
    pub withhold_env: bool,
}

impl Cloud {
    pub fn open(env: &Env, detection: &Detection) -> Result<Cloud, CliError> {
        trust(env, detection.is_agent() || crate::agent::flagged())?;
        let chosen = crate::providers::chosen(env)?;
        let api = Api::new(&chosen.url)
            .map_err(|e| refuse(e, None))?
            .stamped(detection.name(), detection.session_id.as_deref());
        let keychain: Box<dyn Keychain> = match Keyring::open(api.base_url()) {
            Some(keyring) => Box::new(Remembering::new(keyring)),
            None => Box::new(NoKeychain),
        };
        Ok(Cloud {
            cache_dir: penv_cloud::cache_dir(env.as_map()),
            withhold_env: chosen.from_config,
            api,
            keychain,
            now: SystemClock.now(),
        })
    }

    /// The credential this host can prove, whichever kind that turns out to be.
    pub fn bearer(&self, env: &Env, org: Option<&str>) -> Result<Bearer, CliError> {
        self.credential(env, org)?
            .obtain(&self.api, self.now)
            .map_err(|e| refuse(e, None))
    }

    /// Which credential this host would prove itself with, found without
    /// asking the network.
    pub fn credential(
        &self,
        env: &Env,
        org: Option<&str>,
    ) -> Result<Box<dyn Obtain + '_>, CliError> {
        if self.withhold_env {
            // Only what this machine holds for this root: its login or keypair.
            return match credential::resolve_held(self.keychain.as_ref(), self.cache_dir.clone())
            {
                Ok(kind) => Ok(kind),
                Err(penv_cloud::CloudError::NoCredential)
                    if credential::present(env.as_map(), self.keychain.as_ref()) =>
                {
                    Err(CliError::new(
                        "credential_withheld",
                        format!(
                            "{} comes from .penv/config.toml, so penv does not send it PENV_TOKEN, a CI token or an AWS proof.",
                            self.api.base_url()
                        ),
                        format!(
                            "Sign in there with penv login, or set PENV_URL={} to send those credentials on purpose.",
                            self.api.base_url()
                        ),
                    )
                    .with_exit(Exit::NoCredential))
                }
                Err(e) => Err(refuse(e, None)),
            };
        }
        credential::resolve(env.as_map(), self.keychain.as_ref(), org).map_err(|e| refuse(e, None))
    }

    /// The `pcu_` a person's login left behind, and nothing else.
    pub fn user(&self) -> Result<Option<Bearer>, CliError> {
        self.keychain
            .get(penv_cloud::keychain::USER)
            .map(|held| held.filter(|v| !v.is_empty()).map(Bearer::new))
            .map_err(|e| refuse(e, None))
    }

    /// The cache is sealed against the identity of the credential that filled
    /// it, so it opens for that credential and no other.
    pub fn cache(&self, at: &Address, credential: &dyn Obtain) -> Option<Cache> {
        let dir = self.cache_dir.as_deref()?;
        Cache::open(
            dir,
            self.api.base_url(),
            at,
            credential,
            self.keychain.as_ref(),
        )
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

/// The environment a person named, or the one they pick from the project's list
/// at a terminal, or development.
pub fn pick_environment(
    cloud: &Cloud,
    bearer: &Bearer,
    schema: &Schema,
    schema_path: &Path,
    flag: Option<&str>,
    env: &Env,
    may_ask: bool,
) -> Result<String, CliError> {
    let dir = schema_path.parent().unwrap_or(Path::new("."));
    if let Some(named) = crate::source::named_environment(flag, env, schema, dir) {
        return Ok(named);
    }
    let fallback = crate::source::DEFAULT_ENVIRONMENT.to_string();
    let (Some(org), Some(project), true) = (&schema.org, &schema.project, may_ask) else {
        return Ok(fallback);
    };
    let names = cloud
        .api
        .projects(bearer, org)
        .map_err(|e| refuse(e, None))?
        .into_iter()
        .find(|p| p.slug.eq_ignore_ascii_case(project))
        .map(|p| p.environments)
        .unwrap_or_default();
    if names.len() < 2 {
        return Ok(names.into_iter().next().unwrap_or(fallback));
    }
    let initial = names.iter().position(|n| *n == fallback);
    match crate::ui::select("Which environment?", &names, initial) {
        Some(picked) => picked.map(|i| names[i].clone()).map_err(cancelled),
        None => Ok(fallback),
    }
}

/// `--env`, else `PENV_ENV`, else `@currentEnv`, else development.
pub fn environment(flag: Option<&str>, env: &Env, schema: &Schema, schema_path: &Path) -> String {
    let dir = schema_path.parent().unwrap_or(Path::new("."));
    crate::source::environment(flag, env, schema, dir)
}

/// The address the schema names. A schema with no header is local, and says so.
pub fn address(schema: &Schema, environment: &str) -> Result<Address, CliError> {
    match (&schema.org, &schema.project) {
        (Some(org), Some(project)) => Ok(Address::new(org, project, environment)),
        _ => Err(CliError::new(
            "not_cloud",
            format!(
                "{SCHEMA_FILE} is not linked to a cloud project: it has no # @penv=org/project line."
            ),
            "Run penv pull to link it to a project you already have, or penv push to create one from this folder.",
        )),
    }
}

/// Give a schema with no header one of the projects this account can see, and
/// write the header. One project is taken; several are asked for at a terminal.
pub fn link(
    cloud: &Cloud,
    bearer: &Bearer,
    schema_path: &Path,
    schema: &mut Schema,
    may_ask: bool,
) -> Result<(), CliError> {
    let spinner = crate::ui::spinner("Looking for your projects");
    let mut found: Vec<String> = Vec::new();
    for org in cloud.api.orgs(bearer).map_err(|e| refuse(e, None))? {
        let projects = cloud
            .api
            .projects(bearer, &org.slug)
            .map_err(|e| refuse(e, None))?;
        found.extend(projects.iter().map(|p| format!("{}/{}", org.slug, p.slug)));
    }
    spinner.stop(&format!("Found {} project(s)", found.len()));

    let chosen = match found.len() {
        0 => {
            return Err(CliError::new(
                "no_project",
                "your account has no cloud project yet.",
                "Run penv push to create one from this folder.",
            ));
        }
        1 => found.remove(0),
        _ if may_ask => ask_project(&found)?,
        _ => {
            return Err(CliError::new(
                "project_required",
                format!(
                    "{SCHEMA_FILE} is not linked, and your account has {} projects: {}.",
                    found.len(),
                    found.join(", ")
                ),
                format!(
                    "Make this the first line of {SCHEMA_FILE}: # @penv=<org>/<project> @schema=1"
                ),
            ));
        }
    };

    let (org, project) = chosen.split_once('/').unwrap_or((&chosen, ""));
    schema.org = Some(org.to_string());
    schema.project = Some(project.to_string());
    let source = crate::files::read_file(schema_path)?;
    crate::files::write_file(schema_path, &penv_schema::set_header(&source, org, project))?;
    note(&format!("linked {SCHEMA_FILE} to {chosen}."));
    Ok(())
}

fn ask_project(found: &[String]) -> Result<String, CliError> {
    if let Some(picked) = crate::ui::select("Which project is this folder?", found, None) {
        return picked.map(|index| found[index].clone()).map_err(cancelled);
    }
    let mut prompt = String::from("Which project is this folder?\n");
    for (index, name) in found.iter().enumerate() {
        prompt.push_str(&format!("  {}. {name}\n", index + 1));
    }
    prompt.push_str("Number: ");
    let answer = crate::prompt::read_line(&prompt)?;
    answer
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|number| found.get(number.checked_sub(1)?))
        .cloned()
        .ok_or_else(|| {
            CliError::new(
                "unreadable_selection",
                format!(
                    "{} is not one of the {} projects listed.",
                    answer.trim(),
                    found.len()
                ),
                "Run penv pull again and answer with a number such as 1.",
            )
        })
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

/// Write with every key's schema as written, and when penv.cloud refuses a
/// schema field it does not store yet (`hosts`, docs/Cloud-API.md), write once
/// more without it. The committed .env.schema keeps @hosts either way; the
/// warning says the cloud copy lacks it.
pub fn with_hosts_fallback<T>(
    keys: &mut [penv_cloud::api::CloudKey],
    mut write: impl FnMut(&[penv_cloud::api::CloudKey]) -> Result<T, penv_cloud::CloudError>,
) -> Result<T, penv_cloud::CloudError> {
    let carries_hosts = keys
        .iter()
        .any(|k| k.schema.as_ref().is_some_and(|s| s.get("hosts").is_some()));
    match write(keys) {
        Err(penv_cloud::CloudError::Api(e))
            if carries_hosts && e.status == 400 && e.code == "schema_invalid" =>
        {
            for key in keys.iter_mut() {
                if let Some(schema) = key.schema.as_mut().and_then(|s| s.as_object_mut()) {
                    schema.remove("hosts");
                }
            }
            let written = write(keys)?;
            crate::ui::warn(
                "penv.cloud does not store @hosts yet, so the cloud copy of the schema lacks it; .env.schema keeps it.",
            );
            Ok(written)
        }
        other => other,
    }
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
            (_, "project_taken") => CliError::new(
                "project_taken",
                "a project with that name already exists in this workspace.",
                "Pick another name, or point @penv= at the existing project.",
            )
            .with_exit(Exit::Validation),
            (_, "org_ambiguous") => CliError::new(
                "org_ambiguous",
                "more than one workspace answers to the org in @penv=, and both trust this identity.",
                "Rename one of the two workspaces in the console so their slugs differ, then run this again.",
            )
            .with_exit(Exit::Auth),
            (_, "undecryptable") => CliError::new(
                "undecryptable",
                format!("the cloud holds a value for {} but cannot decrypt it; the workspace's key may have been revoked or its KMS access removed.", if where_.is_empty() { "this key" } else { &where_ }),
                "Check the workspace's encryption key in the console, or set the value again with penv set.",
            ),
            (_, "name_invalid") => CliError::new(
                "name_invalid",
                "the cloud stores upper-case key names only: A-Z, 0-9 and _, not starting with a digit.",
                "Rename the key in .env.schema and your code, then run this again.",
            )
            .with_exit(Exit::Validation),
            (_, "exists") => CliError::new(
                "exists",
                "that name is already taken in this project.",
                "Pick another name, or run penv env ls to see the ones in use.",
            )
            .with_exit(Exit::Validation),
            (_, "live_leases") => CliError::new(
                "live_leases",
                "it still has temporary credentials in use, so the server will not delete it yet.",
                "Revoke them in the console, or wait for them to expire, then run this again.",
            ),
            (_, "name_required") => CliError::new(
                "name_required",
                "the name is empty, or has no letters or digits in it.",
                "Use a name such as billing or staging.",
            )
            .with_exit(Exit::Validation),
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
            (403, _) if at.is_none() => CliError::new(
                "forbidden",
                "your account is not allowed to do that. Servers and CI tokens can never create, rename or delete.",
                "Sign in as a person with penv login, or ask an admin for the role in the console.",
            )
            .with_exit(Exit::Auth),
            (403, _) => CliError::new(
                "environment_refused",
                format!("your account has no access to {where_}."),
                "Ask an admin to give you a role on that environment in the console, or pick another with --env <name>.",
            )
            .with_exit(Exit::EnvironmentRefused),
            (404, _) if at.is_none() => CliError::new(
                "not_found",
                "that project or environment does not exist on the server.",
                "Run penv project ls to see the names, then run this again.",
            ),
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

        CloudError::DotSegment(part) => CliError::new(
            "dot_name",
            error.to_string(),
            format!("Give the {part} a name with a letter or digit in it, in the console and in .env.schema."),
        )
        .with_exit(Exit::Validation),

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
    crate::ui::note(line);
}

/// A picker left with Esc or Ctrl-C.
pub fn cancelled(_: std::io::Error) -> CliError {
    CliError::new(
        "cancelled",
        "nothing was chosen, so nothing changed.",
        "Run the command again to choose.",
    )
}

/// Settle which certificate authorities penv trusts before the first request:
/// the compiled-in roots, or the bundle `SSL_CERT_FILE` names.
pub fn trust(env: &Env, agent: bool) -> Result<(), CliError> {
    penv_cloud::tls::configure(env.as_map(), agent).map_err(|message| {
        CliError::new(
            "untrusted_ca_bundle",
            message,
            "Unset SSL_CERT_FILE to use the roots built into penv, or point it at a bundle the system owns, such as /etc/ssl/certs/ca-certificates.crt.",
        )
    })
}

/// Open a page the server named, when it is one penv would send a credential
/// to on the same host as the API; anything else is only printed, for the
/// person to open themselves. No shell ever reads the URL.
pub fn open_browser(url: &str, base_url: &str) -> bool {
    if !openable(url, base_url) {
        return false;
    }
    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("rundll32", vec!["url.dll,FileProtocolHandler", url])
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

/// https on the API's own host. Plain http only where the API itself is the
/// loopback server a test or a local build runs.
fn openable(url: &str, base_url: &str) -> bool {
    let (Ok(url), Some(base_host)) = (
        penv_cloud::api::checked_url(url),
        penv_cloud::api::url_host(base_url),
    ) else {
        return false;
    };
    let secure = url.starts_with("https://") || base_url.starts_with("http://");
    secure && penv_cloud::api::url_host(&url).is_some_and(|host| host == base_host)
}

/// The project name `push` offers for a directory.
pub fn project_name(dir: &Path) -> String {
    dir.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "app".to_string())
}

/// Reads environments for one command: the cloud opens once, a bearer is minted
/// once per org and only when the server has to be asked, and each address is
/// read once through this host's cache.
pub struct Fetcher<'a> {
    env: &'a Env,
    detection: &'a Detection,
    cloud: Option<Cloud>,
    bearers: Vec<(String, Bearer)>,
    read: Vec<(String, Vec<penv_cloud::api::CloudKey>)>,
}

impl<'a> Fetcher<'a> {
    pub fn new(env: &'a Env, detection: &'a Detection) -> Fetcher<'a> {
        Fetcher {
            env,
            detection,
            cloud: None,
            bearers: Vec::new(),
            read: Vec::new(),
        }
    }

    /// The keys stored at `at`, values and write times included.
    pub fn keys(&mut self, at: &Address) -> Result<Vec<penv_cloud::api::CloudKey>, CliError> {
        let label = at.to_string();
        if let Some((_, keys)) = self.read.iter().find(|(l, _)| *l == label) {
            return Ok(keys.clone());
        }
        if self.cloud.is_none() {
            self.cloud = Some(Cloud::open(self.env, self.detection)?);
        }
        let cloud = self.cloud.as_ref().expect("opened above");
        let minted = self
            .bearers
            .iter()
            .find(|(org, _)| org.eq_ignore_ascii_case(&at.org))
            .map(|(_, bearer)| bearer.clone());
        let credential = Once {
            kind: cloud.credential(self.env, Some(&at.org))?,
            minted: RefCell::new(minted),
        };
        let cache = cloud.cache(at, &credential);
        let spinner = crate::ui::spinner(&format!("Reading {at}"));
        let resolved =
            penv_cloud::cache::fetch(&cloud.api, &credential, at, cache.as_ref(), cloud.now)
                .map_err(|e| refuse(e, Some(at)))?;
        spinner.stop(&format!("Read {at}"));
        if let Some(bearer) = credential.minted.into_inner()
            && !self
                .bearers
                .iter()
                .any(|(org, _)| org.eq_ignore_ascii_case(&at.org))
        {
            self.bearers.push((at.org.clone(), bearer));
        }
        if resolved.offline_warning {
            crate::ui::warn(&format!(
                "{at} could not be reached, so this used the development values penv saved last time."
            ));
        }
        if !resolved.body.skipped.is_empty() {
            crate::ui::warn(&format!(
                "no stored value in {at}: {}. Set one with penv set <KEY>.",
                resolved.body.skipped.join(", ")
            ));
        }
        self.read.push((label, resolved.body.keys.clone()));
        Ok(resolved.body.keys)
    }

    pub fn values(&mut self, at: &Address) -> Result<penv_schema::Values, CliError> {
        Ok(self
            .keys(at)?
            .into_iter()
            .filter_map(|key| Some((key.name, key.value?)))
            .collect())
    }
}

/// A credential proved at most once per command, however many addresses ask.
struct Once<'k> {
    kind: Box<dyn Obtain + 'k>,
    minted: RefCell<Option<Bearer>>,
}

impl Obtain for Once<'_> {
    fn obtain(&self, api: &Api, now: u64) -> penv_cloud::Result<Bearer> {
        if let Some(bearer) = self.minted.borrow().clone() {
            return Ok(bearer);
        }
        let bearer = self.kind.obtain(api, now)?;
        *self.minted.borrow_mut() = Some(bearer.clone());
        Ok(bearer)
    }

    fn identity(&self) -> Option<String> {
        self.kind.identity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use penv_cloud::error::ApiError;
    use penv_schema::{BaseType, Type};

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
    fn each_cloud_refusal_the_cli_can_act_on_is_named_for_what_it_means() {
        let at = Address::new("acme", "api", "production");
        for (status, code, says) in [
            (
                409,
                "project_taken",
                "a project with that name already exists",
            ),
            (409, "org_ambiguous", "more than one workspace answers"),
            (409, "undecryptable", "cannot decrypt it"),
            (400, "name_invalid", "upper-case key names only"),
        ] {
            let error = refuse(ApiError::new(status, code).into(), Some(&at));
            assert_eq!(error.code, code);
            assert!(error.message.contains(says), "{code}: {}", error.message);
        }
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
    fn only_a_page_on_the_api_s_own_host_is_opened() {
        let base = "https://penv.cloud";
        assert!(openable("https://penv.cloud/device", base));
        assert!(openable("https://PENV.cloud/device?code=AB&x=1", base));
        for url in [
            "https://evil.example/device",
            "https://penv.cloud.evil.example/device",
            "http://penv.cloud/device",
            "https://u@penv.cloud/device",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "calc.exe",
            "https://evil.example/&calc",
        ] {
            assert!(!openable(url, base), "{url}");
        }
        let local = "http://127.0.0.1:8787";
        assert!(openable("http://127.0.0.1:8787/device", local));
        assert!(!openable("http://localhost:8787/device", local));
    }

    #[test]
    fn a_name_made_only_of_dots_says_which_part_to_rename() {
        let refused = refuse(CloudError::DotSegment("environment"), None);
        assert_eq!(refused.code, "dot_name");
        assert!(refused.fix.contains("environment"), "{}", refused.fix);
    }

    #[test]
    fn a_project_is_named_after_its_directory() {
        assert_eq!(project_name(Path::new("/src/api-gateway")), "api-gateway");
        assert_eq!(project_name(Path::new("")), "app");
    }
}
