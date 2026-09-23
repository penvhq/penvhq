//! Whether a target is relevant to a repository, and which directories to offer
//! when penv asks where its file goes. Detection never decides on its own.

use crate::folder::{Roots, Tree};
use crate::target::{Layout, Rule, Suggest, Target};

/// Directories a package never hides in. Anything starting with a dot is
/// skipped too, so `.git` and `.next` need no entry.
const NEVER: [&str; 4] = ["node_modules", "dist", "build", "target"];

/// How far under the repository root a package is looked for.
const DEPTH: usize = 3;

/// Every directory, relative to the repository root, holding any one of the
/// target's detect files, shallowest first. The empty string names the root,
/// which comes last so a monorepo's Enter never lands on it.
pub fn candidates(tree: &dyn Tree, roots: &Roots, target: &Target) -> Vec<String> {
    let mut out = Vec::new();
    if target.detect.is_empty() {
        return out;
    }
    walk(tree, &roots.repo, "", 0, target, &mut out);
    out.sort_by_key(|dir| (dir.is_empty(), dir.split('/').count(), dir.clone()));
    out
}

fn walk(
    tree: &dyn Tree,
    repo: &str,
    relative: &str,
    depth: usize,
    target: &Target,
    out: &mut Vec<String>,
) {
    let dir = under(repo, relative);
    let listed = if target.detect.iter().any(|f| f.starts_with("*.")) {
        tree.files(&dir)
    } else {
        Vec::new()
    };
    if target
        .detect
        .iter()
        .any(|file| match file.strip_prefix('*') {
            Some(suffix) => listed
                .iter()
                .any(|name| name.ends_with(suffix) && name.len() > suffix.len()),
            None => tree.exists(&format!("{dir}/{file}")),
        })
    {
        out.push(relative.to_string());
    }
    if depth == DEPTH {
        return;
    }
    for name in tree.dirs(&dir) {
        if name.starts_with('.') || NEVER.contains(&name.as_str()) {
            continue;
        }
        let child = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        walk(tree, repo, &child, depth + 1, target, out);
    }
}

/// The target's output under one candidate directory, in forward slashes and
/// relative to the repository root.
pub fn output_path(candidate: &str, output: &str) -> String {
    if candidate.is_empty() {
        output.to_string()
    } else {
        format!("{candidate}/{output}")
    }
}

/// The candidate a path sits in: the deepest one, or the root when none holds it.
pub fn package_of(path: &str, candidates: &[String]) -> String {
    candidates
        .iter()
        .filter(|dir| !dir.is_empty() && path.starts_with(&format!("{dir}/")))
        .max_by_key(|dir| dir.len())
        .cloned()
        .unwrap_or_default()
}

/// Where the target writes inside one package: the first `[[layout]]` the
/// package's own files match, else the target's plain `output`.
pub fn layout_output(tree: &dyn Tree, roots: &Roots, target: &Target, package: &str) -> String {
    let dir = under(&roots.repo, package);
    for layout in &target.layout {
        if let Some(name) = matched_layout(tree, &dir, layout) {
            return output_path(package, &layout.output.replace('*', &name));
        }
    }
    output_path(package, &target.output)
}

/// The directory the language imports from, dropped from a path before the
/// import line is built.
pub fn layout_root(target: &Target, relative: &str) -> Option<String> {
    target.layout.iter().find_map(|layout| {
        let root = layout.root.as_ref()?;
        relative
            .starts_with(&format!("{root}/"))
            .then(|| root.clone())
    })
}

/// The name the one `*` in a layout's `when` pattern stands for here.
fn matched_layout(tree: &dyn Tree, dir: &str, layout: &Layout) -> Option<String> {
    let (head, tail) = layout.when.split_once("/*/")?;
    for name in tree.dirs(&format!("{dir}/{head}")) {
        if name.starts_with('.') {
            continue;
        }
        if tree.exists(&format!("{dir}/{head}/{name}/{tail}")) {
            return Some(name);
        }
    }
    None
}

/// The value a package's files imply for one `[[suggest]]` knob, or `None` when
/// no rule matches and the target's own default stands. Reading inside a file is
/// a guess worth confirming, so a `contains` rule counts only where penv can ask.
pub fn suggested(
    tree: &dyn Tree,
    roots: &Roots,
    suggest: &Suggest,
    package: &str,
    can_ask: bool,
) -> Option<toml::Value> {
    let dir = under(&roots.repo, package);
    suggest
        .rule
        .iter()
        .find(|rule| matches(tree, &dir, rule, can_ask))
        .map(|rule| rule.value.clone())
}

fn matches(tree: &dyn Tree, dir: &str, rule: &Rule, can_ask: bool) -> bool {
    rule.files.iter().any(|file| {
        let path = format!("{dir}/{file}");
        match &rule.contains {
            None => tree.exists(&path),
            Some(text) => {
                can_ask
                    && tree
                        .read(&path)
                        .is_some_and(|body| mentions(&body, text, file))
            }
        }
    })
}

/// Whether a file says the text outside its own comments, so a dependency
/// somebody commented out is not one.
fn mentions(body: &str, text: &str, file: &str) -> bool {
    let marker = comment_marker(file);
    body.lines()
        .filter(|line| !marker.is_some_and(|marker| line.trim_start().starts_with(marker)))
        .any(|line| line.contains(text))
}

fn comment_marker(file: &str) -> Option<&'static str> {
    match file.rsplit('.').next().unwrap_or_default() {
        "toml" | "py" => Some("#"),
        "json" | "jsonc" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" => {
            Some("//")
        }
        _ => None,
    }
}

fn under(repo: &str, relative: &str) -> String {
    if relative.is_empty() {
        repo.to_string()
    } else {
        format!("{repo}/{relative}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder::Source;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake(BTreeMap<String, String>);

    impl Fake {
        fn with(mut self, path: &str) -> Fake {
            self.0.insert(path.to_string(), String::new());
            self
        }
        fn holding(mut self, path: &str, body: &str) -> Fake {
            self.0.insert(path.to_string(), body.to_string());
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
                .filter(|rest| rest.contains('/'))
                .filter_map(|rest| rest.split('/').next())
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

    fn target(detect: &[&str]) -> Target {
        Target {
            name: "ts".into(),
            output: "src/env.ts".into(),
            output_source: Source::BuiltIn,
            detect: detect.iter().map(|f| f.to_string()).collect(),
            types: BTreeMap::new(),
            options: toml::Table::new(),
            knobs: Vec::new(),
            suggest: Vec::new(),
            layout: Vec::new(),
            check: None,
            import: None,
            paths_from: None,
            template: String::new(),
            source: Source::BuiltIn,
            dir: "built in".into(),
        }
    }

    fn roots() -> Roots {
        Roots::new("/repo", None)
    }

    #[test]
    fn any_one_detect_file_makes_a_directory_a_suggestion() {
        let tree = Fake::default()
            .with("/repo/apps/web/package.json")
            .with("/repo/apps/web/tsconfig.json")
            .with("/repo/apps/api/package.json");
        let found = candidates(&tree, &roots(), &target(&["package.json", "tsconfig.json"]));
        assert_eq!(found, ["apps/api", "apps/web"]);
    }

    #[test]
    fn a_pattern_matches_any_file_with_that_extension_and_nothing_else() {
        let tree = Fake::default()
            .with("/repo/services/api/Api.csproj")
            .with("/repo/services/web/web.csproj.user")
            .with("/repo/tools/.csproj");
        let found = candidates(&tree, &roots(), &target(&["*.csproj"]));
        assert_eq!(found, ["services/api"]);
    }

    #[test]
    fn the_root_counts_only_when_it_holds_one_itself_and_comes_last() {
        let tree = Fake::default()
            .with("/repo/package.json")
            .with("/repo/apps/web/package.json");
        let found = candidates(&tree, &roots(), &target(&["package.json", "tsconfig.json"]));
        assert_eq!(found, ["apps/web", ""]);

        let alone = Fake::default().with("/repo/package.json");
        assert_eq!(
            candidates(&alone, &roots(), &target(&["package.json"])),
            [""]
        );
    }

    #[test]
    fn the_shallowest_package_is_first_and_the_root_is_never_what_enter_takes() {
        let tree = Fake::default()
            .with("/repo/package.json")
            .with("/repo/zzz/package.json")
            .with("/repo/aaa/deep/package.json");
        let found = candidates(&tree, &roots(), &target(&["package.json"]));
        assert_eq!(found, ["zzz", "aaa/deep", ""]);
    }

    #[test]
    fn a_workspace_root_is_a_suggestion_like_any_other_and_never_a_verdict() {
        let tree = Fake::default()
            .with("/repo/package.json")
            .with("/repo/pnpm-workspace.yaml")
            .with("/repo/apps/web/package.json");
        let found = candidates(&tree, &roots(), &target(&["package.json"]));
        assert_eq!(found, ["apps/web", ""]);
    }

    #[test]
    fn the_walk_skips_what_a_build_leaves_behind_and_stops_at_three_levels() {
        let tree = Fake::default()
            .with("/repo/node_modules/pkg/package.json")
            .with("/repo/.next/types/package.json")
            .with("/repo/.git/hooks/package.json")
            .with("/repo/apps/web/dist/package.json")
            .with("/repo/a/b/c/d/package.json");
        let found = candidates(&tree, &roots(), &target(&["package.json"]));
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_target_with_nothing_to_detect_belongs_nowhere_by_itself() {
        let tree = Fake::default().with("/repo/package.json");
        assert!(candidates(&tree, &roots(), &target(&[])).is_empty());
    }

    #[test]
    fn an_output_hangs_off_the_package_that_holds_it() {
        assert_eq!(output_path("apps/web", "src/env.ts"), "apps/web/src/env.ts");
        assert_eq!(output_path("", "src/env.ts"), "src/env.ts");
    }

    #[test]
    fn a_written_path_names_the_deepest_package_it_sits_in() {
        let packages = ["".to_string(), "apps".to_string(), "apps/web".to_string()];
        assert_eq!(package_of("apps/web/src/env.ts", &packages), "apps/web");
        assert_eq!(package_of("src/env.ts", &packages), "");
        assert_eq!(package_of("apps/web/src/env.ts", &[]), "");
    }

    #[test]
    fn a_src_layout_takes_the_output_into_the_package_it_names() {
        let mut py = target(&["pyproject.toml"]);
        py.output = "penv_env.py".into();
        py.layout = vec![Layout {
            when: "src/*/__init__.py".into(),
            output: "src/*/penv_env.py".into(),
            root: Some("src".into()),
        }];
        let src = Fake::default()
            .with("/repo/services/api/pyproject.toml")
            .with("/repo/services/api/src/billing/__init__.py");
        assert_eq!(
            layout_output(&src, &roots(), &py, "services/api"),
            "services/api/src/billing/penv_env.py"
        );
        assert_eq!(
            layout_root(&py, "src/billing/penv_env.py").as_deref(),
            Some("src")
        );

        let flat = Fake::default().with("/repo/services/api/pyproject.toml");
        assert_eq!(
            layout_output(&flat, &roots(), &py, "services/api"),
            "services/api/penv_env.py"
        );
        assert_eq!(layout_root(&py, "penv_env.py"), None);
    }

    fn word(value: &str) -> toml::Value {
        toml::Value::String(value.into())
    }

    #[test]
    fn a_suggestion_reads_the_files_of_the_package_that_was_chosen() {
        let mut ts = target(&["package.json"]);
        ts.suggest = vec![Suggest {
            option: "runtime".into(),
            prompt: "which runtime reads the env?".into(),
            rule: vec![
                Rule {
                    value: word("vite"),
                    files: vec!["vite.config.ts".into()],
                    contains: None,
                },
                Rule {
                    value: word("deno"),
                    files: vec!["deno.json".into()],
                    contains: None,
                },
            ],
        }];
        let tree = Fake::default()
            .with("/repo/apps/web/vite.config.ts")
            .with("/repo/apps/edge/deno.json")
            .with("/repo/apps/api/package.json");
        let ask = |package: &str| suggested(&tree, &roots(), &ts.suggest[0], package, false);
        assert_eq!(ask("apps/web"), Some(word("vite")));
        assert_eq!(ask("apps/edge"), Some(word("deno")));
        assert_eq!(ask("apps/api"), None, "no rule leaves the target's default");
    }

    #[test]
    fn a_rule_that_names_contains_reads_the_file_outside_its_comments_and_only_to_ask() {
        let suggest = Suggest {
            option: "pydantic".into(),
            prompt: "use pydantic types?".into(),
            rule: vec![Rule {
                value: toml::Value::Boolean(true),
                files: vec!["pyproject.toml".into()],
                contains: Some("pydantic".into()),
            }],
        };
        let ask = |tree: &Fake, can_ask| suggested(tree, &roots(), &suggest, "api", can_ask);

        let with = Fake::default().holding(
            "/repo/api/pyproject.toml",
            "[project]\ndependencies = [\"pydantic>=2\"]\n",
        );
        assert_eq!(ask(&with, true), Some(toml::Value::Boolean(true)));
        assert_eq!(
            ask(&with, false),
            None,
            "a file's contents never flip a knob nobody can confirm"
        );

        let commented = Fake::default().holding(
            "/repo/api/pyproject.toml",
            "[project]\ndependencies = [\n  # \"pydantic>=2\",\n]\n",
        );
        assert_eq!(ask(&commented, true), None, "a comment is not a dependency");

        let without = Fake::default().holding("/repo/api/pyproject.toml", "[project]\n");
        assert_eq!(ask(&without, true), None);
    }
}
