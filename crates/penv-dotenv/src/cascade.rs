//! Which files hold values for an environment, in the order they are layered.
//! Later files win. `*.local` files belong to one machine and are never pushed.

/// `.env`, `.env.local`, `.env.<env>`, `.env.<env>.local`. `test` skips
/// `.env.local`, as dotenv-flow and Next.js do, so a test run is the same on
/// every machine.
pub fn cascade(environment: &str) -> Vec<String> {
    let mut files = vec![".env".to_string()];
    if environment != "test" {
        files.push(".env.local".to_string());
    }
    files.push(format!(".env.{environment}"));
    files.push(format!(".env.{environment}.local"));
    files
}

/// The machine-local layers, which sit on top of cloud values too.
pub fn overlays(environment: &str) -> Vec<String> {
    cascade(environment)
        .into_iter()
        .filter(|f| is_local(f))
        .collect()
}

/// The shared layers: what `push` sends and then removes.
pub fn shared(environment: &str) -> Vec<String> {
    cascade(environment)
        .into_iter()
        .filter(|f| !is_local(f))
        .collect()
}

pub fn is_local(file: &str) -> bool {
    file.ends_with(".local")
}

/// Suffixes of files beside `.env` that hold no values for penv: the schema,
/// examples and templates, and dotenv-vault's and dotenvx's encrypted file and
/// key file.
pub const NOT_VALUES: [&str; 8] = [
    "schema", "example", "sample", "template", "defaults", "vault", "keys", "me",
];

/// An environment name penv can put in a file name: letters, digits, `_`, `-`
/// and inner dots, and none that would name `.env.local` or a file in
/// [`NOT_VALUES`].
pub fn is_environment_name(name: &str) -> bool {
    let shaped = !name.is_empty()
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    shaped && name != "local" && !NOT_VALUES.contains(&name)
}

/// A file name that is part of some environment's cascade. Schema, examples and
/// templates are not values.
pub fn is_value_file(name: &str) -> bool {
    (name == ".env" || name.starts_with(".env."))
        && !NOT_VALUES
            .iter()
            .any(|suffix| name == format!(".env.{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layers_run_shared_then_local_then_the_environment() {
        assert_eq!(
            cascade("staging"),
            [".env", ".env.local", ".env.staging", ".env.staging.local"]
        );
        assert_eq!(overlays("staging"), [".env.local", ".env.staging.local"]);
        assert_eq!(shared("staging"), [".env", ".env.staging"]);
        assert_eq!(cascade("test"), [".env", ".env.test", ".env.test.local"]);
    }

    #[test]
    fn the_schema_and_examples_hold_no_values() {
        assert!(is_value_file(".env.production"));
        assert!(!is_value_file(".env.schema"));
        assert!(!is_value_file(".env.example"));
        assert!(!is_value_file(".envrc"));
        assert!(!is_value_file(".env.vault") && !is_value_file(".env.keys"));
    }

    #[test]
    fn an_environment_name_cannot_leave_the_folder_or_name_another_file() {
        for good in ["production", "staging-eu", "pr_42", "prod.eu"] {
            assert!(is_environment_name(good), "{good}");
        }
        for bad in [
            "", "../x", "a/b", "a\\b", "local", "schema", "keys", ".hidden", "a..b", "x:y",
        ] {
            assert!(!is_environment_name(bad), "{bad}");
        }
    }
}
