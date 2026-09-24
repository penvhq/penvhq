//! The line a reader pastes to use what `gen` wrote. penv never edits a
//! tsconfig or any other build config; it prints the import and stops there.

use serde_json::Value;

use crate::detect::layout_root;
use crate::target::{Target, word};

/// One config in an `extends` chain: the directory it sits in, relative to the
/// repository root, and its text. The package's own file comes first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub dir: String,
    pub source: String,
}

impl Config {
    pub fn new(dir: impl Into<String>, source: impl Into<String>) -> Config {
        Config {
            dir: dir.into(),
            source: source.into(),
        }
    }
}

/// The file an `extends` names, against the config naming it. A bare specifier
/// resolves through node_modules, which penv cannot prove, so it is not followed.
pub fn extends_of(config: &Config) -> Option<String> {
    let json: Value = serde_json::from_str(&plain_json(&config.source)).ok()?;
    let named = json.get("extends")?.as_str()?;
    if !named.starts_with("./") && !named.starts_with("../") {
        return None;
    }
    let path = join(&config.dir, named)?;
    Some(if path.ends_with(".json") {
        path
    } else {
        format!("{path}.json")
    })
}

/// The import line for a file the target wrote. `path` is relative to the
/// repository root, and `configs` is the `paths_from` chain read at the edge.
/// `{specifier}`, `{module}` and `{dir}` come from the path, and `{<knob>}` is
/// that option's value in effect.
pub fn import_line(
    target: &Target,
    package: &str,
    path: &str,
    configs: &[Config],
) -> Option<String> {
    let line = target.import.as_ref()?;
    let relative = relative_to(package, path);
    let specifier =
        aliased(configs, path).unwrap_or_else(|| format!("./{}", drop_extension(&relative)));
    let module = match layout_root(target, &relative) {
        Some(root) => relative[root.len() + 1..].to_string(),
        None => relative.clone(),
    };
    let mut line = line
        .replace("{specifier}", &specifier)
        .replace("{module}", &drop_extension(&module).replace('/', "."))
        .replace("{dir}", &parent(&relative));
    for (knob, value) in target.effective() {
        line = line.replace(&format!("{{{}}}", knob.name), &word(value));
    }
    Some(line)
}

fn parent(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default()
}

/// The specifier a `paths` map already gives this file. Targets resolve against
/// `baseUrl`, every path is relative to the repository root, and the longest wins.
pub fn aliased(configs: &[Config], path: &str) -> Option<String> {
    let (paths_at, paths) = first(configs, "paths")?;
    let paths = paths.as_object()?;
    let base = match first(configs, "baseUrl") {
        Some((at, url)) => join(&at, url.as_str()?)?,
        None => paths_at,
    };
    let stem = drop_extension(path);

    let mut best: Option<(usize, String)> = None;
    for (alias, targets) in paths {
        for target in targets.as_array().into_iter().flatten() {
            let Some(target) = target.as_str().and_then(|t| join(&base, t)) else {
                continue;
            };
            let hit = match (alias.strip_suffix('*'), target.strip_suffix('*')) {
                (Some(head), Some(prefix)) => path
                    .strip_prefix(prefix)
                    .map(|rest| (prefix.len(), format!("{head}{}", drop_extension(rest)))),
                (None, None) if drop_extension(&target) == stem => {
                    Some((target.len(), alias.clone()))
                }
                _ => None,
            };
            if let Some((length, specifier)) = hit
                && best.as_ref().is_none_or(|(best, _)| length > *best)
            {
                best = Some((length, specifier));
            }
        }
    }
    best.map(|(_, specifier)| specifier)
}

/// One `compilerOptions` field and the directory of the config that set it.
/// `extends` overrides field by field, so the nearest config wins.
fn first(configs: &[Config], field: &str) -> Option<(String, Value)> {
    configs.iter().find_map(|config| {
        let json: Value = serde_json::from_str(&plain_json(&config.source)).ok()?;
        let value = json.get("compilerOptions")?.get(field)?.clone();
        Some((config.dir.clone(), value))
    })
}

/// Two forward-slash paths joined and reduced: no `.` segments left, and `..`
/// taken off the end of what came before it. `None` when a `..` climbs above
/// the repository root, where penv cannot follow.
pub fn join(base: &str, rest: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for segment in base.split('/').chain(rest.split('/')) {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            name => out.push(name),
        }
    }
    Some(out.join("/"))
}

/// A path relative to the package that holds it, in forward slashes.
fn relative_to(package: &str, path: &str) -> String {
    if package.is_empty() {
        return path.to_string();
    }
    path.strip_prefix(&format!("{package}/"))
        .unwrap_or(path)
        .to_string()
}

fn drop_extension(path: &str) -> String {
    let start = path.rfind('/').map_or(0, |slash| slash + 1);
    match path[start..].rfind('.') {
        Some(dot) if dot > 0 => path[..start + dot].to_string(),
        _ => path.to_string(),
    }
}

/// A tsconfig is JSON with comments and trailing commas, and an editor may have
/// left a byte order mark on it; all three go before serde reads it.
fn plain_json(source: &str) -> String {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            in_string = c != '"' || escaped;
            escaped = c == '\\' && !escaped;
            continue;
        }
        match (c, chars.peek()) {
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                let mut star = false;
                for c in chars.by_ref() {
                    if star && c == '/' {
                        break;
                    }
                    star = c == '*';
                }
            }
            _ => {
                in_string = c == '"';
                escaped = false;
                out.push(c);
            }
        }
    }
    drop_trailing_commas(&out)
}

fn drop_trailing_commas(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut in_string = false;
    let mut escaped = false;
    for c in source.chars() {
        if !in_string && c == ',' {
            let rest = &source[out.len() + 1..];
            if rest.trim_start().starts_with([']', '}']) {
                out.push(' ');
                continue;
            }
        }
        if in_string {
            in_string = c != '"' || escaped;
            escaped = c == '\\' && !escaped;
        } else {
            in_string = c == '"';
            escaped = false;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder::Source;
    use crate::target::Layout;
    use std::collections::BTreeMap;

    fn target(import: Option<&str>) -> Target {
        Target {
            name: "ts".into(),
            output: "src/env.ts".into(),
            output_source: Source::BuiltIn,
            detect: Vec::new(),
            types: BTreeMap::new(),
            options: toml::Table::new(),
            knobs: Vec::new(),
            suggest: Vec::new(),
            layout: Vec::new(),
            check: None,
            import: import.map(str::to_string),
            paths_from: None,
            template: String::new(),
            source: Source::BuiltIn,
            dir: "built in".into(),
        }
    }

    const TS: &str = "import { env } from \"{specifier}\";";

    fn own(source: &str) -> Vec<Config> {
        vec![Config::new("", source)]
    }

    fn at(dir: &str, source: &str) -> Vec<Config> {
        vec![Config::new(dir, source)]
    }

    #[test]
    fn a_target_that_names_no_import_line_prints_none() {
        assert_eq!(
            import_line(&target(None), "apps/web", "apps/web/src/env.ts", &[]),
            None
        );
    }

    #[test]
    fn the_specifier_is_relative_to_the_package_that_holds_the_file() {
        assert_eq!(
            import_line(&target(Some(TS)), "apps/web", "apps/web/src/env.ts", &[]),
            Some("import { env } from \"./src/env\";".to_string())
        );
        assert_eq!(
            import_line(&target(Some(TS)), "", "src/env.ts", &[]),
            Some("import { env } from \"./src/env\";".to_string())
        );
    }

    #[test]
    fn a_paths_entry_that_already_reaches_the_file_wins() {
        let config = r#"{
          // the app's own alias
          "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } },
        }"#;
        assert_eq!(
            import_line(
                &target(Some(TS)),
                "apps/web",
                "apps/web/src/env.ts",
                &at("apps/web", config)
            ),
            Some("import { env } from \"@/env\";".to_string())
        );
    }

    #[test]
    fn the_longest_matching_paths_entry_is_the_one_a_reader_would_write() {
        let config = r#"{ "compilerOptions": { "paths": {
            "~/*": ["./*"],
            "@config/*": ["./src/config/*"]
        } } }"#;
        assert_eq!(
            aliased(&own(config), "src/config/env.ts").as_deref(),
            Some("@config/env")
        );
    }

    #[test]
    fn a_paths_entry_without_a_star_maps_one_file() {
        let config = r##"{ "compilerOptions": { "paths": { "#env": ["./src/env.ts"] } } }"##;
        assert_eq!(aliased(&own(config), "src/env.ts").as_deref(), Some("#env"));
    }

    #[test]
    fn a_paths_map_that_points_elsewhere_leaves_the_relative_path() {
        let config = r#"{ "compilerOptions": { "paths": { "@/*": ["./lib/*"] } } }"#;
        assert_eq!(aliased(&own(config), "src/env.ts"), None);
    }

    #[test]
    fn a_config_with_no_paths_at_all_is_not_an_error() {
        assert_eq!(aliased(&own("{}"), "src/env.ts"), None);
        assert_eq!(aliased(&own("not json"), "src/env.ts"), None);
        assert_eq!(aliased(&[], "src/env.ts"), None);
        assert_eq!(
            aliased(
                &own(r#"{ "compilerOptions": { "strict": true } }"#),
                "src/env.ts"
            ),
            None
        );
    }

    #[test]
    fn a_paths_target_resolves_against_the_base_url_and_not_the_config() {
        let config =
            r#"{ "compilerOptions": { "baseUrl": "./src", "paths": { "@/*": ["./*"] } } }"#;
        assert_eq!(
            aliased(&own(config), "src/env.ts").as_deref(),
            Some("@/env")
        );
        assert_eq!(
            aliased(&own(config), "lib/env.ts"),
            None,
            "the alias reaches src, not lib"
        );
    }

    #[test]
    fn an_extends_chain_carries_the_paths_map_the_package_never_repeats() {
        let chain = [
            Config::new("apps/web", r#"{ "extends": "../../tsconfig.base.json" }"#),
            Config::new(
                "",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./apps/web/src/*"] } } }"#,
            ),
        ];
        assert_eq!(
            aliased(&chain, "apps/web/src/env.ts").as_deref(),
            Some("@/env")
        );
    }

    #[test]
    fn the_nearest_config_in_a_chain_is_the_one_that_decides() {
        let chain = [
            Config::new(
                "apps/web",
                r#"{ "compilerOptions": { "paths": { "~/*": ["./src/*"] } } }"#,
            ),
            Config::new(
                "",
                r#"{ "compilerOptions": { "paths": { "@/*": ["./*"] } } }"#,
            ),
        ];
        assert_eq!(
            aliased(&chain, "apps/web/src/env.ts").as_deref(),
            Some("~/env")
        );
    }

    #[test]
    fn an_extends_is_followed_only_when_it_names_a_relative_file() {
        let own = Config::new("", r#"{ "extends": "./tsconfig.base.json" }"#);
        assert_eq!(extends_of(&own).as_deref(), Some("tsconfig.base.json"));

        let up = Config::new("packages/web", r#"{ "extends": "../base" }"#);
        assert_eq!(extends_of(&up).as_deref(), Some("packages/base.json"));

        let package = Config::new("", r#"{ "extends": "@tsconfig/node20/tsconfig.json" }"#);
        assert_eq!(extends_of(&package), None, "penv cannot prove node_modules");
        assert_eq!(extends_of(&Config::new("", "{}")), None);
    }

    #[test]
    fn a_dotted_module_name_is_what_a_python_import_needs() {
        let mut py = target(Some("from {module} import env"));
        py.output = "penv_env.py".into();
        assert_eq!(
            import_line(&py, "services/api", "services/api/penv_env.py", &[]),
            Some("from penv_env import env".to_string())
        );
    }

    #[test]
    fn a_layout_root_is_the_package_root_and_never_part_of_the_module() {
        let mut py = target(Some("from {module} import env"));
        py.output = "penv_env.py".into();
        py.layout = vec![Layout {
            when: "src/*/__init__.py".into(),
            output: "src/*/penv_env.py".into(),
            root: Some("src".into()),
        }];
        assert_eq!(
            import_line(&py, "api", "api/src/billing/penv_env.py", &[]),
            Some("from billing.penv_env import env".to_string())
        );
    }

    #[test]
    fn comments_and_trailing_commas_do_not_stop_the_paths_map_being_read() {
        let config = "{\n  /* block */\n  \"compilerOptions\": {\n    \"paths\": { \"@/*\": [\"./src/*\"], },\n  },\n}";
        assert_eq!(
            aliased(&own(config), "src/env.ts").as_deref(),
            Some("@/env")
        );
    }

    #[test]
    fn a_byte_order_mark_an_editor_left_behind_is_not_a_broken_config() {
        let config = "\u{feff}{ \"compilerOptions\": { \"paths\": { \"@/*\": [\"./src/*\"] } } }";
        assert_eq!(
            aliased(&own(config), "src/env.ts").as_deref(),
            Some("@/env")
        );
        let extending = Config::new("", "\u{feff}{ \"extends\": \"./base.json\" }");
        assert_eq!(extends_of(&extending).as_deref(), Some("base.json"));
    }

    #[test]
    fn a_url_inside_a_string_is_not_a_comment() {
        let config = r#"{ "compilerOptions": { "paths": { "https://x/*": ["./src/*"] } } }"#;
        assert_eq!(
            aliased(&own(config), "src/env.ts").as_deref(),
            Some("https://x/env")
        );
    }

    #[test]
    fn a_joined_path_never_keeps_a_dot_segment() {
        assert_eq!(join("", "./src/env.ts").as_deref(), Some("src/env.ts"));
        assert_eq!(join(".", "./src/./env.ts").as_deref(), Some("src/env.ts"));
        assert_eq!(
            join("apps/web", "../api/src").as_deref(),
            Some("apps/api/src")
        );
        assert_eq!(join("apps/web", "./").as_deref(), Some("apps/web"));
    }

    #[test]
    fn a_path_that_climbs_above_the_repository_is_not_followed() {
        assert_eq!(join("apps/web", "../../../tsconfig.base.json"), None);
        let up = Config::new(
            "apps/web",
            r#"{ "extends": "../../../tsconfig.base.json" }"#,
        );
        assert_eq!(extends_of(&up), None);
        let base = Config::new(
            "apps/web",
            r#"{ "compilerOptions": { "baseUrl": "../../..", "paths": { "@/*": ["./apps/web/src/*"] } } }"#,
        );
        assert_eq!(
            aliased(&[base], "apps/web/src/env.ts"),
            None,
            "an alias nobody can prove is a relative import"
        );
        assert_eq!(
            join("apps/web", "../../tsconfig.base.json").as_deref(),
            Some("tsconfig.base.json")
        );
    }

    fn knob(name: &str, default: &str) -> crate::target::Knob {
        crate::target::Knob {
            name: name.into(),
            default: toml::Value::String(default.into()),
            values: Vec::new(),
            about: "x".into(),
        }
    }

    #[test]
    fn an_option_in_the_import_line_reads_the_value_in_effect() {
        let mut php = target(Some("use {namespace}\\Env;"));
        php.knobs = vec![knob("namespace", "App")];
        assert_eq!(
            import_line(&php, "", "src/Env.php", &[]).as_deref(),
            Some("use App\\Env;"),
            "the default when nothing set it"
        );
        php.options.insert(
            "namespace".into(),
            toml::Value::String("Acme\\Billing".into()),
        );
        assert_eq!(
            import_line(&php, "", "src/Env.php", &[]).as_deref(),
            Some("use Acme\\Billing\\Env;")
        );
    }

    #[test]
    fn a_go_import_names_the_directory_the_file_was_written_to() {
        let go = target(Some("import \"<module>/{dir}\""));
        assert_eq!(
            import_line(&go, "", "internal/config/env.go", &[]).as_deref(),
            Some("import \"<module>/internal/config\"")
        );
        assert_eq!(
            import_line(&go, "services/api", "services/api/env/env.go", &[]).as_deref(),
            Some("import \"<module>/env\"")
        );
    }
}
