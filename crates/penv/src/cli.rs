use clap::{Parser, Subcommand, ValueEnum};

/// What stdout carries. Every format that could print a raw value conflicts with
/// `--agent`; today only `text` can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Json,
    Text,
}

/// clap cannot group subcommands, so the grouping is written out. A test keeps
/// every visible command on it.
pub const HELP: &str = "{about}

{usage-heading} {usage}

Everyday
  run        Run a command with your secrets loaded into it
  ls         List your keys and show which ones have a value
  why        Say where a key's value comes from, never the value
  set        Save one value (typed hidden, never shown)
  unset      Delete one value
  reveal     Show one value; an AI agent needs your approval first
  check      Find problems in .env.schema and missing values
  scan       Find secret values committed to files
  encrypt    Encrypt the secrets in your .env files
  decrypt    Write the .env files' secrets back in plain text
  bundle     Write an encrypted file of one environment's values for a deploy

Move values
  pull       Write a .env file from the cloud
  push       Send your local .env to the cloud, then delete the file

Set up a folder
  init       Create .env.schema from your .env and keep .env out of git
  gen        Write the typed file for your language (ts, py, go, rust, php, java, csharp)
  guard      Write the rules that keep AI tools out of .env

Cloud
  login      Sign in
  logout     Sign out on this machine
  project    Your projects: ls, new, rename, rm
  env        A project's environments: ls, new, rename, copy, rm
  machine    Identities for servers and CI

This tool
  upgrade    Replace penv with the latest release
  help       Show help for a command

Options:
{options}

{after-help}
";

#[derive(Debug, Parser)]
#[command(
    name = "penv",
    version,
    about = "penv gets the right values into the right process at the right time.",
    after_help = "Penv Cloud, the secrets manager behind this CLI: https://penv.cloud",
    disable_help_subcommand = true,
    help_template = HELP
)]
pub struct Cli {
    /// Emit JSON on stdout, whatever stdout is attached to
    #[arg(long, global = true)]
    pub json: bool,

    /// Pick the output format: json or text
    #[arg(long, global = true, value_enum, value_name = "FORMAT")]
    pub format: Option<Format>,

    /// Treat this session as an agent: JSON out, values masked
    #[arg(long, global = true)]
    pub agent: bool,

    /// Read values from this provider instead of the one @penv= names
    #[arg(long, global = true, value_name = "PROVIDER")]
    pub provider: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// clap conflicts on flags, not on one value of one flag, so the pair that
    /// cannot hold is refused here with clap's own parse error.
    pub fn parse_checked() -> Cli {
        use clap::{CommandFactory, error::ErrorKind};

        let cli = Cli::parse();
        if cli.agent && cli.format == Some(Format::Text) {
            Cli::command()
                .error(
                    ErrorKind::ArgumentConflict,
                    "--agent emits JSON, so it cannot be combined with --format text.",
                )
                .exit();
        }
        cli
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Read .env, write .env.schema, and keep .env out of the repository
    Init {
        /// Overwrite an existing .env.schema
        #[arg(long)]
        force: bool,
        /// Guard exactly these harnesses instead of the installed ones
        #[arg(
            long,
            value_name = "NAMES",
            value_delimiter = ',',
            conflicts_with = "no_guards"
        )]
        guards: Option<Vec<String>>,
        /// Write no harness rules at all
        #[arg(long)]
        no_guards: bool,
        /// Write the generated typed file here, relative to the repository root
        #[arg(long, value_name = "PATH")]
        output: Option<std::path::PathBuf>,
    },

    /// Run a command with your secrets loaded into it
    Run {
        /// The environment to read
        #[arg(long)]
        env: Option<String>,
        /// Show secrets in the command's output instead of hiding them
        #[arg(long)]
        no_mask: bool,
        /// Do not load penv's masking into the command's runtime (Node, Bun, Deno, Python)
        #[arg(long)]
        no_preload: bool,
        /// Give keys with @hosts to the command as placeholders; penv puts the values into requests to those hosts. Always on for an AI agent
        #[arg(long)]
        sealed: bool,
        /// The command to run
        #[arg(last = true, num_args = 1..)]
        command: Vec<String>,
    },

    /// Move local values to the cloud and delete .env
    Push {
        /// The environment to write to
        #[arg(long)]
        env: Option<String>,
        /// The organisation that owns a project penv is about to create
        #[arg(long, value_name = "SLUG")]
        org: Option<String>,
        /// Delete cloud keys the schema no longer lists
        #[arg(long)]
        prune: bool,
    },

    /// Write a plain .env from the cloud
    Pull {
        /// The environment to read
        #[arg(long)]
        env: Option<String>,
        /// Confirm a person, not an agent, asked for the file
        #[arg(long)]
        i_am_human: bool,
    },

    /// Sign in through your browser; the login is kept in your system's password store
    Login,

    /// Sign out on this machine
    Logout,

    /// Save one value (typed hidden, never shown)
    Set {
        /// The key to write
        key: String,
        /// The environment to write to
        #[arg(long)]
        env: Option<String>,
        /// Refused: a value passed here lands in the shell history
        #[arg(long, value_name = "VALUE")]
        value: Option<String>,
    },

    /// Delete one value
    Unset {
        /// The key to remove
        key: String,
        /// The environment to write to
        #[arg(long)]
        env: Option<String>,
    },

    /// List your keys and show which ones have a value
    Ls {
        /// The environment to read
        #[arg(long)]
        env: Option<String>,
    },

    /// Write one environment's values, encrypted, to .penv/<env>.bundle for a deploy
    Bundle {
        /// The environment to bundle
        #[arg(long)]
        env: Option<String>,
    },

    /// Encrypt the sensitive values in the .env files beside .env.schema
    Encrypt,

    /// Write the encrypted values in the .env files back in plain text
    Decrypt,

    /// Say where a key's value comes from and how penv treats it, never the value
    Why {
        /// The key
        key: String,
        /// The environment to read
        #[arg(long)]
        env: Option<String>,
    },

    /// Report schema problems and missing values
    Check {
        /// Check one key instead of all of them
        key: Option<String>,
        /// The environment to check
        #[arg(long)]
        env: Option<String>,
        /// Fail when code reads a variable .env.schema does not declare
        #[arg(long)]
        strict: bool,
    },

    /// Write the typed file for your language (ts, py, go, rust, php, java, csharp)
    Gen {
        /// The target name: ts, py, go, rust, php, java or csharp; omit it to list them
        target: Option<String>,
        /// Write here instead, relative to the repository root
        #[arg(long, value_name = "PATH")]
        out: Option<std::path::PathBuf>,
        /// Compare with what is on disk instead of writing
        #[arg(long)]
        check: bool,
        /// Show what this target's options change instead of writing
        #[arg(long, requires = "target", conflicts_with_all = ["out", "check"])]
        options: bool,
    },

    /// Find secret values committed to files
    Scan {
        /// Files or folders to scan instead of what git would commit
        paths: Vec<std::path::PathBuf>,
        /// Scan only what is staged for the next commit
        #[arg(long)]
        staged: bool,
        /// Write a git pre-commit hook that runs penv scan --staged
        #[arg(long)]
        install_hook: bool,
        /// The environment whose values to look for
        #[arg(long)]
        env: Option<String>,
    },

    /// Write the harness rules that keep agents out of .env
    Guard {
        /// The harnesses to write, instead of the installed ones
        harness: Vec<String>,
        /// Write every harness penv knows, installed or not
        #[arg(long)]
        all: bool,
        /// Report coverage instead of writing
        #[arg(long)]
        check: bool,
    },

    /// Show one value; an AI agent needs your approval first
    Reveal {
        /// The key to reveal
        key: String,
        /// The environment to read
        #[arg(long)]
        env: Option<String>,
        /// Print the value a person approved under this id
        #[arg(long, value_name = "ID")]
        approval: Option<String>,
    },

    /// Your projects: ls, new, rename, rm
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },

    /// A project's environments: ls, new, rename, copy, rm
    Env {
        /// The project, as name or org/name; defaults to the one this folder is linked to
        #[arg(long, short = 'p', global = true, value_name = "PROJECT")]
        project: Option<String>,
        #[command(subcommand)]
        command: EnvCommand,
    },

    /// Identities for servers and CI
    Machine {
        #[command(subcommand)]
        command: MachineCommand,
    },

    /// Replace penv with the latest release
    Upgrade {
        /// latest (the default), next for prereleases too, or a version such as 1.2.0
        channel: Option<String>,
        /// Only report what that release is
        #[arg(long)]
        check: bool,
    },

    /// Print the shell completion script
    #[command(hide = true)]
    Completions {
        /// bash, zsh, fish, powershell or elvish
        shell: String,
    },

    /// Run as a harness hook; a payload it cannot read is refused
    #[command(hide = true)]
    Hook {
        /// The harness, such as claude-code
        harness: String,
    },

    /// Print the schema as JSON
    #[command(hide = true)]
    Schema,

    /// Serve .env.schema to an editor over the Language Server Protocol on stdio
    #[command(hide = true)]
    Lsp,

    /// Show help for a command
    Help {
        /// Print help for one command instead
        command: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// List your projects and their environments
    Ls {
        /// Only this organisation
        #[arg(long, value_name = "SLUG")]
        org: Option<String>,
    },
    /// Create a project with a development environment
    New {
        /// The project name
        name: String,
        /// The organisation that owns it
        #[arg(long, value_name = "SLUG")]
        org: Option<String>,
    },
    /// Rename a project; a folder linked to it is updated too
    Rename {
        /// The current name
        old: String,
        /// The new name
        new: String,
        #[arg(long, value_name = "SLUG")]
        org: Option<String>,
    },
    /// Delete a project and every value in it, for good
    Rm {
        /// The project name
        name: String,
        #[arg(long, value_name = "SLUG")]
        org: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum EnvCommand {
    /// List the project's environments
    Ls,
    /// Create an empty environment
    New {
        /// The environment name
        name: String,
    },
    /// Rename an environment
    Rename {
        /// The current name
        old: String,
        /// The new name
        new: String,
    },
    /// Create an environment with another one's keys; values are never copied
    Copy {
        /// The environment to copy from
        from: String,
        /// The environment to create
        to: String,
    },
    /// Delete an environment and every value in it, for good
    Rm {
        /// The environment name
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum MachineCommand {
    /// Give this server its own identity, from a one-time secret made in the console
    Enroll {
        /// The one-time enrolment secret
        secret: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn every_visible_command_is_on_the_grouped_help() {
        for command in Cli::command()
            .get_subcommands()
            .filter(|c| !c.is_hide_set())
        {
            let listed = HELP
                .lines()
                .any(|line| line.trim_start().split(' ').next() == Some(command.get_name()));
            assert!(listed, "{} is missing from HELP", command.get_name());
        }
    }
}
