use clap::{Arg, ArgAction, Command, CommandFactory};
use serde_json::{Value, json};

use crate::cli::Cli;
use crate::error::EXIT_CODES;

/// The manifest grammar version. Consumers pin it.
pub const MANIFEST_VERSION: u32 = 1;

/// What clap cannot know about a command. Everything else comes from the tree.
struct Meta {
    path: &'static str,
    implemented: bool,
    reveals_values: bool,
    requires_approval: bool,
    exit_codes: &'static [i32],
    /// Flags an agent policy strips, because only a person may pass them.
    human_flags: &'static [&'static str],
    /// Flags that also read an environment variable.
    env_flags: &'static [(&'static str, &'static str)],
    /// Word lists for positionals, where the list lives outside clap.
    arg_values: &'static [(&'static str, &'static [&'static str])],
    /// Positionals the system answers for: `command` is an executable and its
    /// arguments. Completions hand those to the shell instead of a word list.
    arg_completes: &'static [(&'static str, &'static str)],
}

const DEFAULT_META: Meta = Meta {
    path: "",
    implemented: false,
    reveals_values: false,
    requires_approval: false,
    exit_codes: &[0, 1],
    human_flags: &[],
    env_flags: &[],
    arg_values: &[],
    arg_completes: &[],
};

const META: &[Meta] = &[
    Meta {
        path: "",
        implemented: true,
        ..DEFAULT_META
    },
    Meta {
        path: "init",
        implemented: true,
        exit_codes: &[0, 1, 3],
        ..DEFAULT_META
    },
    Meta {
        path: "run",
        implemented: true,
        exit_codes: &[0, 1, 3, 5, 6],
        human_flags: &["no-mask"],
        env_flags: &[("env", "PENV_ENV")],
        arg_completes: &[("command", "command")],
        ..DEFAULT_META
    },
    Meta {
        path: "push",
        implemented: true,
        exit_codes: &[0, 1, 2, 3, 5, 6],
        human_flags: &["prune"],
        env_flags: &[("env", "PENV_ENV")],
        ..DEFAULT_META
    },
    Meta {
        path: "pull",
        implemented: true,
        reveals_values: true,
        exit_codes: &[0, 1, 2, 5, 6],
        human_flags: &["i-am-human"],
        env_flags: &[("env", "PENV_ENV")],
        ..DEFAULT_META
    },
    Meta {
        path: "login",
        implemented: true,
        exit_codes: &[0, 1, 2],
        ..DEFAULT_META
    },
    Meta {
        path: "logout",
        implemented: true,
        exit_codes: &[0, 1, 2],
        ..DEFAULT_META
    },
    Meta {
        path: "set",
        implemented: true,
        exit_codes: &[0, 1, 2, 3, 5, 6],
        human_flags: &["value"],
        env_flags: &[("env", "PENV_ENV")],
        ..DEFAULT_META
    },
    Meta {
        path: "unset",
        implemented: true,
        exit_codes: &[0, 1, 2, 5, 6],
        env_flags: &[("env", "PENV_ENV")],
        ..DEFAULT_META
    },
    Meta {
        path: "ls",
        implemented: true,
        exit_codes: &[0, 1, 3, 5, 6],
        env_flags: &[("env", "PENV_ENV")],
        ..DEFAULT_META
    },
    Meta {
        path: "check",
        implemented: true,
        exit_codes: &[0, 1, 3],
        env_flags: &[("env", "PENV_ENV")],
        ..DEFAULT_META
    },
    Meta {
        path: "gen",
        implemented: true,
        exit_codes: &[0, 1, 3],
        ..DEFAULT_META
    },
    Meta {
        path: "scan",
        implemented: true,
        exit_codes: &[0, 1, 3, 5],
        env_flags: &[("env", "PENV_ENV")],
        ..DEFAULT_META
    },
    Meta {
        path: "guard",
        implemented: true,
        exit_codes: &[0, 1, 3],
        ..DEFAULT_META
    },
    Meta {
        path: "reveal",
        implemented: true,
        reveals_values: true,
        requires_approval: true,
        exit_codes: &[0, 1, 2, 4, 5, 6],
        env_flags: &[("env", "PENV_ENV")],
        ..DEFAULT_META
    },
    Meta {
        path: "project",
        implemented: true,
        exit_codes: &[0, 1, 2, 5],
        ..DEFAULT_META
    },
    Meta {
        path: "project ls",
        implemented: true,
        exit_codes: &[0, 1, 2, 5],
        ..DEFAULT_META
    },
    Meta {
        path: "project new",
        implemented: true,
        exit_codes: &[0, 1, 2, 3, 5],
        ..DEFAULT_META
    },
    Meta {
        path: "project rename",
        implemented: true,
        exit_codes: &[0, 1, 2, 3, 5],
        ..DEFAULT_META
    },
    Meta {
        path: "project rm",
        implemented: true,
        requires_approval: true,
        exit_codes: &[0, 1, 2, 4, 5],
        ..DEFAULT_META
    },
    Meta {
        path: "env",
        implemented: true,
        exit_codes: &[0, 1, 2, 5],
        ..DEFAULT_META
    },
    Meta {
        path: "env ls",
        implemented: true,
        exit_codes: &[0, 1, 2, 5],
        ..DEFAULT_META
    },
    Meta {
        path: "env new",
        implemented: true,
        exit_codes: &[0, 1, 2, 3, 5],
        ..DEFAULT_META
    },
    Meta {
        path: "env rename",
        implemented: true,
        exit_codes: &[0, 1, 2, 3, 5],
        ..DEFAULT_META
    },
    Meta {
        path: "env copy",
        implemented: true,
        exit_codes: &[0, 1, 2, 3, 5],
        ..DEFAULT_META
    },
    Meta {
        path: "env rm",
        implemented: true,
        requires_approval: true,
        exit_codes: &[0, 1, 2, 4, 5],
        ..DEFAULT_META
    },
    Meta {
        path: "machine",
        implemented: true,
        exit_codes: &[0, 1, 2],
        ..DEFAULT_META
    },
    Meta {
        path: "machine enroll",
        implemented: true,
        exit_codes: &[0, 1, 2],
        ..DEFAULT_META
    },
    Meta {
        path: "upgrade",
        implemented: true,
        ..DEFAULT_META
    },
    Meta {
        path: "completions",
        implemented: true,
        arg_values: &[("shell", &crate::completions::SHELLS)],
        ..DEFAULT_META
    },
    Meta {
        path: "hook",
        implemented: true,
        exit_codes: &[0, 1, 2],
        ..DEFAULT_META
    },
    Meta {
        path: "schema",
        implemented: true,
        exit_codes: &[0, 1, 3],
        ..DEFAULT_META
    },
    Meta {
        path: "help",
        implemented: true,
        ..DEFAULT_META
    },
];

fn meta(path: &str) -> &'static Meta {
    META.iter()
        .find(|m| m.path == path)
        .unwrap_or(&DEFAULT_META)
}

/// The command tree as data: clap for shape, the table above for policy.
pub fn manifest() -> Value {
    let mut root = Cli::command();
    root.build();
    let globals: Vec<Value> = root
        .get_arguments()
        .filter(|a| a.is_global_set() && !is_builtin(a))
        .map(|a| flag(a, meta("")))
        .collect();

    json!({
        "schemaVersion": MANIFEST_VERSION,
        "penvVersion": env!("CARGO_PKG_VERSION"),
        "about": root.get_about().map(|a| a.to_string()),
        "globalFlags": globals,
        "exitCodes": EXIT_CODES
            .iter()
            .map(|(code, name, meaning)| json!({ "code": code, "name": name, "meaning": meaning }))
            .collect::<Vec<_>>(),
        "commands": root.get_subcommands().map(|c| describe(c, "")).collect::<Vec<_>>(),
    })
}

fn describe(cmd: &Command, prefix: &str) -> Value {
    let name = cmd.get_name().to_string();
    let path = if prefix.is_empty() {
        name.clone()
    } else {
        format!("{prefix} {name}")
    };
    let m = meta(&path);

    let args: Vec<Value> = cmd
        .get_arguments()
        .filter(|a| a.is_positional() && !is_builtin(a))
        .map(|a| positional(a, m))
        .collect();
    let flags: Vec<Value> = cmd
        .get_arguments()
        .filter(|a| !a.is_positional() && !a.is_global_set() && !is_builtin(a))
        .map(|a| flag(a, m))
        .collect();

    json!({
        "name": name,
        "path": path,
        "about": cmd.get_about().map(|a| a.to_string()),
        "implemented": m.implemented,
        "revealsValues": m.reveals_values,
        "requiresApproval": m.requires_approval,
        "exitCodes": m.exit_codes,
        "args": args,
        "flags": flags,
        "subcommands": cmd
            .get_subcommands()
            .map(|c| describe(c, &path))
            .collect::<Vec<_>>(),
    })
}

fn is_builtin(arg: &Arg) -> bool {
    matches!(arg.get_id().as_str(), "help" | "version")
}

fn positional(arg: &Arg, m: &Meta) -> Value {
    let name = arg.get_id().as_str();
    let mut values = possible(arg);
    if values.is_empty()
        && let Some((_, listed)) = m.arg_values.iter().find(|(arg, _)| *arg == name)
    {
        values = listed.iter().map(|v| v.to_string()).collect();
    }
    json!({
        "name": name,
        "about": arg.get_help().map(|h| h.to_string()),
        "required": arg.is_required_set(),
        "variadic": arg.get_num_args().is_some_and(|r| r.max_values() > 1),
        "values": values,
        "completes": m
            .arg_completes
            .iter()
            .find(|(arg, _)| *arg == name)
            .map(|(_, kind)| *kind),
    })
}

/// The words clap accepts for one argument. Completions hint with them.
fn possible(arg: &Arg) -> Vec<String> {
    arg.get_possible_values()
        .iter()
        .filter(|v| !v.is_hide_set())
        .map(|v| v.get_name().to_string())
        .collect()
}

fn flag(arg: &Arg, m: &Meta) -> Value {
    let name = arg.get_id().as_str().replace('_', "-");
    let takes_value = matches!(arg.get_action(), ArgAction::Set | ArgAction::Append);
    json!({
        "name": name,
        "long": arg.get_long().map(|l| format!("--{l}")),
        "short": arg.get_short().map(|s| format!("-{s}")),
        "about": arg.get_help().map(|h| h.to_string()),
        "takesValue": takes_value,
        "default": takes_value
            .then(|| arg.get_default_values().first())
            .flatten()
            .map(|v| v.to_string_lossy().into_owned()),
        "env": m
            .env_flags
            .iter()
            .find(|(flag, _)| *flag == name)
            .map(|(_, var)| *var),
        "human": m.human_flags.contains(&name.as_str()),
        "values": possible(arg),
    })
}

/// Every command in the tree, by path. The manifest test uses it to prove the
/// metadata table covers the tree.
pub fn command_paths() -> Vec<String> {
    let mut root = Cli::command();
    root.build();
    let mut out = vec![String::new()];
    collect(&root, "", &mut out);
    out
}

fn collect(cmd: &Command, prefix: &str, out: &mut Vec<String>) {
    for sub in cmd.get_subcommands() {
        let path = if prefix.is_empty() {
            sub.get_name().to_string()
        } else {
            format!("{prefix} {}", sub.get_name())
        };
        collect(sub, &path, out);
        out.push(path);
    }
}

/// True when the table has an entry of its own for this path.
pub fn has_meta(path: &str) -> bool {
    META.iter().any(|m| m.path == path)
}
