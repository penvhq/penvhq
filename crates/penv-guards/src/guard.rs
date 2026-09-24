use serde::Deserialize;

use crate::error::Error;

/// Array keys a merge unions instead of leaving alone.
const DEFAULT_UNION: [&str; 5] = ["deny", "denyRead", "files", "envVars", "hooks"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Project,
    User,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::User => "user",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Json,
    Toml,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Merge {
    /// Deep merge; named arrays are unioned, nothing existing is removed.
    DenyUnion,
    /// Append what is not already there, line by line or entry by entry.
    AppendUnique,
    /// Add absent keys only; an existing setting is the user's, not ours.
    ReplaceIfAbsent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub path: String,
    pub format: Format,
    pub merge: Merge,
    pub scope: Scope,
    pub union: Vec<String>,
    /// A hook script the harness runs itself; it needs the execute bit.
    pub executable: bool,
    pub template: String,
    /// The template body, read from the folder beside `guard.toml`.
    pub body: String,
}

/// The input shape a harness hands its hook. Three families cover every harness;
/// a new one picks the family it speaks, not a branch in the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Payload {
    /// `tool_input` carrying a path, a pattern or a command.
    ClaudeCode,
    /// The read or shell fields at the top level of the object.
    Cursor,
    /// Anything else: the fields are found by name, wherever they sit.
    #[default]
    Generic,
}

/// What the harness expects to see when the hook refuses, as a template over
/// `reason`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deny {
    #[serde(default)]
    pub stdout: Option<String>,
    #[serde(default)]
    pub stderr: Option<String>,
    pub exit: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    #[serde(default)]
    pub payload: Payload,
    pub deny: Deny,
}

impl Hook {
    /// What a harness with no folder of its own gets: a reason on stderr and the
    /// exit code every hook protocol reads as a refusal.
    pub fn generic() -> Hook {
        Hook {
            payload: Payload::Generic,
            deny: Deny {
                stdout: None,
                stderr: Some("{{ reason }}".into()),
                exit: 2,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guard {
    pub name: String,
    pub description: Option<String>,
    pub scope: Scope,
    /// Paths relative to the repository, or `~`-prefixed for the home directory.
    pub detect: Vec<String>,
    /// Executables whose presence on PATH also means installed.
    pub exe: Vec<String>,
    pub writes: Vec<Write>,
    pub hook: Option<Hook>,
    pub dir: String,
}

impl Guard {
    /// The writes a `guard` run puts on disk.
    pub fn project_writes(&self) -> impl Iterator<Item = &Write> {
        self.writes.iter().filter(|w| w.scope == Scope::Project)
    }

    /// The blocks a person has to paste into their own settings.
    pub fn user_writes(&self) -> impl Iterator<Item = &Write> {
        self.writes.iter().filter(|w| w.scope == Scope::User)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "project")]
    scope: Scope,
    #[serde(default)]
    detect: Vec<String>,
    #[serde(default)]
    exe: Vec<String>,
    #[serde(default, rename = "write")]
    writes: Vec<WriteFile>,
    #[serde(default)]
    hook: Option<Hook>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteFile {
    path: String,
    format: Format,
    merge: Merge,
    #[serde(default)]
    scope: Option<Scope>,
    #[serde(default)]
    union: Option<Vec<String>>,
    #[serde(default)]
    executable: bool,
    template: String,
}

fn project() -> Scope {
    Scope::Project
}

/// Read one guard folder whose files have already been fetched into strings.
pub fn parse(
    name: &str,
    config: &str,
    templates: &dyn Fn(&str) -> Option<String>,
    dir: &str,
) -> Result<Guard, Error> {
    let file: File = toml::from_str(config).map_err(|e| Error::Malformed {
        dir: dir.to_string(),
        message: e.message().to_string(),
    })?;
    if file.name != name {
        return Err(Error::Malformed {
            dir: dir.to_string(),
            message: format!("guard.toml names {}, the folder names {name}", file.name),
        });
    }
    if file.writes.is_empty() {
        return Err(Error::Malformed {
            dir: dir.to_string(),
            message: "a guard with no [[write]] entry writes nothing".into(),
        });
    }
    if let Some(hook) = &file.hook
        && hook.deny.stdout.is_some() == hook.deny.stderr.is_some()
    {
        return Err(Error::Malformed {
            dir: dir.to_string(),
            message: "[hook] deny answers on stdout or on stderr, not both and not neither".into(),
        });
    }

    let writes = file
        .writes
        .into_iter()
        .map(|w| {
            let scope = w.scope.unwrap_or(file.scope);
            if let Some(problem) = unsafe_path(&w.path, scope) {
                return Err(Error::Malformed {
                    dir: dir.to_string(),
                    message: format!(
                        "the write path {} {problem}; a project write names a file inside the repository",
                        w.path
                    ),
                });
            }
            let body = templates(&w.template).ok_or_else(|| Error::Malformed {
                dir: dir.to_string(),
                message: format!("{} is missing", w.template),
            })?;
            Ok(Write {
                path: w.path,
                format: w.format,
                merge: w.merge,
                scope,
                union: w
                    .union
                    .unwrap_or_else(|| DEFAULT_UNION.iter().map(|s| s.to_string()).collect()),
                executable: w.executable,
                template: w.template,
                body,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;

    Ok(Guard {
        name: file.name,
        description: file.description,
        scope: file.scope,
        detect: file.detect,
        exe: file.exe,
        writes,
        hook: file.hook,
        dir: dir.to_string(),
    })
}

/// Why a write path could land outside the repository, if it could. A user-scope
/// path is only printed, so it may start at `~`.
fn unsafe_path(path: &str, scope: Scope) -> Option<&'static str> {
    let drive = path.len() >= 2 && path.as_bytes()[1] == b':';
    if path.is_empty() {
        return Some("is empty");
    }
    if path.starts_with(['/', '\\']) || drive {
        return Some("is absolute");
    }
    if path.split(['/', '\\']).any(|part| part == "..") {
        return Some("climbs out with ..");
    }
    if scope == Scope::Project && path.starts_with('~') {
        return Some("starts in the home directory");
    }
    None
}

/// What a harness looks like when it is installed.
pub trait Probe {
    /// `path` is a detect entry exactly as the guard wrote it.
    fn exists(&self, path: &str) -> bool;
    fn on_path(&self, exe: &str) -> bool;
}

pub fn is_installed(guard: &Guard, probe: &dyn Probe) -> bool {
    guard.detect.iter().any(|p| probe.exists(p)) || guard.exe.iter().any(|e| probe.on_path(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn templates(name: &str) -> Option<String> {
        Some(format!("body of {name}"))
    }

    const CONFIG: &str = r#"
name = "acme"
description = "an example"
detect = [".acme", "~/.acme"]
exe = ["acme"]

[[write]]
path = ".acme/settings.json"
format = "json"
merge = "deny-union"
template = "settings.json.tmpl"

[[write]]
path = "~/.acme/settings.json"
scope = "user"
format = "json"
merge = "deny-union"
union = ["envVars"]
template = "user.json.tmpl"

[hook]
payload = "cursor"
deny = { stdout = '{"permission": "deny"}', exit = 0 }
"#;

    fn guard() -> Guard {
        parse("acme", CONFIG, &templates, "guards/acme").unwrap()
    }

    #[test]
    fn a_folder_of_toml_and_templates_is_a_guard() {
        let guard = guard();
        assert_eq!(guard.detect, [".acme", "~/.acme"]);
        assert_eq!(guard.exe, ["acme"]);
        assert_eq!(guard.writes[0].body, "body of settings.json.tmpl");
        assert_eq!(guard.writes[0].format, Format::Json);
        assert_eq!(guard.writes[0].merge, Merge::DenyUnion);
    }

    #[test]
    fn a_write_inherits_the_guard_scope_and_can_override_it() {
        let guard = guard();
        assert_eq!(guard.writes[0].scope, Scope::Project);
        assert_eq!(guard.writes[1].scope, Scope::User);
        assert_eq!(guard.project_writes().count(), 1);
        assert_eq!(guard.user_writes().count(), 1);
    }

    #[test]
    fn the_union_list_defaults_to_the_keys_that_hold_rules() {
        let guard = guard();
        assert!(guard.writes[0].union.iter().any(|k| k == "denyRead"));
        assert_eq!(guard.writes[1].union, ["envVars"]);
    }

    #[test]
    fn the_hook_response_shape_comes_out_of_the_folder() {
        let hook = guard().hook.unwrap();
        assert_eq!(hook.payload, Payload::Cursor);
        assert_eq!(hook.deny.exit, 0);
        assert_eq!(
            hook.deny.stdout.as_deref(),
            Some(r#"{"permission": "deny"}"#)
        );
    }

    #[test]
    fn a_deny_that_answers_nowhere_or_on_both_streams_is_refused() {
        for deny in [
            "deny = { exit = 2 }",
            "deny = { stdout = 'a', stderr = 'b', exit = 2 }",
        ] {
            let without = CONFIG.split("[hook]").next().unwrap();
            let config = format!("{without}[hook]\n{deny}\n");
            let error = parse("acme", &config, &templates, "guards/acme").unwrap_err();
            assert!(error.to_string().contains("not both and not neither"));
        }
    }

    #[test]
    fn a_named_template_that_is_not_in_the_folder_is_a_broken_guard() {
        let error = parse("acme", CONFIG, &|_| None, "guards/acme").unwrap_err();
        assert!(error.to_string().contains("is missing"));
    }

    #[test]
    fn a_guard_that_writes_nothing_is_refused() {
        let error = parse("acme", "name = \"acme\"\n", &templates, "guards/acme").unwrap_err();
        assert!(error.to_string().contains("writes nothing"));
    }

    #[test]
    fn a_project_write_cannot_leave_the_repository() {
        for (path, scope) in [
            ("/usr/local/bin/penv", "project"),
            ("C:\\Windows\\x", "project"),
            ("\\\\server\\share\\x", "project"),
            ("../../.bashrc", "project"),
            (".claude/../../x", "project"),
            ("~/.bashrc", "project"),
            ("/etc/profile", "user"),
            ("~/../x", "user"),
        ] {
            let config = format!(
                "name = \"acme\"\n[[write]]\npath = '{path}'\nscope = \"{scope}\"\nformat = \"text\"\nmerge = \"append-unique\"\ntemplate = \"t\"\n"
            );
            let error = parse("acme", &config, &templates, "guards/acme").unwrap_err();
            assert!(error.to_string().contains(path), "{path}: {error}");
        }
        assert!(guard().writes.iter().any(|w| w.path.starts_with("~/")));
    }

    struct Fake<'a>(&'a [&'a str], &'a [&'a str]);

    impl Probe for Fake<'_> {
        fn exists(&self, path: &str) -> bool {
            self.0.contains(&path)
        }
        fn on_path(&self, exe: &str) -> bool {
            self.1.contains(&exe)
        }
    }

    #[test]
    fn a_harness_is_installed_when_a_folder_or_the_binary_is_there() {
        let guard = guard();
        assert!(is_installed(&guard, &Fake(&["~/.acme"], &[])));
        assert!(is_installed(&guard, &Fake(&[], &["acme"])));
        assert!(!is_installed(&guard, &Fake(&["~/.other"], &["other"])));
    }
}
