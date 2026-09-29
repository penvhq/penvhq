//! The environment names `penv run` keeps for the command itself, and the
//! inherited variables it never passes on. The lists are `reserved.toml`.

use std::sync::OnceLock;

use crate::error::{CliError, Exit};

struct Lists {
    refused_names: Vec<String>,
    refused_prefixes: Vec<String>,
    scrubbed_names: Vec<String>,
    scrubbed_prefixes: Vec<String>,
}

fn lists() -> &'static Lists {
    static LISTS: OnceLock<Lists> = OnceLock::new();
    LISTS.get_or_init(|| {
        let table: toml::Table =
            toml::from_str(include_str!("reserved.toml")).expect("reserved.toml is TOML");
        let words = |section: &str, field: &str| -> Vec<String> {
            table[section][field]
                .as_array()
                .expect("a list of names")
                .iter()
                .map(|v| v.as_str().expect("a name").to_ascii_uppercase())
                .collect()
        };
        Lists {
            refused_names: words("refused", "names"),
            refused_prefixes: words("refused", "prefixes"),
            scrubbed_names: words("scrubbed", "names"),
            scrubbed_prefixes: words("scrubbed", "prefixes"),
        }
    })
}

/// Why a provider's key cannot become the variable of that name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// It would change how the command runs.
    Reserved,
    /// No process could read it as a variable.
    NotIdentifier,
}

/// `name` is the variable `penv run` would set, which is the key's own name.
pub fn refused(name: &str) -> Option<Refused> {
    if !is_identifier(name) {
        return Some(Refused::NotIdentifier);
    }
    let upper = name.to_ascii_uppercase();
    let lists = lists();
    let reserved = lists.refused_names.contains(&upper)
        || lists.refused_prefixes.iter().any(|p| upper.starts_with(p));
    reserved.then_some(Refused::Reserved)
}

/// `^[A-Za-z_][A-Za-z0-9_]*$`.
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// True for an inherited variable the child never gets.
pub fn scrubbed(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    let lists = lists();
    lists.scrubbed_names.contains(&upper)
        || lists.scrubbed_prefixes.iter().any(|p| upper.starts_with(p))
}

/// Refuse the run when any key would set a variable it may not. Each pair is
/// the parameter as a person knows it (`path/name`) and the variable's name.
pub fn check<'a>(keys: impl IntoIterator<Item = (String, &'a str)>) -> Result<(), CliError> {
    let mut reasons = Vec::new();
    for (parameter, name) in keys {
        match refused(name) {
            Some(Refused::Reserved) => reasons.push(format!(
                "{parameter} maps to {}, which would change how your command runs.",
                name.to_ascii_uppercase()
            )),
            Some(Refused::NotIdentifier) => reasons.push(format!(
                "{parameter} maps to {name}, which is not a variable name: letters, digits and _, not starting with a digit."
            )),
            None => {}
        }
    }
    if reasons.is_empty() {
        return Ok(());
    }
    reasons.sort();
    reasons.dedup();
    Err(CliError::new(
        "reserved_name",
        reasons.join(" "),
        "Rename the parameter. penv run never sets a variable that steers a shell, a runtime, a compiler or a package manager, or a name no process can read.",
    )
    .with_exit(Exit::Validation))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Upper, lower and mixed case of one name.
    fn cases(name: &str) -> [String; 3] {
        let mixed: String = name
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if i % 2 == 0 {
                    c.to_ascii_lowercase()
                } else {
                    c.to_ascii_uppercase()
                }
            })
            .collect();
        [name.to_ascii_uppercase(), name.to_ascii_lowercase(), mixed]
    }

    #[test]
    fn every_prefix_is_refused_in_any_case() {
        for prefix in &lists().refused_prefixes {
            for name in cases(&format!("{prefix}X")) {
                assert_eq!(refused(&name), Some(Refused::Reserved), "{name}");
            }
        }
        for name in [
            "LD_PRELOAD",
            "PIP_INDEX_URL",
            "YARN_NPM_REGISTRY_SERVER",
            "CARGO_REGISTRIES_X_INDEX",
            "PYTHONSTARTUP",
            "PENV_TOKEN",
        ] {
            assert_eq!(refused(name), Some(Refused::Reserved), "{name}");
        }
    }

    #[test]
    fn exact_names_are_refused_in_any_case() {
        for sample in [
            "NODE_OPTIONS",
            "BASH_ENV",
            "PATH",
            "HOME",
            "IFS",
            "GLIBC_TUNABLES",
            "JAVA_TOOL_OPTIONS",
            "_JAVA_OPTIONS",
            "DOTNET_STARTUP_HOOKS",
            "RUSTC_WRAPPER",
            "OPENSSL_CONF",
            "GIT_SSH_COMMAND",
            "EDITOR",
            "SSH_AUTH_SOCK",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_API_URL",
        ] {
            for name in cases(sample) {
                assert_eq!(refused(&name), Some(Refused::Reserved), "{name}");
            }
        }
    }

    #[test]
    fn an_ordinary_name_passes() {
        for name in [
            "DATABASE_URL",
            "STRIPE_SECRET_KEY",
            "PORT",
            "node_env",
            "_PRIVATE",
            "ENVIRONMENT",
            "PATHNAME",
            "HOMEPAGE_URL",
            "PERSONAL_TOKEN",
            "GITLAB_TOKEN",
        ] {
            assert_eq!(refused(name), None, "{name}");
        }
    }

    #[test]
    fn a_name_no_process_can_read_is_refused() {
        for name in [
            "",
            "1PASSWORD",
            "my.key",
            "my-key",
            "A B",
            "KEY=",
            "ÄPFEL",
            "a/b",
        ] {
            assert_eq!(refused(name), Some(Refused::NotIdentifier), "{name:?}");
        }
    }

    #[test]
    fn the_refusal_names_the_parameter_and_the_variable_and_never_a_value() {
        let error = check([
            ("db/node_options".to_string(), "node_options"),
            ("PORT".to_string(), "PORT"),
            ("api/my.key".to_string(), "my.key"),
        ])
        .unwrap_err();
        assert_eq!(error.code, "reserved_name");
        assert_eq!(error.exit, Exit::Validation);
        assert!(
            error.message.contains(
                "db/node_options maps to NODE_OPTIONS, which would change how your command runs."
            ),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("api/my.key maps to my.key"),
            "{}",
            error.message
        );
        assert!(!error.message.contains("PORT"), "{}", error.message);
        assert!(
            error.fix.starts_with("Rename the parameter."),
            "{}",
            error.fix
        );
        assert!(check([("PORT".to_string(), "PORT")]).is_ok());
    }

    #[test]
    fn the_runner_s_tokens_and_step_files_are_scrubbed_and_nothing_else() {
        for name in [
            "ACTIONS_ID_TOKEN_REQUEST_URL",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
            "ACTIONS_RUNTIME_TOKEN",
            "ACTIONS_RUNTIME_URL",
            "ACTIONS_RESULTS_URL",
            "ACTIONS_CACHE_URL",
            "GITHUB_STATE",
            "GITHUB_ENV",
            "GITHUB_OUTPUT",
            "GITHUB_PATH",
            "INPUT_TOKEN",
            "STATE_x",
            "github_env",
        ] {
            assert!(scrubbed(name), "{name}");
        }
        for name in [
            "PATH",
            "HOME",
            "GITHUB_SHA",
            "GITHUB_REPOSITORY",
            "CI",
            "PENV_TOKEN",
            "ACTIONS_STEP_DEBUG",
        ] {
            assert!(!scrubbed(name), "{name}");
        }
    }
}
