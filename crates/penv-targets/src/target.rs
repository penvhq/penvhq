use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::Error;
use crate::folder::Source;

/// The schema base types a target must map. A target that misses one cannot
/// render every schema, so the loader refuses it. `integer` is not one of them:
/// it is the optional entry used for `number(isInt=true)`.
pub const BASE_TYPES: [&str; 7] = [
    "string", "number", "boolean", "url", "email", "port", "enum",
];

/// The `integer` entry, used when a number carries `isInt`.
pub const INT_TYPE: &str = "integer";

/// A value an `[options]` knob was settled to.
pub type OptionValue = toml::Value;

/// A target name is a word: it becomes a path segment and a file name.
pub(crate) fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// How `gen --check` compiles what the target rendered. `{file}` in an argument
/// is the rendered file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    /// The first element may name alternatives as `python3|python|py`.
    pub command: Vec<String>,
    /// What proves the toolchain is installed; a failure is a skip, not an error.
    #[serde(default)]
    pub probe: Vec<String>,
    /// Directories under the chosen package looked in before PATH.
    #[serde(default)]
    pub bin: Vec<String>,
    /// Extra files written beside the rendered one before the command runs.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

/// One `[[option]]` block: what an `[options]` knob changes, the values it
/// takes, and what it is when nobody says. `[options]` holds the values; this
/// says what they mean, so `gen --options` and the remembered override can.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Knob {
    pub name: String,
    pub default: toml::Value,
    /// The values this knob takes; empty means free text.
    #[serde(default)]
    pub values: Vec<toml::Value>,
    /// One line of plain English saying what the knob changes.
    pub about: String,
}

impl Knob {
    /// The values as the prompts, the table and the override write them.
    pub fn words(&self) -> Vec<String> {
        self.values.iter().map(word).collect()
    }
}

/// One option value as a word: a string is its own, anything else is its TOML.
pub fn word(value: &toml::Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

/// One `[options]` knob a folder asks penv to work out from the chosen package.
/// Its entry in `[options]` is the default, and the answer penv takes silently.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suggest {
    pub option: String,
    pub prompt: String,
    #[serde(default)]
    pub rule: Vec<Rule>,
}

impl Suggest {
    /// What the prompt offers: the `[options]` default first, then each value a
    /// rule sets, once.
    pub fn offered(&self, default: Option<&toml::Value>) -> Vec<toml::Value> {
        let mut out: Vec<toml::Value> = default.into_iter().cloned().collect();
        for rule in &self.rule {
            if !out.contains(&rule.value) {
                out.push(rule.value.clone());
            }
        }
        out
    }
}

/// A package holding one of `files` (carrying `contains`, when the rule names
/// it) sets the option to this value.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub value: toml::Value,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub contains: Option<String>,
}

/// An output shape a package's own layout asks for: `when` holds one `*` for a
/// directory name, and `output` and `root` read the same name back.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub when: String,
    pub output: String,
    /// The directory the language imports from, dropped from the import line.
    #[serde(default)]
    pub root: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub name: String,
    /// Where `gen` writes, relative to the directory holding `.env.schema`.
    pub output: String,
    /// Which folder set `output`; only a repo one decides where a file goes.
    pub output_source: Source,
    /// Any one of these files in a directory makes this target relevant there.
    pub detect: Vec<String>,
    /// Schema base type name to a language type. The `enum` entry is a
    /// minijinja expression over `values`.
    pub types: BTreeMap<String, String>,
    /// Whatever `[options]` holds, reaching the template as `options`. The
    /// folder names its own knobs; no entry has meaning in Rust.
    pub options: toml::Table,
    /// The `[[option]]` blocks describing those knobs, one per `[options]` key.
    pub knobs: Vec<Knob>,
    pub suggest: Vec<Suggest>,
    pub layout: Vec<Layout>,
    pub check: Option<Check>,
    /// The line `gen` prints so the reader knows how to import what it wrote.
    /// `{specifier}` is the path the language imports by, `{module}` its dotted
    /// module name.
    pub import: Option<String>,
    /// A file in the package directory whose `paths` map gives a shorter
    /// specifier than a relative path, such as a `tsconfig.json`.
    pub paths_from: Option<String>,
    pub template: String,
    pub source: Source,
    pub dir: String,
}

impl Target {
    /// Every knob at the value in effect: what `[options]` holds, or the
    /// `[[option]]` default when nothing set it.
    pub fn effective(&self) -> Vec<(&Knob, &toml::Value)> {
        self.knobs
            .iter()
            .map(|knob| (knob, self.options.get(&knob.name).unwrap_or(&knob.default)))
            .collect()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: String,
    output: String,
    #[serde(default)]
    detect: Vec<String>,
    #[serde(default)]
    types: BTreeMap<String, String>,
    #[serde(default)]
    options: toml::Table,
    #[serde(default)]
    option: Vec<Knob>,
    #[serde(default)]
    suggest: Vec<Suggest>,
    #[serde(default)]
    layout: Vec<Layout>,
    #[serde(default)]
    check: Option<Check>,
    #[serde(default)]
    import: Option<String>,
    #[serde(default)]
    paths_from: Option<String>,
}

/// Read one merged `target.toml`. `output_source` is the folder the merged
/// `output` came from, which is not always the folder that won the lookup.
pub fn parse(
    name: &str,
    config: &toml::Table,
    template: &str,
    source: Source,
    output_source: Source,
    dir: &str,
) -> Result<Target, Error> {
    let file: File =
        toml::Value::Table(config.clone())
            .try_into()
            .map_err(|e: toml::de::Error| Error::Malformed {
                dir: dir.to_string(),
                message: e.message().to_string(),
            })?;

    if file.name != name {
        return Err(Error::Malformed {
            dir: dir.to_string(),
            message: format!("target.toml names {}, the folder names {name}", file.name),
        });
    }
    if file.output.trim().is_empty() {
        return Err(Error::Malformed {
            dir: dir.to_string(),
            message: "output is empty".into(),
        });
    }
    for base in BASE_TYPES {
        if !file.types.contains_key(base) {
            return Err(Error::Malformed {
                dir: dir.to_string(),
                message: format!("[types] has no entry for {base}"),
            });
        }
    }
    if file.check.as_ref().is_some_and(|c| c.command.is_empty()) {
        return Err(Error::Malformed {
            dir: dir.to_string(),
            message: "[check] has an empty command".into(),
        });
    }
    // Each one is written beside the rendered file in a scratch directory.
    for extra in file.check.iter().flat_map(|c| c.files.keys()) {
        if extra.is_empty() || extra.contains(['/', '\\', ':']) || extra == "." || extra == ".." {
            return Err(Error::Malformed {
                dir: dir.to_string(),
                message: format!("[check.files] names {extra}, which is not a plain file name"),
            });
        }
    }
    for knob in &file.option {
        if !knob.values.is_empty() && !knob.values.contains(&knob.default) {
            return Err(Error::Malformed {
                dir: dir.to_string(),
                message: format!(
                    "[[option]] {} defaults to {}, which is not one of its values",
                    knob.name,
                    word(&knob.default)
                ),
            });
        }
    }
    for (key, value) in &file.options {
        let Some(knob) = file.option.iter().find(|knob| &knob.name == key) else {
            return Err(Error::Malformed {
                dir: dir.to_string(),
                message: format!("[options] {key} has no [[option]] block saying what it changes"),
            });
        };
        if !knob.values.is_empty() && !knob.values.contains(value) {
            let allowed: Vec<String> = knob.values.iter().map(toml::Value::to_string).collect();
            return Err(Error::Malformed {
                dir: dir.to_string(),
                message: format!(
                    "[options] {key} = {value} is not one of {}",
                    allowed.join(", ")
                ),
            });
        }
    }
    for suggest in &file.suggest {
        if !file.options.contains_key(&suggest.option) {
            return Err(Error::Malformed {
                dir: dir.to_string(),
                message: format!(
                    "[[suggest]] names {}, which [options] has no default for",
                    suggest.option
                ),
            });
        }
        // `contains` is read inside the files, so a rule with none never matches.
        for rule in &suggest.rule {
            if rule.files.is_empty() {
                return Err(Error::Malformed {
                    dir: dir.to_string(),
                    message: format!(
                        "a [[suggest.rule]] for {} names no files to look in",
                        suggest.option
                    ),
                });
            }
        }
    }
    for layout in &file.layout {
        if !layout.when.contains("/*/") {
            return Err(Error::Malformed {
                dir: dir.to_string(),
                message: format!("[[layout]] when is {}, with no /*/ in it", layout.when),
            });
        }
    }

    Ok(Target {
        name: file.name,
        output: file.output,
        output_source,
        detect: file.detect,
        types: file.types,
        options: file.options,
        knobs: file.option,
        suggest: file.suggest,
        layout: file.layout,
        check: file.check,
        import: file.import,
        paths_from: file.paths_from,
        template: template.to_string(),
        source,
        dir: dir.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TYPES: &str = r#"
[types]
string = "string"
number = "number"
integer = "number"
boolean = "boolean"
url = "string"
email = "string"
port = "number"
enum = "values | join(' | ')"
"#;

    const KEY_CASE: &str = r#"
[[option]]
name = "key_case"
default = "upper"
values = ["upper", "camel"]
about = "Property names in the exported object."
"#;

    const RUNTIME: &str = r#"
[[option]]
name = "runtime"
default = "node"
values = ["node", "vite", "deno"]
about = "Where the values are read from at run time."
"#;

    const PYDANTIC: &str = r#"
[[option]]
name = "pydantic"
default = false
values = [false, true]
about = "Pydantic types for urls and secrets."
"#;

    fn config(head: &str) -> toml::Table {
        toml::from_str(&format!("{head}{TYPES}")).expect("the fixture parses")
    }

    fn bare(text: &str) -> toml::Table {
        toml::from_str(text).expect("the fixture parses")
    }

    fn read(name: &str, head: &str) -> Result<Target, Error> {
        parse(
            name,
            &config(head),
            "hello",
            Source::Repo,
            Source::Repo,
            ".penv/targets/ts",
        )
    }

    #[test]
    fn a_folder_of_toml_and_a_template_is_a_target() {
        let target = read(
            "ts",
            "name = \"ts\"\noutput = \"src/env.ts\"\ndetect = [\"package.json\", \"tsconfig.json\"]\n",
        )
        .unwrap();
        assert_eq!(target.output, "src/env.ts");
        assert_eq!(target.detect, ["package.json", "tsconfig.json"]);
        assert_eq!(target.types["port"], "number");
        assert_eq!(target.source, Source::Repo);
    }

    #[test]
    fn the_folder_name_and_the_declared_name_have_to_agree() {
        let error = read("py", "name = \"ts\"\noutput = \"src/env.ts\"\n").unwrap_err();
        assert!(error.to_string().contains("names ts"));
    }

    #[test]
    fn a_missing_base_type_is_refused_before_anything_renders() {
        let error = parse(
            "ts",
            &bare("name = \"ts\"\noutput = \"src/env.ts\"\n[types]\nstring = \"string\"\n"),
            "",
            Source::BuiltIn,
            Source::BuiltIn,
            "ts",
        )
        .unwrap_err();
        assert!(error.to_string().contains("no entry for"));
    }

    #[test]
    fn the_integer_entry_is_optional_because_the_schema_has_no_integer_type() {
        let mut config = config("name = \"go\"\noutput = \"env.go\"\n");
        config
            .get_mut("types")
            .and_then(toml::Value::as_table_mut)
            .expect("a types table")
            .remove(INT_TYPE);
        let target = parse(
            "go",
            &config,
            "",
            Source::Repo,
            Source::Repo,
            ".penv/targets/go",
        )
        .unwrap();
        assert!(!target.types.contains_key(INT_TYPE));
    }

    #[test]
    fn the_check_command_is_data_the_folder_carries() {
        let target = read(
            "ts",
            "name = \"ts\"\noutput = \"src/env.ts\"\n[check]\ncommand = [\"tsc\", \"{file}\"]\nprobe = [\"tsc\", \"--version\"]\nbin = [\"node_modules/.bin\"]\n",
        )
        .unwrap();
        let check = target.check.unwrap();
        assert_eq!(check.command, ["tsc", "{file}"]);
        assert_eq!(check.probe, ["tsc", "--version"]);
        assert_eq!(check.bin, ["node_modules/.bin"]);
    }

    #[test]
    fn the_options_table_is_whatever_the_folder_puts_there() {
        let target = read(
            "ts",
            &format!("name = \"ts\"\noutput = \"src/env.ts\"\n[options]\nkey_case = \"camel\"\n{KEY_CASE}"),
        )
        .unwrap();
        assert_eq!(target.options["key_case"].as_str(), Some("camel"));
    }

    #[test]
    fn every_knob_says_what_it_changes_and_what_it_takes() {
        let target = read(
            "ts",
            &format!("name = \"ts\"\noutput = \"src/env.ts\"\n[options]\nkey_case = \"camel\"\n{KEY_CASE}"),
        )
        .unwrap();
        let knob = &target.knobs[0];
        assert_eq!(knob.words(), ["upper", "camel"]);
        assert!(knob.about.starts_with("Property names"));
        assert_eq!(
            target.effective()[0].1.as_str(),
            Some("camel"),
            "[options] holds the value in effect, [[option]] the default"
        );
    }

    #[test]
    fn a_knob_with_no_value_of_its_own_reads_its_default() {
        let target = read("ts", &format!("name = \"ts\"\noutput = \"a\"\n{KEY_CASE}")).unwrap();
        assert_eq!(target.effective()[0].1.as_str(), Some("upper"));
    }

    #[test]
    fn a_knob_whose_default_is_not_one_of_its_values_is_a_broken_folder() {
        let error = read(
            "ts",
            "name = \"ts\"\noutput = \"a\"\n[[option]]\nname = \"key_case\"\ndefault = \"snake\"\nvalues = [\"upper\", \"camel\"]\nabout = \"Property names.\"\n",
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("not one of its values"),
            "{error}"
        );
    }

    #[test]
    fn an_option_nothing_describes_is_a_knob_nobody_can_find() {
        let error = read(
            "ts",
            "name = \"ts\"\noutput = \"a\"\n[options]\nkey_case = \"camel\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("no [[option]] block"), "{error}");
    }

    #[test]
    fn a_knob_with_no_values_listed_takes_free_text() {
        let target = read(
            "ts",
            "name = \"ts\"\noutput = \"a\"\n[options]\nheader = \"// generated\"\n[[option]]\nname = \"header\"\ndefault = \"\"\nabout = \"The line written above the file.\"\n",
        )
        .unwrap();
        assert!(target.knobs[0].words().is_empty());
    }

    #[test]
    fn a_rule_carries_the_value_it_sets_and_the_prompt_reads_the_default_first() {
        let target = read(
            "ts",
            &format!("name = \"ts\"\noutput = \"a\"\n[options]\npydantic = false\n{PYDANTIC}[[suggest]]\noption = \"pydantic\"\nprompt = \"use pydantic types?\"\n[[suggest.rule]]\nvalue = true\nfiles = [\"pyproject.toml\"]\ncontains = \"pydantic\"\n"),
        )
        .unwrap();
        let suggest = &target.suggest[0];
        assert_eq!(suggest.rule[0].value, toml::Value::Boolean(true));
        assert_eq!(suggest.rule[0].contains.as_deref(), Some("pydantic"));
        assert_eq!(
            suggest.offered(target.options.get("pydantic")),
            [toml::Value::Boolean(false), toml::Value::Boolean(true)]
        );
    }

    #[test]
    fn a_suggestion_with_no_default_in_options_is_a_broken_folder() {
        let error = read(
            "ts",
            "name = \"ts\"\noutput = \"a\"\n[[suggest]]\noption = \"runtime\"\nprompt = \"which?\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("no default"), "{error}");
    }

    #[test]
    fn a_rule_sets_any_value_it_likes_because_there_is_no_list_to_be_in() {
        let target = read(
            "ts",
            &format!("name = \"ts\"\noutput = \"a\"\n[options]\nruntime = \"node\"\n{RUNTIME}[[suggest]]\noption = \"runtime\"\nprompt = \"which?\"\n[[suggest.rule]]\nvalue = \"bun\"\nfiles = [\"bunfig.toml\"]\n"),
        )
        .unwrap();
        assert_eq!(
            target.suggest[0].offered(target.options.get("runtime")),
            [
                toml::Value::String("node".into()),
                toml::Value::String("bun".into())
            ]
        );
    }

    #[test]
    fn a_rule_that_names_nothing_to_look_at_is_a_broken_folder() {
        let error = read(
            "ts",
            &format!("name = \"ts\"\noutput = \"a\"\n[options]\nruntime = \"node\"\n{RUNTIME}[[suggest]]\noption = \"runtime\"\nprompt = \"which?\"\n[[suggest.rule]]\nvalue = \"vite\"\n"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("names no files"), "{error}");
    }

    #[test]
    fn a_rule_that_reads_inside_no_file_is_a_broken_folder() {
        let error = read(
            "ts",
            &format!("name = \"ts\"\noutput = \"a\"\n[options]\nruntime = \"node\"\n{RUNTIME}[[suggest]]\noption = \"runtime\"\nprompt = \"which?\"\n[[suggest.rule]]\nvalue = \"vite\"\ncontains = \"vite\"\n"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("names no files"), "{error}");
    }

    #[test]
    fn an_option_value_the_knob_does_not_take_is_refused_by_name() {
        let error = read(
            "ts",
            &format!("name = \"ts\"\noutput = \"a\"\n[options]\nruntime = \"bun\"\n{RUNTIME}"),
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("runtime = \"bun\""), "{message}");
        assert!(
            message.contains("\"node\", \"vite\", \"deno\""),
            "{message}"
        );

        let error = read(
            "py",
            &format!("name = \"py\"\noutput = \"a\"\n[options]\npydantic = \"false\"\n{PYDANTIC}"),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("pydantic = \"false\" is not one of false, true"),
            "a string is not the boolean it spells: {error}"
        );

        let free = read(
            "ts",
            "name = \"ts\"\noutput = \"a\"\n[options]\nheader = \"anything\"\n[[option]]\nname = \"header\"\ndefault = \"\"\nabout = \"Free text.\"\n",
        );
        assert!(free.is_ok(), "a knob with no values takes any value");
    }

    #[test]
    fn a_check_file_is_a_plain_name_beside_the_output() {
        for name in [
            "../../home/u/.bashrc",
            "/etc/passwd",
            "a/b.ts",
            "..",
            "C:x",
            "",
        ] {
            let error = read(
                "ts",
                &format!("name = \"ts\"\noutput = \"a\"\n[check]\ncommand = [\"tsc\"]\n[check.files]\n{} = \"x\"\n", toml::Value::String(name.into())),
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("[check.files]"),
                "{name}: {error}"
            );
        }
        assert!(
            read(
                "ts",
                "name = \"ts\"\noutput = \"a\"\n[check]\ncommand = [\"tsc\"]\n[check.files]\n\"globals.d.ts\" = \"x\"\n",
            )
            .is_ok()
        );
    }

    #[test]
    fn a_layout_pattern_with_no_directory_to_stand_for_is_a_broken_folder() {
        let error = read(
            "ts",
            "name = \"ts\"\noutput = \"a\"\n[[layout]]\nwhen = \"src/index.ts\"\noutput = \"src/env.ts\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("/*/"), "{error}");

        let target = read(
            "ts",
            "name = \"ts\"\noutput = \"a\"\n[[layout]]\nwhen = \"src/*/index.ts\"\noutput = \"src/*/env.ts\"\n",
        )
        .unwrap();
        assert_eq!(target.layout[0].when, "src/*/index.ts");
    }

    #[test]
    fn an_unknown_field_is_a_typo_not_an_extension() {
        let error = read("ts", "name = \"ts\"\noutput = \"a\"\ndetects = []\n").unwrap_err();
        assert!(matches!(error, Error::Malformed { .. }));
    }
}
