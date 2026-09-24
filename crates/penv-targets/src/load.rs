use crate::error::Error;
use crate::folder::{self, BuiltIn, Roots, Source, Tree};
use crate::target::{Target, is_name, parse};

macro_rules! built_in {
    ($name:literal) => {
        BuiltIn {
            name: $name,
            files: &[
                (
                    "target.toml",
                    include_str!(concat!("../targets/", $name, "/target.toml")),
                ),
                (
                    "env.tmpl",
                    include_str!(concat!("../targets/", $name, "/env.tmpl")),
                ),
            ],
        }
    };
}

/// The folders shipped inside the binary. The layout on disk is the same one a
/// user target uses.
pub const BUILT_IN: &[BuiltIn] = &[
    built_in!("ts"),
    built_in!("py"),
    built_in!("go"),
    built_in!("rust"),
    built_in!("php"),
    built_in!("java"),
    built_in!("csharp"),
];

/// One place a target reads from: its two files, each read once, and where
/// the `target.toml` really lives, for messages.
struct Layer {
    source: Source,
    at: String,
    config: String,
    template: Option<String>,
}

/// Repo folder, then home folder, then built in. A file the winning folder does
/// not hold comes from the next place, so an override inherits the rest.
pub fn load(tree: &dyn Tree, roots: &Roots, name: &str) -> Result<Target, Error> {
    if !is_name(name) {
        return Err(Error::Name {
            name: name.to_string(),
        });
    }
    let places = folder::places(roots, "targets", name);
    let mut layers: Vec<Layer> = Vec::new();
    for (source, dir) in &places {
        let path = format!("{dir}/target.toml");
        let template = tree.read(&format!("{dir}/env.tmpl"));
        let Some(config) = tree.read(&path) else {
            // A template on its own overrides nothing and is a typo worth naming.
            if template.is_some() {
                return Err(Error::Malformed {
                    dir: dir.clone(),
                    message: "there is an env.tmpl but no target.toml".into(),
                });
            }
            continue;
        };
        layers.push(Layer {
            source: *source,
            at: tree.origin(&path).unwrap_or_else(|| dir.clone()),
            config,
            template,
        });
    }
    if let Some(built_in) = BUILT_IN.iter().find(|b| b.name == name) {
        layers.push(Layer {
            source: Source::BuiltIn,
            at: "built in".into(),
            config: built_in.file("target.toml").unwrap_or_default().to_string(),
            template: built_in.file("env.tmpl").map(str::to_string),
        });
    }
    let Some(winner) = layers.first() else {
        let mut looked: Vec<String> = places.into_iter().map(|(_, dir)| dir).collect();
        looked.push("built in".into());
        return Err(Error::NotFound {
            name: name.to_string(),
            looked,
        });
    };

    // Key by key inside a table too, so an override naming one `[options]` knob
    // keeps the other knobs and the [types] map it inherits.
    let mut config = toml::Table::new();
    let mut output_source = Source::BuiltIn;
    for layer in layers.iter().rev() {
        let table: toml::Table = toml::from_str(&layer.config).map_err(|e| Error::Malformed {
            dir: layer.at.clone(),
            message: e.message().to_string(),
        })?;
        if table.contains_key("output") {
            output_source = layer.source;
        }
        merge(&mut config, table);
    }
    let template = layers
        .iter()
        .find_map(|layer| layer.template.clone())
        .ok_or_else(|| Error::Malformed {
            dir: winner.at.clone(),
            message: "there is a target.toml but no env.tmpl".into(),
        })?;
    parse(
        name,
        &config,
        &template,
        winner.source,
        output_source,
        &winner.at,
    )
}

/// `from` over `into`, key by key inside tables. `[[option]]` blocks merge by
/// `name` and `[[suggest]]` blocks by `option`, so adding one keeps the rest.
pub fn merge(into: &mut toml::Table, from: toml::Table) {
    for (field, value) in from {
        let by = match field.as_str() {
            "option" => Some("name"),
            "suggest" => Some("option"),
            _ => None,
        };
        match (into.get_mut(&field), value, by) {
            (Some(toml::Value::Table(held)), toml::Value::Table(table), _) => merge(held, table),
            (Some(toml::Value::Array(held)), toml::Value::Array(blocks), Some(by)) => {
                for block in blocks {
                    let named = block.get(by).cloned();
                    match held
                        .iter_mut()
                        .find(|h| named.is_some() && h.get(by) == named.as_ref())
                    {
                        Some(slot) => *slot = block,
                        None => held.push(block),
                    }
                }
            }
            (_, value, _) => {
                into.insert(field, value);
            }
        }
    }
}

/// Every target name this repository has, each one loaded or the reason it
/// could not be. A folder nobody can read is named rather than dropped.
pub fn available(tree: &dyn Tree, roots: &Roots) -> Vec<Result<Target, Error>> {
    let mut names = folder::names(tree, roots, "targets", BUILT_IN);
    names.sort();
    names.iter().map(|name| load(tree, roots, name)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder::Source;
    use std::collections::BTreeMap;

    #[derive(Clone, Default)]
    struct Fake(BTreeMap<String, String>);

    impl Fake {
        fn with(mut self, path: &str, contents: &str) -> Fake {
            self.0.insert(path.to_string(), contents.to_string());
            self
        }
    }

    impl Tree for Fake {
        fn read(&self, path: &str) -> Option<String> {
            self.0.get(path).cloned()
        }
        fn dirs(&self, path: &str) -> Vec<String> {
            let prefix = format!("{path}/");
            let mut out: Vec<String> = self
                .0
                .keys()
                .filter_map(|p| p.strip_prefix(&prefix))
                .filter_map(|rest| rest.split('/').next())
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect();
            out.dedup();
            out
        }
        fn files(&self, path: &str) -> Vec<String> {
            let prefix = format!("{path}/");
            self.0
                .keys()
                .filter_map(|p| p.strip_prefix(&prefix))
                .filter(|rest| !rest.contains('/'))
                .map(str::to_string)
                .collect()
        }
    }

    const TYPES: &str = r#"
[types]
string = "S"
number = "N"
boolean = "B"
url = "U"
email = "E"
port = "P"
enum = "values | join('|')"
"#;

    fn roots() -> Roots {
        Roots::new("/repo", Some("/home".into()))
    }

    fn custom(name: &str) -> String {
        format!("name = \"{name}\"\noutput = \"out.txt\"\n{TYPES}")
    }

    #[test]
    fn built_in_targets_load_with_no_tree_at_all() {
        let tree = Fake::default();
        let ts = load(&tree, &roots(), "ts").unwrap();
        assert_eq!(ts.source, Source::BuiltIn);
        assert_eq!(ts.output, "src/env.ts");
        let py = load(&tree, &roots(), "py").unwrap();
        assert_eq!(py.output, "penv_env.py");
    }

    #[test]
    fn the_repo_folder_beats_the_home_folder_and_the_built_in_one() {
        let tree = Fake::default()
            .with("/repo/.penv/targets/ts/target.toml", &custom("ts"))
            .with("/repo/.penv/targets/ts/env.tmpl", "repo")
            .with("/home/.penv/targets/ts/target.toml", &custom("ts"))
            .with("/home/.penv/targets/ts/env.tmpl", "home");
        let target = load(&tree, &roots(), "ts").unwrap();
        assert_eq!(target.source, Source::Repo);
        assert_eq!(target.template, "repo");
        assert_eq!(target.dir, "/repo/.penv/targets/ts");
    }

    #[test]
    fn the_home_folder_beats_the_built_in_one() {
        let tree = Fake::default()
            .with("/home/.penv/targets/py/target.toml", &custom("py"))
            .with("/home/.penv/targets/py/env.tmpl", "home");
        let target = load(&tree, &roots(), "py").unwrap();
        assert_eq!(target.source, Source::User);
        assert_eq!(target.output, "out.txt");
    }

    #[test]
    fn an_unknown_name_says_where_it_looked() {
        let error = load(&Fake::default(), &roots(), "zig").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("/repo/.penv/targets/zig"), "{message}");
        assert!(message.contains("/home/.penv/targets/zig"), "{message}");
        assert!(message.contains("built in"), "{message}");
    }

    #[test]
    fn a_repo_folder_of_only_a_target_toml_keeps_the_built_in_template() {
        let tree = Fake::default().with(
            "/repo/.penv/targets/ts/target.toml",
            "name = \"ts\"\noutput = \"apps/web/src/env.ts\"\n",
        );
        let target = load(&tree, &roots(), "ts").unwrap();
        assert_eq!(target.source, Source::Repo);
        assert_eq!(target.output, "apps/web/src/env.ts");
        assert!(target.template.contains("export const env"), "built in");
        assert_eq!(target.types["port"], "number", "the built-in [types] too");
    }

    #[test]
    fn a_partial_repo_folder_reads_the_home_folder_before_the_built_in_one() {
        let tree = Fake::default()
            .with(
                "/repo/.penv/targets/ts/target.toml",
                "name = \"ts\"\noutput = \"apps/web/src/env.ts\"\n",
            )
            .with("/home/.penv/targets/ts/target.toml", &custom("ts"))
            .with("/home/.penv/targets/ts/env.tmpl", "home");
        let target = load(&tree, &roots(), "ts").unwrap();
        assert_eq!(target.source, Source::Repo);
        assert_eq!(target.output, "apps/web/src/env.ts");
        assert_eq!(target.template, "home");
    }

    #[test]
    fn a_template_with_no_target_toml_beside_it_is_a_broken_folder() {
        let tree = Fake::default().with("/repo/.penv/targets/ts/env.tmpl", "repo");
        let error = load(&tree, &roots(), "ts").unwrap_err();
        assert!(error.to_string().contains("no target.toml"), "{error}");
    }

    #[test]
    fn a_target_toml_no_place_has_a_template_for_is_a_broken_folder() {
        let tree = Fake::default().with("/repo/.penv/targets/zig/target.toml", &custom("zig"));
        let error = load(&tree, &roots(), "zig").unwrap_err();
        assert!(error.to_string().contains("no env.tmpl"), "{error}");
    }

    #[test]
    fn available_lists_the_built_ins_plus_whatever_the_folders_add() {
        let tree = Fake::default()
            .with("/repo/.penv/targets/zig/target.toml", &custom("zig"))
            .with("/repo/.penv/targets/zig/env.tmpl", "x");
        let found = available(&tree, &roots());
        let names: Vec<String> = found
            .iter()
            .map(|t| t.as_ref().unwrap().name.clone())
            .collect();
        assert_eq!(
            names,
            ["csharp", "go", "java", "php", "py", "rust", "ts", "zig"]
        );
    }

    #[test]
    fn a_folder_nobody_can_read_is_listed_as_the_error_it_is() {
        let tree = Fake::default().with("/repo/.penv/targets/zig/target.toml", "name = \"zig\"\n");
        let broken = available(&tree, &roots())
            .into_iter()
            .find(Result::is_err)
            .expect("the malformed folder was dropped instead of named")
            .unwrap_err();
        assert!(
            broken.to_string().contains("/repo/.penv/targets/zig"),
            "{broken}"
        );
    }

    #[test]
    fn a_target_is_detected_where_any_file_it_names_sits() {
        let tree = Fake::default().with("/repo/package.json", "{}");
        let r = roots();
        let scan = crate::detect::Scan::new(&tree, &r);
        let ts = load(&tree, &r, "ts").unwrap();
        let py = load(&tree, &r, "py").unwrap();
        assert_eq!(scan.candidates(&ts), [""], "one detect file is enough");
        assert!(scan.candidates(&py).is_empty());
    }

    #[test]
    fn a_name_that_is_not_a_word_never_reaches_a_path() {
        let tree = Fake::default()
            .with("/x/target.toml", &custom("x"))
            .with("/x/env.tmpl", "x");
        for name in ["../../x", "a/b", "", "Ts", "ts.tmpl"] {
            let error = load(&tree, &roots(), name).unwrap_err();
            assert!(matches!(error, Error::Name { .. }), "{name}: {error}");
        }
    }

    #[test]
    fn adding_one_option_block_keeps_the_ones_it_inherits() {
        let tree = Fake::default().with(
            "/repo/.penv/targets/ts/target.toml",
            "name = \"ts\"\n[options]\nbanner = \"hi\"\n[[option]]\nname = \"banner\"\ndefault = \"\"\nabout = \"A line on top.\"\n[[option]]\nname = \"mask\"\ndefault = false\nvalues = [true, false]\nabout = \"Masking, off here.\"\n[[suggest]]\noption = \"runtime\"\nprompt = \"which runtime?\"\n[[suggest.rule]]\nvalue = \"deno\"\nfiles = [\"deno.json\"]\n",
        );
        let target = load(&tree, &roots(), "ts").unwrap();
        let names: Vec<&str> = target.knobs.iter().map(|k| k.name.as_str()).collect();
        assert_eq!(names, ["key_case", "mask", "runtime", "banner"]);
        let mask = target.knobs.iter().find(|k| k.name == "mask").unwrap();
        assert_eq!(
            mask.default,
            toml::Value::Boolean(false),
            "a block by the same name replaces"
        );
        assert_eq!(target.suggest.len(), 1, "replaced by option, not added");
        assert_eq!(target.suggest[0].prompt, "which runtime?");
        assert_eq!(target.suggest[0].rule.len(), 1);
    }

    #[test]
    fn a_broken_home_file_is_named_and_not_the_repo_one_over_it() {
        let tree = Fake::default()
            .with(
                "/repo/.penv/targets/ts/target.toml",
                "name = \"ts\"\noutput = \"a.ts\"\n",
            )
            .with("/home/.penv/targets/ts/target.toml", "name = ");
        let message = load(&tree, &roots(), "ts").unwrap_err().to_string();
        assert!(message.starts_with("/home/.penv/targets/ts "), "{message}");
    }

    #[test]
    fn a_config_section_that_breaks_is_named_as_the_section() {
        let tree = Fake::default().with(
            "/repo/.penv/config.toml",
            "[targets.ts]\noutput = \"a.ts\"\ndetects = []\n",
        );
        let settled = crate::Settled::new(&tree, "/repo").unwrap();
        let message = load(&settled, &roots(), "ts").unwrap_err().to_string();
        assert!(
            message.starts_with("/repo/.penv/config.toml [targets.ts] "),
            "{message}"
        );
    }

    #[test]
    fn only_a_repo_folder_naming_output_decides_where_a_file_goes() {
        let built_in = load(&Fake::default(), &roots(), "ts").unwrap();
        assert_eq!(built_in.output_source, Source::BuiltIn);

        let options_only = Fake::default().with(
            "/repo/.penv/targets/ts/target.toml",
            "name = \"ts\"\n[options]\nkey_case = \"camel\"\n",
        );
        let target = load(&options_only, &roots(), "ts").unwrap();
        assert_eq!(target.source, Source::Repo);
        assert_eq!(
            target.output_source,
            Source::BuiltIn,
            "an override that says nothing about output decides nothing"
        );
        assert_eq!(target.options["key_case"].as_str(), Some("camel"));
        assert_eq!(
            target.options["runtime"].as_str(),
            Some("node"),
            "one knob overridden keeps the others"
        );

        let named = Fake::default().with(
            "/repo/.penv/targets/ts/target.toml",
            "name = \"ts\"\noutput = \"apps/web/src/env.ts\"\n",
        );
        assert_eq!(
            load(&named, &roots(), "ts").unwrap().output_source,
            Source::Repo
        );
    }
}
