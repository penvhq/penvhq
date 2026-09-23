//! `.penv/config.toml` beside `.env.schema`: committed, so every clone on every
//! machine reads the same settings. It holds dates and settings, never a value.

use std::path::{Path, PathBuf};

use crate::error::CliError;
use crate::files::{read_file, show, write_file_making_parents};

pub const CONFIG_FILE: &str = ".penv/config.toml";

/// The file as a table, so sections this build does not know survive a write.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    table: toml::Table,
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
        })
    }

    pub fn save(&self, dir: &Path) -> Result<PathBuf, CliError> {
        let path = Config::path(dir);
        write_file_making_parents(&path, &self.render())?;
        Ok(path)
    }

    pub fn render(&self) -> String {
        toml::to_string(&self.table).unwrap_or_default()
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
