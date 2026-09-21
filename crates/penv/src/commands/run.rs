use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread::JoinHandle;

use penv_agent::Policy;
use penv_mask::Masker;
use penv_schema::{Schema, Values, extras, validate};

use crate::agent::detect_here;
use crate::commands::cloud;
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::{ENV_FILE, SCHEMA_FILE, find_schema, on_path, read_file, show};
use crate::output::{Output, Report};

/// The only environment local mode has. The rest live in the cloud.
pub const DEFAULT_ENVIRONMENT: &str = "development";

/// Validate, then hand the values to one child process and nothing else. The
/// child's output is this command's output, so penv writes only to stderr and
/// leaves with the child's exit code.
pub fn run(
    out: &Output,
    cwd: &Path,
    environment: Option<&str>,
    no_mask: bool,
    argv: &[String],
    process_env: &Env,
    agent_flag: bool,
) -> Result<Report, CliError> {
    if argv.is_empty() {
        return Err(CliError::new(
            "no_command",
            "penv run has nothing to run.",
            "Put the command after --, as in penv run -- npm start.",
        ));
    }
    let mut stderr = std::io::stderr();

    // The design has init fire when run meets a .env with no schema for it.
    if find_schema(cwd).is_none() && cwd.join(ENV_FILE).is_file() {
        // Mid-run is no place for a picker, so this takes the installed set.
        let inferred = super::init::run(
            out,
            cwd,
            false,
            &super::init::Guards::Installed,
            None,
            process_env,
            agent_flag,
        )?;
        let _ = writeln!(
            stderr,
            "penv: there was no {SCHEMA_FILE}, so penv init wrote one from {ENV_FILE}."
        );
        let _ = out.write(&inferred, &mut stderr);
    }

    let (schema_path, schema) = super::load_schema(cwd)?;
    let dir = schema_path.parent().unwrap_or(cwd);
    let env_path = dir.join(ENV_FILE);
    let has_env = env_path.is_file();

    let stdout_tty = std::io::stdout().is_terminal();
    let detection = detect_here(process_env, stdout_tty);
    let agent = detection.is_agent() || agent_flag;
    let policy = Policy::for_(&detection, agent_flag);

    // A header means the cloud; the local file only stands in while offline.
    let fetched = if schema.is_cloud() {
        let name = cloud::environment(environment, process_env);
        match cloud_values(&schema, &name, process_env, &detection, &mut stderr) {
            Ok(values) => Some((name, values)),
            Err(error) if error.code == "offline" && has_env => {
                let _ = writeln!(
                    stderr,
                    "penv: the cloud could not be reached, so this run used the local {ENV_FILE}."
                );
                None
            }
            Err(error) => return Err(error),
        }
    } else {
        None
    };

    let (environment, mut values) = match fetched {
        Some(pair) => pair,
        None => {
            let name = resolve_environment(environment, process_env)?;
            let values = if has_env {
                penv_dotenv::read(&read_file(&env_path)?).values()
            } else {
                Values::new()
            };
            (name, values)
        }
    };
    apply_defaults(&schema, &mut values);

    let violations = validate(&schema, &values);
    if !violations.is_empty() {
        let style = out.style();
        let text = violations
            .iter()
            .map(|v| format!("{} {}", style.red("fail"), v.message))
            .collect::<Vec<_>>()
            .join("\n");
        let body = super::check::body(
            &schema_path,
            has_env.then(|| show(&env_path)),
            &[],
            &violations,
            &[],
            &[],
        );
        return Ok(Report::new(body, text).with_exit(Exit::Validation));
    }

    let interactive = stdout_tty && std::io::stdin().is_terminal();
    let (mask, ignored_no_mask) = masking(&policy, no_mask, agent, interactive);
    if ignored_no_mask {
        let _ = writeln!(
            stderr,
            "penv: --no-mask was ignored. It works only when you run penv yourself in a terminal, with no pipe and no AI agent."
        );
    }

    let drift = extras(&schema, &values);
    if !drift.is_empty() {
        let _ = writeln!(
            stderr,
            "penv: not listed in {SCHEMA_FILE}: {}. Your command still gets them, hidden in its output. Add them to {SCHEMA_FILE}, or delete them with penv unset <KEY>.",
            drift.join(", ")
        );
    }

    let secrets = if mask {
        masked_values(&schema, &values)
    } else {
        Vec::new()
    };
    let code = spawn(argv, &values, &environment, secrets)?;
    std::process::exit(code)
}

/// The values for the address the schema names, through the cache when this host
/// has one. Design section 5 decides whether the server is asked at all.
fn cloud_values(
    schema: &Schema,
    environment: &str,
    process_env: &Env,
    detection: &penv_agent::Detection,
    stderr: &mut impl Write,
) -> Result<Values, CliError> {
    let at = cloud::address(schema, environment)?;
    let opened = cloud::Cloud::open(process_env, detection)?;
    let bearer = opened.bearer(process_env, schema.org.as_deref())?;
    let cache = opened.cache(&at, &bearer);
    let resolved = penv_cloud::cache::fetch(&opened.api, &bearer, &at, cache.as_ref(), opened.now)
        .map_err(|e| cloud::refuse(e, Some(&at)))?;

    if resolved.offline_warning {
        let _ = writeln!(
            stderr,
            "penv: {at} could not be reached, so this run used the development values penv saved last time."
        );
    }
    if !resolved.body.skipped.is_empty() {
        let _ = writeln!(
            stderr,
            "penv: not passed to your command: {}. They have no stored value: never set, or generated by the cloud on demand. Set one with penv set <KEY>.",
            resolved.body.skipped.join(", ")
        );
    }

    Ok(resolved
        .body
        .keys
        .iter()
        .filter_map(|key| Some((key.name.clone(), key.value.clone()?)))
        .collect())
}

/// Local mode has one environment. `--env`, else `PENV_ENV`, else development.
fn resolve_environment(flag: Option<&str>, env: &Env) -> Result<String, CliError> {
    let flag = flag.filter(|v| !v.is_empty());
    let from_variable = flag.is_none();
    let name = flag
        .or_else(|| env.get("PENV_ENV").filter(|v| !v.is_empty()))
        .unwrap_or(DEFAULT_ENVIRONMENT)
        .to_string();
    if name == DEFAULT_ENVIRONMENT {
        return Ok(name);
    }
    let message = if from_variable {
        format!("PENV_ENV is set to {name}, which is not a local environment.")
    } else {
        format!("{name} is not a local environment.")
    };
    Err(CliError::new("environment_refused", message, format!(
        "Without the cloud, penv run only knows {DEFAULT_ENVIRONMENT}, read from {ENV_FILE}. Run penv push to link this project to the cloud and use other environments."
    ))
    .with_exit(Exit::EnvironmentRefused))
}

/// A key the file leaves empty takes the default written on its schema line.
fn apply_defaults(schema: &Schema, values: &mut Values) {
    for key in &schema.keys {
        if let Some(default) = &key.default
            && values.get(&key.name).is_none_or(String::is_empty)
        {
            values.insert(key.name.clone(), default.clone());
        }
    }
}

/// Everything the child is handed except the keys the schema marks public. A key
/// the schema never heard of is masked; `check` names it as drift.
fn masked_values(schema: &Schema, values: &Values) -> Vec<String> {
    values
        .iter()
        .filter(|(name, _)| schema.get(name).is_none_or(|key| key.sensitive))
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
        .cloned()
        .collect()
}

/// Whether to mask, and whether `--no-mask` was ignored saying so. Turning
/// masking off is a person's move: both ends must be a terminal and no agent.
fn masking(policy: &Policy, no_mask: bool, agent: bool, interactive: bool) -> (bool, bool) {
    if !no_mask {
        return (policy.mask, false);
    }
    if agent || !interactive {
        return (policy.mask, true);
    }
    (false, false)
}

/// A bare name is looked up along PATH with PATHEXT, so `npm` finds `npm.cmd`
/// on Windows; anything with a separator is used as given.
fn program(name: &str) -> PathBuf {
    if name.contains(['/', '\\']) {
        return PathBuf::from(name);
    }
    on_path(name, &[])
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Inherit the environment, override it with the resolved values, and pipe the
/// output only when there is something to scrub out of it.
fn spawn(
    argv: &[String],
    values: &Values,
    environment: &str,
    secrets: Vec<String>,
) -> Result<i32, CliError> {
    let piped = !secrets.is_empty();
    let mut command = Command::new(program(&argv[0]));
    command.args(&argv[1..]);
    command.env("PENV_ENV", environment);
    for (key, value) in values {
        command.env(key, value);
    }
    command.stdin(Stdio::inherit());
    if piped {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(sigint::default_in_child);
    }

    let mut child = command.spawn().map_err(|e| {
        CliError::new(
            "spawn_failed",
            format!("{} could not be started: {e}.", argv[0]),
            "Check the name of the command and that it is on PATH.",
        )
    })?;

    let previous_sigint = sigint::ignore_in_parent();

    let mut pumps: Vec<JoinHandle<()>> = Vec::new();
    if piped {
        if let Some(pipe) = child.stdout.take() {
            pumps.push(pump(pipe, std::io::stdout(), Masker::new(secrets.clone())));
        }
        if let Some(pipe) = child.stderr.take() {
            pumps.push(pump(pipe, std::io::stderr(), Masker::new(secrets)));
        }
    }

    let status = child.wait();
    for pump in pumps {
        let _ = pump.join();
    }

    sigint::restore(previous_sigint);

    let status = status.map_err(|e| {
        CliError::new(
            "child_failed",
            format!("penv lost track of {} while it was running: {e}.", argv[0]),
            "Run the command without penv to check that it starts.",
        )
    })?;
    Ok(exit_code(&status))
}

/// One pipe, scrubbed as it arrives so a long-running child is not buffered.
fn pump<R, W>(mut from: R, mut to: W, mut masker: Masker) -> JoinHandle<()>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        let mut scrubbed = Vec::with_capacity(chunk.len());
        loop {
            let read = match from.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            scrubbed.clear();
            masker.feed(&chunk[..read], &mut scrubbed);
            if !scrubbed.is_empty() {
                let _ = to.write_all(&scrubbed);
                let _ = to.flush();
            }
        }
        scrubbed.clear();
        masker.finish(&mut scrubbed);
        if !scrubbed.is_empty() {
            let _ = to.write_all(&scrubbed);
        }
        let _ = to.flush();
    })
}

fn exit_code(status: &ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    status.code().unwrap_or(1)
}

/// Ctrl-C belongs to the child. penv must not die first and close the pipes it
/// is still scrubbing.
#[cfg(unix)]
pub(crate) mod sigint {
    const SIGINT: i32 = 2;
    const SIG_DFL: usize = 0;
    const SIG_IGN: usize = 1;

    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }

    pub fn ignore_in_parent() -> usize {
        unsafe { signal(SIGINT, SIG_IGN) }
    }

    pub fn restore(previous: usize) {
        unsafe { signal(SIGINT, previous) };
    }

    /// Ignoring a signal survives exec, so the child is handed the default back.
    pub fn default_in_child() -> std::io::Result<()> {
        unsafe { signal(SIGINT, SIG_DFL) };
        Ok(())
    }
}

/// Windows has no sigaction. A null handler added to the console control table
/// makes this process ignore CTRL_C_EVENT; the child, created before the call,
/// keeps the default and still receives it from the console it shares.
#[cfg(windows)]
pub(crate) mod sigint {
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(handler: usize, add: i32) -> i32;
    }

    pub fn ignore_in_parent() -> usize {
        unsafe { SetConsoleCtrlHandler(0, 1) as usize }
    }

    pub fn restore(_previous: usize) {
        unsafe { SetConsoleCtrlHandler(0, 0) };
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) mod sigint {
    pub fn ignore_in_parent() -> usize {
        0
    }

    pub fn restore(_previous: usize) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use penv_agent::Detection;
    use penv_schema::{BaseType, Key, Type};

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs)
    }

    fn key(name: &str, default: Option<&str>, sensitive: bool) -> Key {
        Key {
            name: name.to_string(),
            ty: Type::new(BaseType::String),
            required: true,
            sensitive,
            default: default.map(str::to_string),
            ..Key::default()
        }
    }

    fn schema(keys: Vec<Key>) -> Schema {
        Schema {
            keys,
            ..Schema::default()
        }
    }

    #[test]
    fn the_environment_falls_back_from_the_flag_to_the_variable_to_development() {
        assert_eq!(
            resolve_environment(None, &env(&[])).unwrap(),
            DEFAULT_ENVIRONMENT
        );
        assert_eq!(
            resolve_environment(None, &env(&[("PENV_ENV", "development")])).unwrap(),
            DEFAULT_ENVIRONMENT
        );
        assert_eq!(
            resolve_environment(Some("development"), &env(&[("PENV_ENV", "staging")])).unwrap(),
            DEFAULT_ENVIRONMENT
        );
        assert_eq!(
            resolve_environment(None, &env(&[("PENV_ENV", "")])).unwrap(),
            DEFAULT_ENVIRONMENT
        );
    }

    #[test]
    fn any_other_environment_is_refused_with_exit_six() {
        for (flag, pairs) in [
            (Some("production"), &[][..]),
            (None, &[("PENV_ENV", "staging")][..]),
        ] {
            let error = resolve_environment(flag, &env(pairs)).unwrap_err();
            assert_eq!(error.exit, Exit::EnvironmentRefused);
            assert_eq!(error.code, "environment_refused");
            assert!(error.fix.contains("cloud"));
        }

        let from_variable =
            resolve_environment(None, &env(&[("PENV_ENV", "staging")])).unwrap_err();
        assert!(
            from_variable.message.contains("PENV_ENV"),
            "the message must name where staging came from: {}",
            from_variable.message
        );
        let from_flag = resolve_environment(Some("staging"), &env(&[])).unwrap_err();
        assert!(
            !from_flag.message.contains("PENV_ENV"),
            "{}",
            from_flag.message
        );
    }

    #[test]
    fn defaults_fill_the_keys_the_file_left_empty() {
        let schema = schema(vec![
            key("PORT", Some("3000"), false),
            key("NODE_ENV", Some("development"), false),
            key("STRIPE_SECRET_KEY", None, true),
        ]);
        let mut values: Values = [
            ("NODE_ENV".to_string(), "test".to_string()),
            ("STRIPE_SECRET_KEY".to_string(), String::new()),
        ]
        .into_iter()
        .collect();

        apply_defaults(&schema, &mut values);

        assert_eq!(values.get("PORT").unwrap(), "3000");
        assert_eq!(values.get("NODE_ENV").unwrap(), "test");
        assert_eq!(values.get("STRIPE_SECRET_KEY").unwrap(), "");
    }

    #[test]
    fn every_value_but_a_public_one_is_masked() {
        let schema = schema(vec![
            key("STRIPE_SECRET_KEY", None, true),
            key("EMPTY_SECRET", None, true),
            key("NEXT_PUBLIC_APP_URL", None, false),
        ]);
        let values: Values = [
            ("STRIPE_SECRET_KEY", "sk_test_FAKE0000"),
            ("EMPTY_SECRET", ""),
            ("NEXT_PUBLIC_APP_URL", "http://localhost:3000"),
            ("LEGACY_API_KEY", "left_over_FAKE"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

        let masked = masked_values(&schema, &values);
        assert!(
            masked.contains(&"left_over_FAKE".to_string()),
            "a key the schema never heard of must still be masked: {masked:?}"
        );
        assert!(masked.contains(&"sk_test_FAKE0000".to_string()));
        assert!(!masked.contains(&"http://localhost:3000".to_string()));
        assert_eq!(masked.len(), 2, "{masked:?}");
    }

    #[test]
    fn a_key_the_schema_does_not_declare_is_drift() {
        let schema = schema(vec![key("PORT", Some("3000"), false)]);
        let values: Values = [("PORT", "3000"), ("LEGACY_API_KEY", "left_over_FAKE")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(extras(&schema, &values), ["LEGACY_API_KEY"]);
    }

    #[test]
    fn a_person_may_turn_masking_off_only_at_a_terminal_on_both_ends() {
        let human = Policy::human();
        let agent = Policy::agent();
        const INTERACTIVE: bool = true;
        const PIPED: bool = false;

        assert_eq!(masking(&human, false, false, INTERACTIVE), (false, false));
        assert_eq!(masking(&human, true, false, INTERACTIVE), (false, false));
        assert_eq!(
            masking(&agent, true, true, INTERACTIVE),
            (true, true),
            "an agent never turns masking off"
        );
        assert_eq!(
            masking(&agent, true, false, PIPED),
            (true, true),
            "a pipe is not a person, so --no-mask is ignored"
        );
    }

    #[test]
    fn the_parent_can_ignore_an_interrupt_and_put_it_back() {
        let previous = sigint::ignore_in_parent();
        sigint::restore(previous);
    }

    #[test]
    fn a_tightened_session_masks_until_a_person_says_otherwise() {
        let detection = Detection {
            markers: vec![penv_agent::NON_INTERACTIVE],
            ..Detection::default()
        };
        let policy = Policy::for_(&detection, false);
        assert_eq!(masking(&policy, false, false, true), (true, false));
        assert_eq!(masking(&policy, true, false, true), (false, false));
    }
}
