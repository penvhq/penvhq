//! Targets configured in one place. `.penv/config.toml` holds a `[targets.<name>]`
//! section for each target a repository uses, with the same fields a
//! `target.toml` has, and `.penv/<name>.tmpl` holds a template override. This
//! presents both to the folder lookup as the repository's own target folder, so
//! the loader and its inheritance stay as they are. A `.penv/targets/<name>/`
//! folder left from before still reads, below the configuration.

use crate::error::Error;
use crate::folder::Tree;
use crate::load::merge;
use crate::target::is_name as named;

pub const CONFIG: &str = ".penv/config.toml";

pub struct Settled<'a> {
    inner: &'a dyn Tree,
    folder: String,
    penv: String,
    sections: toml::Table,
}

impl<'a> Settled<'a> {
    /// `repo` is the directory holding `.env.schema` and `.penv/`. A config
    /// file that does not parse is named here, before anything reads through it.
    pub fn new(inner: &'a dyn Tree, repo: &str) -> Result<Settled<'a>, Error> {
        let penv = format!("{repo}/.penv");
        let path = format!("{repo}/{CONFIG}");
        let table = match inner.read(&path) {
            Some(text) => text.parse::<toml::Table>().map_err(|e| Error::Malformed {
                dir: path.clone(),
                message: e.message().to_string(),
            })?,
            None => toml::Table::new(),
        };
        let sections = table
            .get("targets")
            .and_then(|t| t.as_table())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|(name, _)| named(name))
            .collect();
        Ok(Settled {
            inner,
            folder: format!("{penv}/targets"),
            penv,
            sections,
        })
    }

    /// The `target.toml` a section stands for, `name` filled in, read over the
    /// old folder's file when there is one. A template override with no section
    /// stands for a target that changes nothing else.
    fn section(&self, name: &str, path: &str) -> Option<String> {
        let legacy = self.inner.read(path);
        let mut table = match self.sections.get(name).and_then(|v| v.as_table()) {
            Some(table) => table.clone(),
            None if self.inner.exists(&format!("{}/{name}.tmpl", self.penv)) => toml::Table::new(),
            None => return legacy,
        };
        if let Some(text) = legacy {
            // A broken old file is served as it is, so the loader names it.
            let Ok(mut below) = text.parse::<toml::Table>() else {
                return Some(text);
            };
            merge(&mut below, table);
            table = below;
        }
        table.insert("name".into(), toml::Value::String(name.to_string()));
        toml::to_string(&table).ok()
    }

    /// `<repo>/.penv/targets/<name>/<file>` split into its name and file.
    fn mapped<'p>(&self, path: &'p str) -> Option<(&'p str, &'p str)> {
        let rest = path.strip_prefix(&self.folder)?.strip_prefix('/')?;
        rest.split_once('/')
    }
}

impl Tree for Settled<'_> {
    fn files(&self, path: &str) -> Vec<String> {
        self.inner.files(path)
    }

    fn read(&self, path: &str) -> Option<String> {
        match self.mapped(path) {
            Some((name, "target.toml")) if named(name) => self.section(name, path),
            Some((name, "env.tmpl")) if named(name) => self
                .inner
                .read(&format!("{}/{name}.tmpl", self.penv))
                .or_else(|| self.inner.read(path)),
            _ => self.inner.read(path),
        }
    }

    fn dirs(&self, path: &str) -> Vec<String> {
        let mut out = self.inner.dirs(path);
        if path == self.folder {
            for name in self.sections.keys() {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
            out.sort();
        }
        out
    }

    fn exists(&self, path: &str) -> bool {
        match self.mapped(path) {
            Some(_) => self.read(path).is_some(),
            None => self.inner.exists(path),
        }
    }

    fn origin(&self, path: &str) -> Option<String> {
        match self.mapped(path) {
            Some((name, "target.toml")) if self.sections.contains_key(name) => {
                Some(format!("{}/config.toml [targets.{name}]", self.penv))
            }
            _ => self.inner.origin(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake(BTreeMap<String, String>);

    impl Fake {
        fn with(mut self, path: &str, body: &str) -> Fake {
            self.0.insert(path.into(), body.into());
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
                .filter_map(|k| k.strip_prefix(&prefix))
                .filter_map(|rest| rest.split_once('/').map(|(d, _)| d.to_string()))
                .collect();
            out.dedup();
            out
        }
    }

    #[test]
    fn a_config_section_reads_as_the_repository_target_toml() {
        let tree = Fake::default().with(
            "/repo/.penv/config.toml",
            "[schema]\nversion = 1\n\n[targets.ts]\noutput = \"src/env.ts\"\n\n[targets.ts.options]\nruntime = \"vite\"\n",
        );
        let settled = Settled::new(&tree, "/repo").unwrap();
        let text = settled.read("/repo/.penv/targets/ts/target.toml").unwrap();
        let table: toml::Table = text.parse().unwrap();
        assert_eq!(table["name"].as_str(), Some("ts"));
        assert_eq!(table["output"].as_str(), Some("src/env.ts"));
        assert_eq!(table["options"]["runtime"].as_str(), Some("vite"));
        assert!(settled.exists("/repo/.penv/targets/ts/target.toml"));
        assert_eq!(settled.dirs("/repo/.penv/targets"), ["ts"]);
    }

    #[test]
    fn a_template_beside_config_overrides_and_the_old_folder_still_reads() {
        let tree = Fake::default()
            .with("/repo/.penv/go.tmpl", "package env")
            .with(
                "/repo/.penv/targets/py/target.toml",
                "name = \"py\"\noutput = \"a.py\"\n",
            );
        let settled = Settled::new(&tree, "/repo").unwrap();
        assert_eq!(
            settled.read("/repo/.penv/targets/go/env.tmpl").as_deref(),
            Some("package env")
        );
        assert_eq!(
            settled
                .read("/repo/.penv/targets/go/target.toml")
                .as_deref(),
            Some("name = \"go\"\n"),
            "a template alone still names its target"
        );
        assert!(
            settled.read("/repo/.penv/targets/py/target.toml").is_some(),
            "legacy folder"
        );
        assert_eq!(settled.read("/home/.penv/targets/ts/target.toml"), None);
    }

    #[test]
    fn a_section_whose_name_is_not_a_word_is_never_a_target() {
        let tree = Fake::default()
            .with(
                "/repo/.penv/config.toml",
                "[targets.\"../evil\"]\noutput = \"x\"\n",
            )
            .with("/repo/.penv/../evil.tmpl", "x");
        let settled = Settled::new(&tree, "/repo").unwrap();
        assert!(settled.dirs("/repo/.penv/targets").is_empty());
        assert_eq!(
            settled.read("/repo/.penv/targets/../evil/target.toml"),
            None
        );
    }

    #[test]
    fn the_old_folder_still_reads_below_the_section_and_below_a_lone_template() {
        let tree = Fake::default()
            .with(
                "/repo/.penv/config.toml",
                "[targets.zig]\noutput = \"new.zig\"\n[targets.zig.types]\nport = \"u16\"\n",
            )
            .with(
                "/repo/.penv/targets/zig/target.toml",
                "name = \"zig\"\noutput = \"old.zig\"\ndetect = [\"build.zig\"]\n[types]\nport = \"int\"\nstring = \"[]const u8\"\n",
            )
            .with("/repo/.penv/go.tmpl", "package env")
            .with(
                "/repo/.penv/targets/go/target.toml",
                "name = \"go\"\noutput = \"cmd/env.go\"\n",
            );
        let settled = Settled::new(&tree, "/repo").unwrap();
        let zig: toml::Table = settled
            .read("/repo/.penv/targets/zig/target.toml")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(zig["output"].as_str(), Some("new.zig"), "the section wins");
        assert_eq!(
            zig["detect"][0].as_str(),
            Some("build.zig"),
            "the folder reads below"
        );
        assert_eq!(zig["types"]["port"].as_str(), Some("u16"));
        assert_eq!(
            zig["types"]["string"].as_str(),
            Some("[]const u8"),
            "key by key"
        );

        let go: toml::Table = settled
            .read("/repo/.penv/targets/go/target.toml")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            go["output"].as_str(),
            Some("cmd/env.go"),
            "a lone template hides nothing"
        );
    }

    #[test]
    fn a_config_that_does_not_parse_is_named_rather_than_read_as_empty() {
        let tree = Fake::default().with("/repo/.penv/config.toml", "[targets.ts\n");
        let error = Settled::new(&tree, "/repo").err().expect("refused");
        assert!(
            error.to_string().contains("/repo/.penv/config.toml"),
            "{error}"
        );
    }

    #[test]
    fn a_section_is_named_as_where_its_fields_live() {
        let tree =
            Fake::default().with("/repo/.penv/config.toml", "[targets.ts]\noutput = \"a\"\n");
        let settled = Settled::new(&tree, "/repo").unwrap();
        assert_eq!(
            settled
                .origin("/repo/.penv/targets/ts/target.toml")
                .as_deref(),
            Some("/repo/.penv/config.toml [targets.ts]")
        );
        assert_eq!(settled.origin("/repo/.penv/targets/py/target.toml"), None);
    }

    #[test]
    fn the_config_section_wins_over_a_leftover_folder() {
        let tree = Fake::default()
            .with(
                "/repo/.penv/config.toml",
                "[targets.ts]\noutput = \"new.ts\"\n",
            )
            .with(
                "/repo/.penv/targets/ts/target.toml",
                "name = \"ts\"\noutput = \"old.ts\"\n",
            );
        let settled = Settled::new(&tree, "/repo").unwrap();
        let text = settled.read("/repo/.penv/targets/ts/target.toml").unwrap();
        assert!(text.contains("new.ts"), "{text}");
    }
}
