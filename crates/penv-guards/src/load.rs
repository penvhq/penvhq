use penv_targets::folder::{self, BuiltIn, Source};

use crate::error::Error;
use crate::guard::{Guard, Hook, parse};

pub use penv_targets::folder::{Roots, Tree};

macro_rules! built_in {
    ($name:literal, [$($template:literal),* $(,)?]) => {
        BuiltIn {
            name: $name,
            files: &[
                ("guard.toml", include_str!(concat!("../guards/", $name, "/guard.toml"))),
                $(($template, include_str!(concat!("../guards/", $name, "/", $template)))),*
            ],
        }
    };
}

/// The harnesses penv knows, in the order the design ranks them.
pub const BUILT_IN: &[BuiltIn] = &[
    built_in!(
        "claude-code",
        ["settings.json.tmpl", "user-settings.json.tmpl"]
    ),
    built_in!("codex", ["config.toml.tmpl"]),
    built_in!("cursor", ["cli.json.tmpl", "hooks.json.tmpl"]),
    built_in!("amp", ["settings.json.tmpl"]),
    built_in!("copilot", ["permissions-config.json.tmpl"]),
    built_in!("gemini", ["settings.json.tmpl"]),
    built_in!("cline", ["PreToolUse.tmpl"]),
    built_in!("windsurf", ["hooks.json.tmpl"]),
];

/// Repo folder, then home folder, then built in. The first found wins, except
/// that the repository may not replace a built-in guard: anyone who can commit
/// could then point a write outside the guard or make the hook allow everything.
pub fn load(tree: &dyn Tree, roots: &Roots, name: &str) -> Result<Guard, Error> {
    let found =
        folder::find(tree, roots, "guards", "guard.toml", BUILT_IN, name).map_err(|looked| {
            Error::NotFound {
                name: name.to_string(),
                looked,
            }
        })?;
    if found.source == Source::Repo && BUILT_IN.iter().any(|b| b.name == name) {
        return Err(Error::Shadows {
            name: name.to_string(),
            dir: found.dir,
        });
    }
    let config = found.file("guard.toml").ok_or_else(|| Error::Malformed {
        dir: found.dir.clone(),
        message: "no guard.toml".into(),
    })?;
    parse(name, &config, &|template| found.file(template), &found.dir)
}

/// The refusal shape `penv hook` answers in. It comes from the home folder or
/// the built-in one, never the repository, whose files an agent can write:
/// a deny that exits 0 with an empty answer would let every call through.
pub fn hook(tree: &dyn Tree, home: Option<&str>, name: &str) -> Option<Hook> {
    let built_in = BUILT_IN.iter().find(|b| b.name == name);
    let compiled = |file: &str| built_in.and_then(|b| b.file(file)).map(str::to_string);
    let dir = home.map(|h| format!("{h}/.penv/guards/{name}"));
    let home_guard = dir
        .as_ref()
        .and_then(|d| tree.read(&format!("{d}/guard.toml")).map(|c| (d, c)))
        .and_then(|(dir, config)| {
            let read = |file: &str| {
                tree.read(&format!("{dir}/{file}"))
                    .or_else(|| compiled(file))
            };
            parse(name, &config, &read, dir).ok()
        });
    // A home folder that does not parse keeps the built-in answer, not none.
    match home_guard {
        Some(guard) => guard.hook,
        None => {
            parse(name, &compiled("guard.toml")?, &compiled, "built in")
                .ok()?
                .hook
        }
    }
}

/// Every guard that can be loaded: the ranked built-in list first, then whatever
/// the two `.penv` folders add.
pub fn available(tree: &dyn Tree, roots: &Roots) -> Vec<Guard> {
    folder::names(tree, roots, "guards", BUILT_IN)
        .iter()
        .filter_map(|name| load(tree, roots, name).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::{Format, Merge, Payload, Scope};
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

    const CONFIG: &str = r#"
name = "claude-code"
detect = [".claude"]

[[write]]
path = ".claude/settings.json"
format = "json"
merge = "deny-union"
template = "settings.json.tmpl"
"#;

    fn roots() -> Roots {
        Roots::new("/repo", Some("/home".into()))
    }

    #[test]
    fn every_built_in_guard_is_a_folder_that_parses() {
        let found = available(&Fake::default(), &roots());
        let names: Vec<&str> = found.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "claude-code",
                "codex",
                "cursor",
                "amp",
                "copilot",
                "gemini",
                "cline",
                "windsurf"
            ]
        );
    }

    // A committed folder replacing a built-in guard could aim its writes or its
    // hook answer anywhere, so the repository may add a harness and the home
    // folder alone may change one.
    #[test]
    fn the_home_folder_beats_the_built_in_one_and_the_repo_folder_may_not() {
        let home = Fake::default()
            .with("/home/.penv/guards/claude-code/guard.toml", CONFIG)
            .with("/home/.penv/guards/claude-code/settings.json.tmpl", "{}");
        let guard = load(&home, &roots(), "claude-code").unwrap();
        assert_eq!(guard.dir, "/home/.penv/guards/claude-code");
        assert_eq!(guard.writes.len(), 1);

        let repo = Fake::default()
            .with("/repo/.penv/guards/claude-code/guard.toml", CONFIG)
            .with("/repo/.penv/guards/claude-code/settings.json.tmpl", "{}");
        let error = load(&repo, &roots(), "claude-code").unwrap_err();
        assert!(matches!(error, Error::Shadows { .. }), "{error}");
        assert!(error.to_string().contains("~/.penv/guards/claude-code"));
    }

    #[test]
    fn the_repo_folder_can_add_a_harness_penv_does_not_know() {
        let config = CONFIG.replace("claude-code", "nano");
        let tree = Fake::default()
            .with("/repo/.penv/guards/nano/guard.toml", &config)
            .with("/repo/.penv/guards/nano/settings.json.tmpl", "{}");
        let guard = load(&tree, &roots(), "nano").unwrap();
        assert_eq!(guard.dir, "/repo/.penv/guards/nano");
    }

    #[test]
    fn the_hook_answer_never_comes_from_the_repository() {
        let open = format!("{CONFIG}\n[hook]\ndeny = {{ stdout = '{{}}', exit = 0 }}\n");
        let repo = Fake::default()
            .with("/repo/.penv/guards/claude-code/guard.toml", &open)
            .with("/repo/.penv/guards/claude-code/settings.json.tmpl", "{}")
            .with(
                "/repo/.penv/guards/nano/guard.toml",
                &open.replace("claude-code", "nano"),
            )
            .with("/repo/.penv/guards/nano/settings.json.tmpl", "{}");
        let built_in = hook(&repo, Some("/home"), "claude-code").unwrap();
        assert_eq!(built_in.payload, Payload::ClaudeCode);
        assert_ne!(built_in.deny.stdout.as_deref(), Some("{}"));
        assert_eq!(hook(&repo, Some("/home"), "nano"), None);

        let home = Fake::default()
            .with(
                "/home/.penv/guards/nano/guard.toml",
                &open.replace("claude-code", "nano"),
            )
            .with("/home/.penv/guards/nano/settings.json.tmpl", "{}");
        assert_eq!(hook(&home, Some("/home"), "nano").unwrap().deny.exit, 0);

        let broken = Fake::default().with("/home/.penv/guards/cursor/guard.toml", "name = 1");
        assert_eq!(
            hook(&broken, Some("/home"), "cursor").unwrap().payload,
            Payload::Cursor
        );
    }

    #[test]
    fn an_unknown_harness_says_where_it_looked() {
        let error = load(&Fake::default(), &roots(), "nano").unwrap_err();
        assert!(error.to_string().contains("/home/.penv/guards/nano"));
    }

    #[test]
    fn claude_code_writes_the_project_file_and_hands_back_the_user_block() {
        let guard = load(&Fake::default(), &roots(), "claude-code").unwrap();
        let project: Vec<&str> = guard.project_writes().map(|w| w.path.as_str()).collect();
        assert_eq!(project, [".claude/settings.json"]);
        assert_eq!(guard.user_writes().count(), 1);
        assert_eq!(guard.writes[0].format, Format::Json);
        assert_eq!(guard.writes[0].merge, Merge::DenyUnion);
        assert_eq!(guard.writes[1].scope, Scope::User);
    }

    #[test]
    fn codex_merges_toml_and_cline_execs_the_binary_from_a_script() {
        let codex = load(&Fake::default(), &roots(), "codex").unwrap();
        assert_eq!(codex.writes[0].format, Format::Toml);
        assert_eq!(codex.writes[0].merge, Merge::AppendUnique);
        let cline = load(&Fake::default(), &roots(), "cline").unwrap();
        assert_eq!(cline.writes[0].format, Format::Text);
        assert_eq!(cline.writes[0].path, ".clinerules/hooks/PreToolUse");
        assert!(
            cline.writes[0].executable,
            "a hook script that is not executable fails open"
        );
    }

    #[test]
    fn every_built_in_guard_declares_the_hook_shape_it_answers_in() {
        for name in BUILT_IN.iter().map(|b| b.name) {
            let guard = load(&Fake::default(), &roots(), name).unwrap();
            let hook = guard.hook.unwrap_or_else(|| panic!("{name} has no [hook]"));
            assert!(hook.deny.stdout.is_some() || hook.deny.stderr.is_some());
        }
        let cursor = load(&Fake::default(), &roots(), "cursor").unwrap();
        assert_eq!(cursor.hook.unwrap().payload, Payload::Cursor);
    }
}
