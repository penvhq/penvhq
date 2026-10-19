//! `.penv/config.toml` beside `.env.schema`: committed, so every clone on every
//! machine reads the same settings. It holds dates and settings, never a value.

use std::path::{Path, PathBuf};

use penv_schema::editor::vocabulary;
use penv_schema::functions::Policy;

use crate::error::CliError;
use crate::files::{read_file, show, write_file_making_parents};

pub const CONFIG_FILE: &str = ".penv/config.toml";

/// A new `.penv/config.toml`: every setting with the value penv uses when it is
/// absent, and what the other value does. `{version}` is the schema version.
pub const TEMPLATE: &str = r#"# penv settings for this repository. Every setting is listed with the value
# penv uses when it is absent. Commit this file.

[schema]
# The .env.schema language version this repository is written in.
# Values: {version}.
version = {version}

[run]
# Load penv's masking into the Node, Bun, Deno and Python processes penv run starts.
# Values: true (the app's own logs and responses are masked) | false (only penv run's output is).
preload = true

[local]
# How penv set, penv pull and random() write sensitive values to .env files.
# Values: false (plain text) | true (enc:v1:..., keyed in this machine's keychain; see penv encrypt).
encrypt = false

[public]
# Prefixes that ship a key to the browser, beyond NEXT_PUBLIC_, VITE_, PUBLIC_ and the like.
# Values: a list of prefixes, e.g. ["APP_PUBLIC_"]; empty adds none.
prefixes = []

[url]
# URL parts a url() read shows unmasked though the URL is sensitive. password never is.
# Values: scheme, host, port, origin, path, user and query.NAME, e.g. "query.sslmode".
public = ["scheme", "host", "port", "origin", "path"]

[hash]
# Keys whose hash() result is shown unmasked. Each must be a key whose value is a hash() call.
# Values: a list of keys, e.g. ["API_KEY_FP"]; empty adds none.
public = []

# One section per key whose json() reads may be shown unmasked:
# [json.GCP_SA_JSON]
# public = ["client_email", "project_id"]
"#;

/// The file as a table, so sections this build does not know survive a write,
/// and as its text, so a write keeps every comment and the order it had.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    table: toml::Table,
    text: Option<String>,
    /// Comments for `[targets.<name>.options]` keys: (target, option, comment).
    notes: Vec<(String, String, String)>,
}

pub fn template() -> String {
    TEMPLATE.replace("{version}", &penv_schema::SCHEMA_VERSION.to_string())
}

/// `text` with every section and setting the template has and it lacks, each
/// with the template's comments. What is already there is left as it is.
pub fn with_defaults(text: &str) -> String {
    let (Ok(mut doc), Ok(template)) = (
        text.parse::<toml_edit::DocumentMut>(),
        template().parse::<toml_edit::DocumentMut>(),
    ) else {
        return text.to_string();
    };
    for (name, item) in template.iter() {
        match doc.get_mut(name) {
            None => {
                doc.insert(name, item.clone());
            }
            Some(existing) => {
                let (Some(have), Some(want)) = (existing.as_table_mut(), item.as_table()) else {
                    continue;
                };
                for (key, value) in want.iter() {
                    if !have.contains_key(key)
                        && let Some((formatted, _)) = want.get_key_value(key)
                    {
                        have.insert_formatted(formatted, value.clone());
                    }
                }
            }
        }
    }
    doc.to_string()
}

/// Write every value `table` holds into `doc`, keeping what `doc` already says
/// about the rest: comments, order, and settings the table does not name.
fn upsert(doc: &mut toml_edit::Table, table: &toml::Table) {
    for (key, value) in table {
        match value {
            toml::Value::Table(inner) => {
                let entry = doc
                    .entry(key)
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                if !entry.is_table() {
                    *entry = toml_edit::Item::Table(toml_edit::Table::new());
                }
                if let Some(t) = entry.as_table_mut() {
                    upsert(t, inner);
                }
            }
            other => {
                let rendered = other.to_string();
                let same = doc
                    .get(key)
                    .and_then(|i| i.as_value())
                    .is_some_and(|v| v.to_string().trim() == rendered.trim());
                if !same && let Ok(v) = rendered.parse::<toml_edit::Value>() {
                    match doc.get_mut(key).and_then(|i| i.as_value_mut()) {
                        Some(slot) => {
                            let decor = slot.decor().clone();
                            *slot = v;
                            *slot.decor_mut() = decor;
                        }
                        None => {
                            doc.insert(key, toml_edit::Item::Value(v));
                        }
                    }
                }
            }
        }
    }
}

impl Config {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(CONFIG_FILE)
    }

    pub fn load(dir: &Path) -> Result<Config, CliError> {
        let path = Config::path(dir);
        if !path.is_file() {
            return Ok(Config::default());
        }
        Config::parse(&read_file(&path)?).map_err(|e| {
            CliError::new(
                "invalid_config",
                format!("{} is not valid TOML: {e}", show(&path)),
                "Fix the line it names, or delete the file to start it again.",
            )
        })
    }

    pub fn parse(text: &str) -> Result<Config, toml::de::Error> {
        Ok(Config {
            table: text.parse()?,
            text: Some(text.to_string()),
            notes: Vec::new(),
        })
    }

    pub fn save(&self, dir: &Path) -> Result<PathBuf, CliError> {
        let path = Config::path(dir);
        write_file_making_parents(&path, &self.render())?;
        Ok(path)
    }

    /// The file to write: the text it was read from (a new file starts from the
    /// template), with every value this config holds written in.
    pub fn render(&self) -> String {
        let base = self.text.clone().unwrap_or_else(template);
        match base.parse::<toml_edit::DocumentMut>() {
            Ok(mut doc) => {
                upsert(doc.as_table_mut(), &self.table);
                self.annotate(&mut doc);
                doc.to_string()
            }
            Err(_) => toml::to_string(&self.table).unwrap_or_default(),
        }
    }

    /// Add every setting the template lists and this file lacks, with its
    /// comment, so the file names everything penv reads.
    pub fn fill_defaults(&mut self) {
        let base = self.text.clone().unwrap_or_else(template);
        let filled = with_defaults(&base);
        if let Ok(table) = filled.parse() {
            self.table = table;
        }
        self.text = Some(filled);
    }

    /// `[schema] version`: the schema language version. It lives here, not in
    /// `.env.schema`, because varlock rejects `@schema`.
    pub fn schema_version(&self) -> Option<i64> {
        self.table.get("schema")?.get("version")?.as_integer()
    }

    /// True when `[schema] version` is written at all, whatever its type.
    pub fn has_schema_version(&self) -> bool {
        self.table
            .get("schema")
            .and_then(|s| s.get("version"))
            .is_some()
    }

    pub fn set_schema_version(&mut self, version: i64) {
        let section = self
            .table
            .entry("schema")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !section.is_table() {
            *section = toml::Value::Table(toml::Table::new());
        }
        if let Some(table) = section.as_table_mut() {
            table.insert("version".into(), toml::Value::Integer(version));
        }
    }

    /// `[public] prefixes`: prefixes beyond the frameworks' own that ship a key to
    /// the browser, for a custom Vite `envPrefix` and the like.
    pub fn public_prefixes(&self) -> Vec<String> {
        self.table
            .get("public")
            .and_then(|t| t.get("prefixes"))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `[run] preload`: false keeps penv's masking out of the child's runtime.
    /// Absent means true.
    pub fn preload(&self) -> bool {
        self.table
            .get("run")
            .and_then(|t| t.get("preload"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    }

    /// `[local] encrypt`: sensitive values penv writes to .env files are
    /// encrypted. Absent means false.
    pub fn encrypt(&self) -> bool {
        self.table
            .get("local")
            .and_then(|t| t.get("encrypt"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    pub fn set_encrypt(&mut self, on: bool) {
        let section = self
            .table
            .entry("local")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !section.is_table() {
            *section = toml::Value::Table(toml::Table::new());
        }
        if let Some(table) = section.as_table_mut() {
            table.insert("encrypt".into(), toml::Value::Boolean(on));
        }
    }

    /// `[providers.<slug>] url`: where that provider's API is, when not its default.
    pub fn provider_url(&self, slug: &str) -> Option<String> {
        self.table
            .get("providers")?
            .get(slug)?
            .get("url")?
            .as_str()
            .map(str::to_string)
    }

    /// `[targets.<name>]`: where `penv gen` writes and the options it uses.
    pub fn target(&self, name: &str) -> Option<toml::Table> {
        self.table.get("targets")?.get(name)?.as_table().cloned()
    }

    pub fn set_target(&mut self, name: &str, section: toml::Table) {
        let targets = self
            .table
            .entry("targets")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !targets.is_table() {
            *targets = toml::Value::Table(toml::Table::new());
        }
        if let Some(table) = targets.as_table_mut() {
            table.insert(name.to_string(), toml::Value::Table(section));
        }
    }

    /// Say above each `[targets.<name>.options]` key what it does. A key that
    /// already carries a comment keeps it.
    pub fn note_target_option(&mut self, target: &str, option: &str, comment: &str) {
        self.notes
            .push((target.to_string(), option.to_string(), comment.to_string()));
    }

    fn annotate(&self, doc: &mut toml_edit::DocumentMut) {
        for (target, option, comment) in &self.notes {
            let Some(options) = doc
                .get_mut("targets")
                .and_then(|t| t.get_mut(target.as_str()))
                .and_then(|t| t.get_mut("options"))
                .and_then(|t| t.as_table_mut())
            else {
                continue;
            };
            let Some(mut key) = options.key_mut(option) else {
                continue;
            };
            let decor = key.leaf_decor_mut();
            let has_comment = decor
                .prefix()
                .and_then(|p| p.as_str())
                .is_some_and(|p| p.contains('#'));
            if !has_comment {
                let lines: String = comment.lines().map(|l| format!("# {l}\n")).collect();
                decor.set_prefix(lines);
            }
        }
    }

    /// `[url] public`, `[json.KEY] public` and `[hash] public`. An absent list is the
    /// default; a list or entry `check` refuses declassifies nothing.
    pub fn policy(&self) -> Policy {
        let mut policy = Policy::default();
        if let Some(list) = self.table.get("url").and_then(|t| t.get("public")) {
            policy.url_public = strings(list).filter(|p| url_part(p)).collect();
        }
        if let Some(keys) = self.table.get("json").and_then(|t| t.as_table()) {
            for (key, section) in keys {
                if let Some(list) = section.get("public") {
                    policy
                        .json_public
                        .insert(key.clone(), strings(list).collect());
                }
            }
        }
        if let Some(list) = self.table.get("hash").and_then(|t| t.get("public")) {
            policy.hash_public = strings(list).collect();
        }
        policy
    }

    /// What `check` refuses in the `public` lists that needs no schema to see,
    /// each naming the section and the entry.
    pub fn policy_problems(&self) -> Vec<String> {
        let mut found = Vec::new();
        match self.table.get("url") {
            None => {}
            Some(toml::Value::Table(url)) => {
                if let Some(list) = url.get("public") {
                    found.extend(list_problems("[url] public", list, "URL parts"));
                    for (i, part) in list.as_array().into_iter().flatten().enumerate() {
                        let Some(part) = part.as_str() else { continue };
                        if is_sensitive_part(part) {
                            found.push(format!(
                                "[url] public lists {part}, which is always sensitive; remove it"
                            ));
                        } else if !url_part(part) {
                            found.push(format!(
                                "[url] public lists {}, which is not a URL part; use scheme, host, port, origin, path, user or query.NAME",
                                label(part, Some(i))
                            ));
                        }
                    }
                }
            }
            Some(_) => found.push("url is not a section; write [url] public = [...]".into()),
        }
        match self.table.get("json") {
            None => {}
            Some(toml::Value::Table(keys)) => {
                for (key, section) in keys {
                    let name = format!("[json.{}] public", label(key, None));
                    match section.get("public") {
                        Some(list) => found.extend(list_problems(&name, list, "read paths")),
                        None => found.push(format!(
                            "{name} is missing; write a list of read paths, such as [\"client_email\"]"
                        )),
                    }
                }
            }
            Some(_) => found.push(
                "json is not a section; write one per key, such as [json.GCP_SA_JSON]".into(),
            ),
        }
        match self.table.get("hash") {
            None => {}
            Some(toml::Value::Table(hash)) => {
                if let Some(list) = hash.get("public") {
                    found.extend(list_problems("[hash] public", list, "keys"));
                }
            }
            Some(_) => found.push("hash is not a section; write [hash] public = [...]".into()),
        }
        found
    }

    /// `[rotation]`: the day each key was last written in local mode.
    pub fn rotated(&self, key: &str) -> Option<&str> {
        self.table.get("rotation")?.get(key)?.as_str()
    }

    pub fn set_rotated(&mut self, key: &str, day: &str) {
        let section = self
            .table
            .entry("rotation")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !section.is_table() {
            *section = toml::Value::Table(toml::Table::new());
        }
        if let Some(table) = section.as_table_mut() {
            table.insert(key.to_string(), toml::Value::String(day.to_string()));
        }
    }
}

fn strings(list: &toml::Value) -> impl Iterator<Item = String> + '_ {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
}

/// A part `url()` reads that a `public` list may name: never `password`.
fn url_part(part: &str) -> bool {
    vocabulary().url_parts.iter().any(|p| p.names(part)) && !is_sensitive_part(part)
}

fn is_sensitive_part(part: &str) -> bool {
    vocabulary()
        .url_parts
        .iter()
        .any(|p| p.sensitive && p.names(part))
}

fn list_problems(name: &str, list: &toml::Value, what: &str) -> Vec<String> {
    let Some(items) = list.as_array() else {
        return vec![format!("{name} must be a list of {what}")];
    };
    items
        .iter()
        .enumerate()
        .filter(|(_, v)| !v.is_str())
        .map(|(i, _)| format!("{name} entry {} is not a string; quote it", i + 1))
        .collect()
}

/// A config entry as a message shows it: by its text when that reads as a name,
/// otherwise by position, so a value pasted into a list is never echoed.
pub fn label(entry: &str, index: Option<usize>) -> String {
    let named = !entry.is_empty()
        && entry.len() <= 64
        && entry
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-[]".contains(c));
    match (named, index) {
        (true, _) => entry.to_string(),
        (false, Some(i)) => format!("entry {}", i + 1),
        (false, None) => "an entry that is not a name".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_policy_is_the_default_and_a_refused_entry_declassifies_nothing() {
        assert_eq!(Config::default().policy(), Policy::default());
        let config = Config::parse(
            "[url]\npublic = [\"host\", \"user\", \"password\", \"query.sslmode\", \"hots\", 3]\n\n[json.SA]\npublic = [\"client_email\"]\n\n[hash]\npublic = [\"FP\"]\n",
        )
        .unwrap();
        let policy = config.policy();
        let url: Vec<&str> = policy.url_public.iter().map(String::as_str).collect();
        assert_eq!(url, ["host", "query.sslmode", "user"]);
        assert!(policy.json_public["SA"].contains("client_email"));
        assert!(policy.hash_public.contains("FP"));
        let problems = config.policy_problems().join("\n");
        assert!(
            problems.contains("[url] public lists password"),
            "{problems}"
        );
        assert!(problems.contains("[url] public lists hots"), "{problems}");
        assert!(
            problems.contains("[url] public entry 6 is not a string"),
            "{problems}"
        );
    }

    #[test]
    fn a_public_list_that_is_not_a_list_is_empty_and_refused() {
        let config = Config::parse("[url]\npublic = \"user\"\n").unwrap();
        assert!(config.policy().url_public.is_empty());
        assert_eq!(
            config.policy_problems(),
            ["[url] public must be a list of URL parts"]
        );
        let pasted = Config::parse("[url]\npublic = [\"postgres://u:pw@db.test/x\"]\n").unwrap();
        let problems = pasted.policy_problems().join("\n");
        assert!(problems.contains("entry 1"), "{problems}");
        assert!(!problems.contains("pw@"), "{problems}");
    }

    #[test]
    fn the_template_reads_as_the_default_policy() {
        let config = Config::parse(&template()).unwrap();
        assert_eq!(config.policy(), Policy::default());
        assert!(config.policy_problems().is_empty());
    }

    #[test]
    fn rotation_dates_are_kept_and_other_sections_survive() {
        let mut config = Config::parse("[future]\nsetting = true\n").unwrap();
        config.set_rotated("STRIPE_SECRET_KEY", "2026-09-22");
        let back = Config::parse(&config.render()).unwrap();
        assert_eq!(back.rotated("STRIPE_SECRET_KEY"), Some("2026-09-22"));
        assert!(back.render().contains("[future]"));
    }

    #[test]
    fn a_target_option_is_commented_with_its_values_and_a_written_comment_stays() {
        let mut config =
            Config::parse("[targets.ts.options]\n# mine\nmask = false\nruntime = \"node\"\n")
                .unwrap();
        config.note_target_option("ts", "mask", "Mask secrets.\nValues: true | false.");
        config.note_target_option(
            "ts",
            "runtime",
            "Where values are read.\nValues: node | deno.",
        );
        let text = config.render();
        assert!(text.contains("# mine\nmask = false"), "{text}");
        assert!(!text.contains("# Mask secrets."), "{text}");
        assert!(
            text.contains("# Where values are read.\n# Values: node | deno.\nruntime = \"node\""),
            "{text}"
        );
    }
}
