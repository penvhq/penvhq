use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread::JoinHandle;

use penv_mask::Masker;
use penv_schema::{Schema, Values, extras, validate};

use crate::source;

use crate::agent::detect_here;
use crate::commands::cloud;
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::{ENV_FILE, SCHEMA_FILE, find_schema, on_path, show};
use crate::output::{Output, Report};
use crate::ui;

/// Validate, then hand the values to one child process and nothing else. The
/// child's output is this command's output, so penv writes only to stderr and
/// leaves with the child's exit code.
#[allow(clippy::too_many_arguments)]
pub fn run(
    out: &Output,
    cwd: &Path,
    environment: Option<&str>,
    no_mask: bool,
    no_preload: bool,
    sealed: bool,
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
    // The design has init fire when run meets a .env with no schema for it.
    // Any value file counts, the way every other loader reads them.
    let found = crate::files::value_files(cwd);
    if find_schema(cwd).is_none() && !found.is_empty() {
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
        let read = found.iter().map(|p| show(p)).collect::<Vec<_>>().join(", ");
        ui::note(&format!(
            "there was no {SCHEMA_FILE}, so penv init wrote one from {read}."
        ));
        let _ = out.write(&inferred, &mut std::io::stderr());
    }

    let (schema_path, schema) = super::load_schema(cwd)?;
    let dir = schema_path.parent().unwrap_or(cwd);
    let env_path = dir.join(ENV_FILE);
    let has_env = env_path.is_file();

    let stdout_tty = std::io::stdout().is_terminal();
    let detection = detect_here(process_env, stdout_tty);
    let agent = detection.is_agent() || agent_flag;

    let environment = source::environment(environment, process_env, &schema, dir);
    let mut fetcher = cloud::Fetcher::new(process_env, &detection);
    let resolved = source::values_with(
        &schema,
        dir,
        &environment,
        process_env,
        &mut fetcher,
        source::Generate::Yes,
    )?;
    source::report(&resolved, dir);
    for (file, how, keys) in source::exposed_secrets(&schema, &resolved) {
        ui::warn(&source::exposure_message(&file, how, &keys));
    }
    if source::failed(&resolved.errors) {
        return Err(source::unresolved(&resolved.errors));
    }
    let tainted = resolved.tainted.clone();
    let failed_asserts = resolved.failed_asserts.clone();
    let values = resolved.values.clone();

    let mut violations = validate(&schema.for_environment(&environment), &values);
    violations.extend(source::public_leaks(&schema, &tainted));
    violations.extend(failed_asserts.iter().map(|(line, message)| {
        penv_schema::Violation::new(&format!("@assert line {line}"), "assert", message.clone())
    }));
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
    let (mask, ignored_no_mask) = masking(no_mask, agent, interactive);
    if ignored_no_mask {
        ui::warn(
            "--no-mask was ignored. It works only when you run penv yourself in a terminal, with no pipe and no AI agent.",
        );
    }

    if mask {
        let short = source::too_short_to_mask(&schema, &tainted, &values);
        if !short.is_empty() {
            ui::warn(&format!(
                "{} {} sensitive and shorter than {} characters, so it cannot be masked; mark it @sensitive=false or use a longer value.",
                short.join(", "),
                if short.len() == 1 { "is" } else { "are" },
                penv_mask::MIN_SECRET_LEN
            ));
        }
    }

    let drift = extras(&schema, &values);
    if !drift.is_empty() {
        ui::warn(&format!(
            "not listed in {SCHEMA_FILE}: {}. Your command still gets them, hidden in its output. Add them to {SCHEMA_FILE}, or delete them with penv unset <KEY>.",
            drift.join(", ")
        ));
    }

    let secrets = if mask {
        masked_values(&schema, &values, &tainted)
    } else {
        Vec::new()
    };
    let named: Vec<(String, String)> = values
        .iter()
        .filter(|(name, value)| !value.is_empty() && source::is_sensitive(&schema, &tainted, name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    // Coarse filesystem clocks: a build that finishes within the same tick
    // still counts as after the start.
    let started = std::time::SystemTime::now() - std::time::Duration::from_secs(2);
    // The preload follows masking. Turning it off is a person's move, like
    // --no-mask: an agent could otherwise start a server that echoes the
    // environment and read the secret back over HTTP.
    let wants_preload = !(no_preload && !agent && interactive)
        && (agent
            || crate::config::Config::load(dir)
                .map(|c| c.preload())
                .unwrap_or(true));
    if no_preload && (agent || !interactive) {
        ui::warn(
            "--no-preload was ignored. It works only when you run penv yourself in a terminal, with no pipe and no AI agent.",
        );
    }
    let injection = match (mask && wants_preload, crate::preload::files()) {
        (true, Some(files)) => {
            let names: Vec<String> = named.iter().map(|(n, _)| n.clone()).collect();
            crate::preload::inject(
                &files,
                &names,
                argv,
                |name| {
                    values
                        .get(name)
                        .cloned()
                        .or_else(|| std::env::var(name).ok())
                },
                |program| crate::preload::deno_has_preload(program, &files),
            )
        }
        _ => crate::preload::Injection::default(),
    };
    // Sealed: keys with @hosts reach the child as placeholders, and the proxy in
    // this process puts the values into requests to those hosts. An agent is
    // always sealed; a person asks for it with --sealed.
    let mut child_values = values.clone();
    let mut injection = injection;
    if (agent || sealed)
        && let Some(seal) = crate::sealed::run::prepare(&schema, &values, process_env, agent)?
    {
        child_values = source::sealed_values(&resolved, process_env, &seal.placeholders);
        injection.env.extend(seal.env);
    }
    let code = spawn(argv, &child_values, &environment, secrets, &injection)?;
    std::process::exit(after_build(dir, started, code, &named))
}

/// After a successful run, look at what it wrote into browser output folders
/// (`.next/static`, `dist`, `build`, ...). A secret there ships to every visitor,
/// so the run fails with exit 3, naming file, line and key. Never the value.
fn after_build(
    dir: &Path,
    started: std::time::SystemTime,
    code: i32,
    secrets: &[(String, String)],
) -> i32 {
    if code != 0 || secrets.is_empty() {
        return code;
    }
    let dirs = super::scan::client_output(dir);
    let mut files = super::scan::changed_since(&dirs, started);
    files.extend(super::scan::native_bundles_since(dir, started));
    if files.is_empty() {
        return code;
    }
    let found = super::scan::find(&files, secrets);
    if found.is_empty() {
        return code;
    }
    for (file, line, key) in &found {
        let at = if *line == 0 {
            show(file)
        } else {
            format!("{}:{line}", show(file))
        };
        ui::warn(&format!(
            "{at} holds the value of {key}, and that file ships to the browser or the app."
        ));
    }
    ui::warn(
        "Remove the value from client code, rotate it if this build was deployed, and read it on the server.",
    );
    Exit::Validation as i32
}

/// Everything the child is handed except the keys the schema marks public. A key
/// the schema never heard of is masked; `check` names it as drift. A public key
/// computed from a sensitive one is masked too.
fn masked_values(
    schema: &Schema,
    values: &Values,
    tainted: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    values
        .iter()
        .filter(|(name, _)| source::is_sensitive(schema, tainted, name))
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
        .cloned()
        .collect()
}

/// Whether to mask, and whether `--no-mask` was ignored saying so. Masking is on
/// for every run, CI and a person's terminal included, because a log is copied
/// further than anyone expects. Turning it off is a person's move: both ends
/// must be a terminal and no agent.
fn masking(no_mask: bool, agent: bool, interactive: bool) -> (bool, bool) {
    if !no_mask {
        return (true, false);
    }
    if agent || !interactive {
        return (true, true);
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
    injection: &crate::preload::Injection,
) -> Result<i32, CliError> {
    let piped = !secrets.is_empty();
    let mut command = Command::new(program(&argv[0]));
    if injection.deno_args.is_empty() || argv.len() < 2 {
        command.args(&argv[1..]);
    } else {
        command
            .arg(&argv[1])
            .args(&injection.deno_args)
            .args(&argv[2..]);
    }
    command.env("PENV_ENV", environment);
    // The key that decrypts value files stays with penv.
    command.env_remove(crate::localcrypt::KEY_VAR);
    command.env_remove(crate::bundle::KEY_VAR);
    for (key, value) in values {
        command.env(key, value);
    }
    for (key, value) in &injection.env {
        command.env(key, value);
    }
    command.stdin(Stdio::inherit());
    if piped {
        // A pipe makes the child think it is not on a terminal and drop its
        // colours. When penv itself is on one, say so the way Node, Python and
        // most CLIs read it, unless the environment already chose.
        if std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && !values.contains_key("NO_COLOR")
        {
            for (name, value) in [("FORCE_COLOR", "1"), ("CLICOLOR_FORCE", "1")] {
                if std::env::var_os(name).is_none() && !values.contains_key(name) {
                    command.env(name, value);
                }
            }
        }
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
                Ok(0) => break,
                Ok(n) => n,
                // Stopping here would leave the child blocked on a full pipe.
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
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
    fn defaults_fill_the_keys_the_file_left_empty() {
        let schema = schema(vec![
            key("PORT", Some("3000"), false),
            key("NODE_ENV", Some("development"), false),
            key("STRIPE_SECRET_KEY", None, true),
        ]);
        let raw = [
            (
                "NODE_ENV".to_string(),
                penv_schema::resolve::Raw::literal("test"),
            ),
            (
                "STRIPE_SECRET_KEY".to_string(),
                penv_schema::resolve::Raw::literal(""),
            ),
        ]
        .into_iter()
        .collect();

        let (values, errors) = source::finish(&schema, raw, &env(&[]), source::DEFAULT_ENVIRONMENT);
        assert!(errors.is_empty());

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

        let masked = masked_values(&schema, &values, &Default::default());
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
        const INTERACTIVE: bool = true;
        const PIPED: bool = false;

        assert_eq!(
            masking(false, false, INTERACTIVE),
            (true, false),
            "masking is on by default, for a person too"
        );
        assert_eq!(
            masking(false, false, PIPED),
            (true, false),
            "CI and pipes mask"
        );
        assert_eq!(masking(true, false, INTERACTIVE), (false, false));
        assert_eq!(
            masking(true, true, INTERACTIVE),
            (true, true),
            "an agent never turns masking off"
        );
        assert_eq!(
            masking(true, false, PIPED),
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
        let policy = penv_agent::Policy::for_(&detection, false);
        assert!(policy.mask);
        assert_eq!(masking(false, false, true), (true, false));
        assert_eq!(masking(true, false, true), (false, false));
    }
}
