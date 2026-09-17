use clap::{Parser, Subcommand, ValueEnum};

/// What stdout carries. Every format that could print a raw value conflicts with
/// `--agent`; today only `text` can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Json,
    Text,
}

#[derive(Debug, Parser)]
#[command(
    name = "penv",
    version,
    about = "penv gets the right values into the right process at the right time.",
    after_help = "Penv Cloud, the secrets manager behind this CLI: https://penv.cloud",
    disable_help_subcommand = true
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

    /// Validate, then run a command with the values in its environment only
    Run {
        /// The environment to read
        #[arg(long)]
        env: Option<String>,
        /// Leave the child's output unmasked
        #[arg(long)]
        no_mask: bool,
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

    /// Sign in with a device code; the credential goes to the OS keychain
    Login,

    /// Remove the stored credential
    Logout,

    /// Write one value without echoing it
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

    /// Remove one value
    Unset {
        /// The key to remove
        key: String,
        /// The environment to write to
        #[arg(long)]
        env: Option<String>,
    },

    /// List keys, types and which ones have a value
    Ls,

    /// Report schema problems and missing values
    Check {
        /// Check one key instead of all of them
        key: Option<String>,
    },

    /// Write the typed file for a language target
    Gen {
        /// The target name, such as ts or py; omit it to list the targets
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

    /// Print one value; an agent session needs a person's approval
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

    /// Machine identities
    Machine {
        #[command(subcommand)]
        command: MachineCommand,
    },

    /// Replace this binary from the latest release
    Upgrade {
        /// Report what the release carries instead of replacing anything
        #[arg(long)]
        check: bool,
    },

    /// Print the shell completion script
    Completions {
        /// bash, zsh, fish, powershell or elvish
        shell: String,
    },

    /// Run as a harness hook; a payload it cannot read is refused
    Hook {
        /// The harness, such as claude-code
        harness: String,
    },

    /// Print the schema as JSON
    Schema,

    /// Print the command manifest
    Help {
        /// Print help for one command instead
        command: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum MachineCommand {
    /// Bind a server keypair from a one-time secret
    Enroll {
        /// The one-time enrolment secret
        secret: String,
    },
}
