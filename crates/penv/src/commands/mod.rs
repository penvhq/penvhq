mod bundle;
mod check;
pub mod cloud;
mod crypt;
mod environment;
mod r#gen;
pub mod guard;
pub mod hook;
pub mod init;
mod login;
mod logout;
mod ls;
mod machine;
mod project;
mod pull;
mod push;
mod reveal;
mod run;
mod scan;
mod set;
mod state;
mod upgrade;
mod why;

use std::path::Path;

use crate::cli::{Cli, Command, MachineCommand};
use crate::env::Env;
use crate::error::CliError;
use crate::manifest::manifest;
use crate::output::{Output, Report};
use penv_schema::Schema;

pub fn dispatch(cli: &Cli, out: &Output, cwd: &Path, env: &Env) -> Result<Report, CliError> {
    if cli.agent {
        crate::agent::flag_session();
    }
    crate::providers::remember(cli.provider.as_deref(), cwd);
    match &cli.command {
        None => state::run(out, cwd, env),
        Some(Command::Init {
            force,
            guards,
            no_guards,
            output,
        }) => {
            let choice = match (guards.as_deref().map(init::named), no_guards) {
                (_, true) => init::Guards::None,
                (Some(named), _) if named.is_empty() => init::Guards::None,
                (Some(named), _) => init::Guards::Named(named),
                _ => init::Guards::Ask,
            };
            init::run(out, cwd, *force, &choice, output.as_deref(), env, cli.agent)
        }
        Some(Command::Run {
            env: environment,
            no_mask,
            no_preload,
            sealed,
            command,
        }) => run::run(
            out,
            cwd,
            environment.as_deref(),
            *no_mask,
            *no_preload,
            *sealed,
            command,
            env,
            cli.agent,
        ),
        Some(Command::Push {
            env: environment,
            org,
            prune,
        }) => push::run(
            out,
            cwd,
            environment.as_deref(),
            org.as_deref(),
            *prune,
            env,
        ),
        Some(Command::Pull {
            env: environment,
            i_am_human,
        }) => pull::run(
            out,
            cwd,
            environment.as_deref(),
            *i_am_human,
            env,
            cli.agent,
        ),
        Some(Command::Login) => login::run(out, cwd, env, cli.agent),
        Some(Command::Logout) => logout::run(out, cwd, env),
        Some(Command::Set {
            key,
            env: environment,
            value,
        }) => set::set(out, cwd, key, environment.as_deref(), value.as_deref(), env),
        Some(Command::Unset {
            key,
            env: environment,
        }) => set::unset(out, cwd, key, environment.as_deref(), env),
        Some(Command::Reveal {
            key,
            env: environment,
            approval,
        }) => reveal::run(
            out,
            cwd,
            key,
            environment.as_deref(),
            env,
            cli.agent,
            approval.as_deref(),
        ),
        Some(Command::Machine {
            command: MachineCommand::Enroll { secret },
        }) => machine::enroll(out, cwd, secret, env),
        Some(Command::Check {
            key,
            env: environment,
            strict,
        }) => check::run(
            out,
            cwd,
            key.as_deref(),
            environment.as_deref(),
            *strict,
            env,
        ),
        Some(Command::Ls { env: name }) => ls::run(out, cwd, name.as_deref(), env),
        Some(Command::Why { key, env: name }) => why::run(out, cwd, key, name.as_deref(), env),
        Some(Command::Encrypt) => crypt::run(out, cwd, true, env, cli.agent),
        Some(Command::Bundle { env: name }) => {
            bundle::run(out, cwd, name.as_deref(), env, cli.agent)
        }
        Some(Command::Decrypt) => crypt::run(out, cwd, false, env, cli.agent),
        Some(Command::Project { command }) => project::run(out, cwd, command, env, cli.agent),
        Some(Command::Env { project, command }) => {
            environment::run(out, cwd, project.as_deref(), command, env, cli.agent)
        }
        Some(Command::Gen {
            target,
            out: to,
            check,
            options,
        }) => r#gen::run(
            out,
            cwd,
            target.as_deref(),
            to.as_deref(),
            match (*options, *check) {
                (true, _) => r#gen::Mode::Options,
                (_, true) => r#gen::Mode::Check,
                _ => r#gen::Mode::Write,
            },
            env,
            cli.agent,
        ),
        Some(Command::Guard {
            harness,
            all,
            check,
        }) => guard::run(out, cwd, *check, *all, harness),
        Some(Command::Scan {
            paths,
            staged,
            install_hook,
            env: environment,
        }) => scan::run(
            out,
            cwd,
            paths,
            *staged,
            *install_hook,
            environment.as_deref(),
            env,
        ),
        Some(Command::Hook { harness }) => hook::run(harness),
        Some(Command::Upgrade { channel, check }) => upgrade::run(out, channel.as_deref(), *check),
        Some(Command::Completions { shell }) => completions(
            shell,
            cli.json || cli.format == Some(crate::cli::Format::Json),
        ),
        Some(Command::Schema) => schema(cwd),
        Some(Command::Help { command }) => help(command.as_deref()),
    }
}

/// A person is watching and can answer: both ends are a terminal, the report is
/// text, and nothing says an agent is driving.
pub fn interactive(out: &Output, env: &Env, agent_flag: bool) -> bool {
    use std::io::IsTerminal;

    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    tty && !out.is_json() && !agent_flag && !crate::agent::detect_here(env, tty).is_agent()
}

/// Load the nearest schema with its imports, reporting its diagnostics as one
/// validation failure.
fn load_schema(cwd: &Path) -> Result<(std::path::PathBuf, Schema), CliError> {
    crate::source::load(cwd)
}

/// A shell reads the script off stdout, and the install line redirects it into a
/// file, so nothing but an explicit --json turns it into an object.
fn completions(shell: &str, json: bool) -> Result<Report, CliError> {
    use std::io::Write;

    let script = crate::completions::script(shell, &manifest())?;
    if json {
        return Ok(Report::new(
            serde_json::json!({ "shell": shell, "script": script }),
            script,
        ));
    }
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(script.as_bytes());
    let _ = stdout.flush();
    Ok(Report::silent())
}

fn schema(cwd: &Path) -> Result<Report, CliError> {
    let (_, schema) = load_schema(cwd)?;
    let json = schema.to_json();
    let text = serde_json::to_string_pretty(&json).unwrap_or_default();
    Ok(Report::new(json, text))
}

/// `help --json` is the manifest; on a terminal it is clap's own help text.
fn help(command: Option<&str>) -> Result<Report, CliError> {
    use clap::CommandFactory;

    let mut root = Cli::command();
    let text = match command {
        None => root.render_long_help().to_string(),
        Some(name) => root
            .get_subcommands()
            .find(|c| c.get_name() == name)
            .cloned()
            .ok_or_else(|| {
                CliError::new(
                    "unknown_command",
                    format!("penv has no {name} command."),
                    "Run penv help to see the tree.",
                )
            })?
            .render_long_help()
            .to_string(),
    };
    Ok(Report::new(manifest(), text))
}
