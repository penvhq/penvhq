//! The folder lookup and the template helpers that targets and guards share.
//!
//! A drop-in part is a directory under `.penv/<kind>/<name>/` in the repository,
//! under `~/.penv/<kind>/<name>/`, or shipped inside the binary. The order is the
//! same for every kind, so only the files inside a folder differ.

use minijinja::value::Value as Jinja;
use minijinja::{AutoEscape, Environment, UndefinedBehavior};

/// Reading a tree of folders, so the loader is a pure function over one.
pub trait Tree {
    fn read(&self, path: &str) -> Option<String>;
    /// Names of the directories directly under `path`.
    fn dirs(&self, path: &str) -> Vec<String>;
    fn exists(&self, path: &str) -> bool {
        self.read(path).is_some()
    }
    /// Names of the files directly under `path`, for a detect pattern such as
    /// `*.csproj`. A tree that cannot list them matches no pattern.
    fn files(&self, _path: &str) -> Vec<String> {
        Vec::new()
    }
}

/// Where folders are looked for. `repo` is the directory holding `.env.schema`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    pub repo: String,
    pub home: Option<String>,
}

impl Roots {
    pub fn new(repo: impl Into<String>, home: Option<String>) -> Roots {
        Roots {
            repo: repo.into(),
            home,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Repo,
    User,
    BuiltIn,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Repo => "repo",
            Source::User => "user",
            Source::BuiltIn => "built in",
        }
    }
}

/// A folder compiled into the binary, as the files it holds by name.
pub struct BuiltIn {
    pub name: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

impl BuiltIn {
    pub fn file(&self, name: &str) -> Option<&'static str> {
        self.files
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, body)| *body)
    }
}

/// One folder that was found, with its files still unread. A file the folder
/// does not hold is read from the next place in the lookup order, so a folder
/// that overrides one file inherits the rest.
#[derive(Debug)]
pub struct Found<'a> {
    pub source: Source,
    pub dir: String,
    readers: Vec<(Source, Reader<'a>)>,
}

type ReadFile<'a> = Box<dyn Fn(&str) -> Option<String> + 'a>;

struct Reader<'a>(ReadFile<'a>);

impl std::fmt::Debug for Reader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<files>")
    }
}

impl Found<'_> {
    pub fn file(&self, name: &str) -> Option<String> {
        self.file_from(name).map(|(_, body)| body)
    }

    /// The file and the place it came from, which is not always the folder that
    /// won the lookup.
    pub fn file_from(&self, name: &str) -> Option<(Source, String)> {
        self.readers
            .iter()
            .find_map(|(source, read)| (read.0)(name).map(|body| (*source, body)))
    }

    /// Every copy of one file down the lookup order, the winning folder first.
    pub fn files(&self, name: &str) -> Vec<(Source, String)> {
        self.readers
            .iter()
            .filter_map(|(source, read)| (read.0)(name).map(|body| (*source, body)))
            .collect()
    }
}

/// The folders on disk a lookup reads, in order.
pub fn places(roots: &Roots, kind: &str, name: &str) -> Vec<(Source, String)> {
    let mut out = vec![(Source::Repo, format!("{}/.penv/{kind}/{name}", roots.repo))];
    if let Some(home) = &roots.home {
        out.push((Source::User, format!("{home}/.penv/{kind}/{name}")));
    }
    out
}

/// Repo folder, then home folder, then built in. The first one holding `entry`
/// wins, and every place after it stays readable for the files it does not hold;
/// the error is the list of places that were looked in.
pub fn find<'a>(
    tree: &'a dyn Tree,
    roots: &Roots,
    kind: &str,
    entry: &str,
    built_in: &'static [BuiltIn],
    name: &str,
) -> Result<Found<'a>, Vec<String>> {
    let places = places(roots, kind, name);
    let compiled = built_in.iter().find(|b| b.name == name);
    let won = places
        .iter()
        .position(|(_, dir)| tree.read(&format!("{dir}/{entry}")).is_some());

    let (source, dir, from) = match (won, compiled) {
        (Some(index), _) => (places[index].0, places[index].1.clone(), index),
        (None, Some(_)) => (Source::BuiltIn, "built in".to_string(), places.len()),
        (None, None) => {
            let mut looked: Vec<String> = places.into_iter().map(|(_, dir)| dir).collect();
            looked.push("built in".into());
            return Err(looked);
        }
    };

    let mut readers: Vec<(Source, Reader<'a>)> = places
        .into_iter()
        .skip(from)
        .map(|(source, dir)| {
            let reader: ReadFile<'a> = Box::new(move |file| tree.read(&format!("{dir}/{file}")));
            (source, Reader(reader))
        })
        .collect();
    if let Some(b) = compiled {
        readers.push((
            Source::BuiltIn,
            Reader(Box::new(move |file| b.file(file).map(str::to_string))),
        ));
    }
    Ok(Found {
        source,
        dir,
        readers,
    })
}

/// The built-in names in their own order, then whatever the two `.penv` folders
/// add.
pub fn names(
    tree: &dyn Tree,
    roots: &Roots,
    kind: &str,
    built_in: &'static [BuiltIn],
) -> Vec<String> {
    let mut names: Vec<String> = built_in.iter().map(|b| b.name.to_string()).collect();
    let mut roots_dirs = vec![format!("{}/.penv/{kind}", roots.repo)];
    if let Some(home) = &roots.home {
        roots_dirs.push(format!("{home}/.penv/{kind}"));
    }
    for dir in roots_dirs {
        for name in tree.dirs(&dir) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// The template engine every folder renders through: a typo is an error, and
/// nothing is HTML.
pub fn environment() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_auto_escape_callback(|_| AutoEscape::None);
    env.add_filter("quote", quote);
    env.add_filter("json", to_json);
    env
}

pub fn quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
}

pub fn to_json(value: Jinja) -> Result<String, minijinja::Error> {
    serde_json::to_string(&value).map_err(|e| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("value cannot be written as JSON: {e}"),
        )
    })
}

/// A minijinja failure with the detail line it hides behind `detail()`.
pub fn describe(error: &minijinja::Error) -> String {
    match error.detail() {
        Some(detail) => format!("{error}: {detail}"),
        None => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
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
    }

    const BUILT_IN: &[BuiltIn] = &[BuiltIn {
        name: "ts",
        files: &[("target.toml", "built in config"), ("env.tmpl", "built in")],
    }];

    fn roots() -> Roots {
        Roots::new("/repo", Some("/home".into()))
    }

    #[test]
    fn the_repo_folder_beats_the_home_folder_and_the_built_in_one() {
        let tree = Fake::default()
            .with("/repo/.penv/targets/ts/target.toml", "repo config")
            .with("/repo/.penv/targets/ts/env.tmpl", "repo")
            .with("/home/.penv/targets/ts/target.toml", "home config");
        let found = find(&tree, &roots(), "targets", "target.toml", BUILT_IN, "ts").unwrap();
        assert_eq!(found.source, Source::Repo);
        assert_eq!(found.file("env.tmpl").as_deref(), Some("repo"));
    }

    #[test]
    fn a_file_the_winning_folder_lacks_comes_from_the_next_place() {
        let tree = Fake::default().with("/repo/.penv/targets/ts/target.toml", "repo config");
        let found = find(&tree, &roots(), "targets", "target.toml", BUILT_IN, "ts").unwrap();
        assert_eq!(found.source, Source::Repo);
        assert_eq!(
            found.file_from("env.tmpl"),
            Some((Source::BuiltIn, "built in".to_string()))
        );
    }

    #[test]
    fn the_built_in_folder_is_the_last_place_looked() {
        let tree = Fake::default();
        let found = find(&tree, &roots(), "targets", "target.toml", BUILT_IN, "ts").unwrap();
        assert_eq!(found.source, Source::BuiltIn);
        assert_eq!(found.file("env.tmpl").as_deref(), Some("built in"));
    }

    #[test]
    fn an_unknown_name_reports_every_place_it_looked() {
        let tree = Fake::default();
        let looked = find(&tree, &roots(), "guards", "guard.toml", BUILT_IN, "nano").unwrap_err();
        assert_eq!(
            looked,
            [
                "/repo/.penv/guards/nano",
                "/home/.penv/guards/nano",
                "built in"
            ]
        );
    }

    #[test]
    fn the_names_are_the_built_in_ones_plus_the_folders() {
        let tree = Fake::default().with("/home/.penv/targets/go/target.toml", "x");
        assert_eq!(names(&tree, &roots(), "targets", BUILT_IN), ["ts", "go"]);
    }
}
