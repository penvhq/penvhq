//! `.penv/config.toml` beside `.env.schema`: committed, so every clone on every
//! machine reads the same settings. It holds dates and settings, never a value.

use std::path::{Path, PathBuf};

use crate::error::CliError;
use crate::files::{read_file, show, write_file_making_parents};

pub const CONFIG_FILE: &str = ".penv/config.toml";

/// A new `.penv/config.toml`: every setting with the value penv uses when it is
/// absent, and what the other value does. `{version}` is the schema version.
pub const TEMPLATE: &str = r#"# penv settings for this repository. Every setting is listed with the value
# penv uses when it is absent. Commit this file.

[schema]
# The .env.schema language version this repository is written in.
version = {version}

[run]
# Load penv's masking into the Node, Bun, Deno and Python processes penv run
# starts, so the app's own logs and responses hide secrets.
# false: only penv run's output pipe hides them; what the app writes to a file
# or sends over the network is not masked. An AI agent's run keeps it on.
preload = true

[local]
# Whether penv encrypts the sensitive values it writes to .env files.
# false: penv set, penv pull and random() write values in plain text, readable
# by any tool that reads .env itself.
# true: they write enc:v1:... with a key kept in this machine's keychain, and
# penv decrypts when it reads. A tool that reads .env itself (docker compose, a
# framework started without penv run) then sees enc:v1:... instead of the value.
# penv encrypt turns this on and converts existing values; penv decrypt turns it
# off and converts them back.
encrypt = false

[public]
# Prefixes that ship a key to the browser, beyond the frameworks' own
# (NEXT_PUBLIC_, VITE_, PUBLIC_, NUXT_PUBLIC_, EXPO_PUBLIC_, REACT_APP_, ...).
prefixes = []
"#;

/// The file as a table, so sections this build does not know survive a write,
/// and as its text, so a write keeps every comment and the order it had.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    table: toml::Table,
    text: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_dates_are_kept_and_other_sections_survive() {
        let mut config = Config::parse("[future]\nsetting = true\n").unwrap();
        config.set_rotated("STRIPE_SECRET_KEY", "2026-09-22");
        let back = Config::parse(&config.render()).unwrap();
        assert_eq!(back.rotated("STRIPE_SECRET_KEY"), Some("2026-09-22"));
        assert!(back.render().contains("[future]"));
    }
}
