use serde_json::Value;
use toml_edit::{DocumentMut, TableLike};

use crate::error::Error;
use crate::guard::{Format, Merge, Write};

/// What a write would leave on disk, and whether that differs from what is
/// there now. Applying the same fragment twice changes nothing the second time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub content: String,
    pub changed: bool,
    /// The parts of the fragment the existing file holds something else in
    /// place of, by dotted path: left as the file has them, and not in force.
    pub blocked: Vec<String>,
}

/// Fold a rendered fragment into whatever is already at the path. Nothing an
/// existing file says is ever removed or overwritten.
pub fn apply(write: &Write, existing: Option<&str>, fragment: &str) -> Result<Outcome, Error> {
    match write.format {
        Format::Json => json(write, existing, fragment),
        Format::Toml => toml_format(write, existing, fragment),
        Format::Text if fragment.starts_with("#!") => Ok(script(existing, fragment)),
        Format::Text => Ok(text(existing.unwrap_or_default(), fragment)),
    }
}

fn json(write: &Write, existing: Option<&str>, fragment: &str) -> Result<Outcome, Error> {
    let add: Value = serde_json::from_str(fragment).map_err(|e| Error::Unreadable {
        path: write.template.clone(),
        message: format!("the template did not render JSON: {e}"),
    })?;
    let Some(source) = existing else {
        return Ok(Outcome {
            content: write_json(&add),
            changed: true,
            blocked: Vec::new(),
        });
    };
    let mut base: Value = serde_json::from_str(source).map_err(|e| Error::Unreadable {
        path: write.path.clone(),
        message: format!("the file on disk is not JSON: {e}"),
    })?;
    let before = base.clone();
    let mut blocked = Vec::new();
    merge_json(&mut base, &add, "", &write.union, write.merge, &mut blocked);
    Ok(if base == before {
        Outcome {
            content: source.to_string(),
            changed: false,
            blocked,
        }
    } else {
        Outcome {
            content: write_json(&base),
            changed: true,
            blocked,
        }
    })
}

fn write_json(value: &Value) -> String {
    let mut out = serde_json::to_string_pretty(value).unwrap_or_default();
    out.push('\n');
    out
}

fn merge_json(
    base: &mut Value,
    add: &Value,
    path: &str,
    union: &[String],
    mode: Merge,
    blocked: &mut Vec<String>,
) {
    match (base, add) {
        (Value::Object(base), Value::Object(add)) => {
            for (name, value) in add {
                match base.get_mut(name) {
                    Some(slot) => merge_json(slot, value, &child(path, name), union, mode, blocked),
                    None => {
                        base.insert(name.clone(), value.clone());
                    }
                }
            }
        }
        (Value::Array(base), Value::Array(add)) if unites(leaf(path), union, mode) => {
            for item in add {
                if !base.contains(item) {
                    base.push(item.clone());
                }
            }
        }
        (base, add) if base != add => blocked.push(path.to_string()),
        _ => {}
    }
}

fn child(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

fn leaf(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

fn unites(key: &str, union: &[String], mode: Merge) -> bool {
    match mode {
        Merge::DenyUnion => union.iter().any(|k| k == key),
        Merge::AppendUnique => true,
        Merge::ReplaceIfAbsent => false,
    }
}

/// TOML is edited in place, so the comments and order a person wrote survive.
fn toml_format(write: &Write, existing: Option<&str>, fragment: &str) -> Result<Outcome, Error> {
    let add: DocumentMut =
        fragment
            .parse()
            .map_err(|e: toml_edit::TomlError| Error::Unreadable {
                path: write.template.clone(),
                message: format!("the template did not render TOML: {}", e.message()),
            })?;
    let Some(source) = existing else {
        return Ok(Outcome {
            content: add.to_string(),
            changed: true,
            blocked: Vec::new(),
        });
    };
    let mut base: DocumentMut =
        source
            .parse()
            .map_err(|e: toml_edit::TomlError| Error::Unreadable {
                path: write.path.clone(),
                message: format!("the file on disk is not TOML: {}", e.message()),
            })?;
    let mut blocked = Vec::new();
    merge_table(base.as_table_mut(), add.as_table(), "", &mut blocked);
    let content = base.to_string();
    Ok(Outcome {
        changed: content != source,
        content,
        blocked,
    })
}

fn merge_table(
    base: &mut dyn TableLike,
    add: &dyn TableLike,
    path: &str,
    blocked: &mut Vec<String>,
) {
    for (name, item) in add.iter() {
        let at = child(path, name);
        let Some(slot) = base.get_mut(name) else {
            base.insert(name, item.clone());
            continue;
        };
        if let (Some(slot), Some(item)) = (slot.as_table_like_mut(), item.as_table_like()) {
            merge_table(slot, item, &at, blocked);
            continue;
        }
        // `[[x]]` entries: penv's is added unless an equal one is already there.
        if let (Some(slot), Some(items)) =
            (slot.as_array_of_tables_mut(), item.as_array_of_tables())
        {
            for table in items.iter() {
                if !slot.iter().any(|t| plain_table(t) == plain_table(table)) {
                    slot.push(table.clone());
                }
            }
            continue;
        }
        match (slot.as_array_mut(), item.as_array()) {
            (Some(slot), Some(items)) => {
                for value in items.iter() {
                    if !slot.iter().any(|v| plain(v) == plain(value)) {
                        slot.push(value.clone());
                    }
                }
            }
            _ => {
                let same = match (slot.as_value(), item.as_value()) {
                    (Some(a), Some(b)) => plain(a) == plain(b),
                    _ => false,
                };
                if !same {
                    blocked.push(at);
                }
            }
        }
    }
}

/// A table without its layout, for comparing.
fn plain_table(table: &toml_edit::Table) -> String {
    let mut table = table.clone();
    table.decor_mut().clear();
    table.fmt();
    table.to_string()
}

/// A value without the spacing and comments around it, for comparing.
fn plain(value: &toml_edit::Value) -> String {
    let mut value = value.clone();
    value.decor_mut().clear();
    value.to_string()
}

/// A hook script runs top to bottom and may exit early or exec something else,
/// so penv's lines count only as the first commands after the shebang. A script
/// that already runs something else there is not penv's to rewrite.
fn script(existing: Option<&str>, fragment: &str) -> Outcome {
    let wanted: Vec<&str> = fragment.lines().skip(1).collect();
    let source = existing.unwrap_or_default();
    let lines: Vec<&str> = source.lines().collect();
    let head = usize::from(lines.first().is_some_and(|l| l.starts_with("#!")));
    let commands = || {
        lines[head..]
            .iter()
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
    };
    if commands().next().is_none() {
        let mut out: Vec<&str> = if head == 1 {
            vec![lines[0]]
        } else {
            vec![fragment.lines().next().unwrap_or_default()]
        };
        out.extend(&wanted);
        out.extend(&lines[head..]);
        let mut content = out.join("\n");
        content.push('\n');
        let changed = content != source;
        return Outcome {
            content: if changed { content } else { source.to_string() },
            changed,
            blocked: Vec::new(),
        };
    }
    let placed = commands()
        .take(wanted.len())
        .copied()
        .eq(wanted.iter().copied());
    Outcome {
        content: source.to_string(),
        changed: false,
        blocked: if placed {
            Vec::new()
        } else {
            wanted.iter().map(|l| l.to_string()).collect()
        },
    }
}

fn text(existing: &str, fragment: &str) -> Outcome {
    let mut lines: Vec<&str> = existing.lines().collect();
    let mut changed = false;
    for line in fragment.lines() {
        if !lines.contains(&line) {
            lines.push(line);
            changed = true;
        }
    }
    if !changed {
        return Outcome {
            content: existing.to_string(),
            changed: false,
            blocked: Vec::new(),
        };
    }
    let mut content = lines.join("\n");
    content.push('\n');
    Outcome {
        content,
        changed: true,
        blocked: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::Scope;

    fn write(format: Format, merge: Merge) -> Write {
        Write {
            path: "settings".into(),
            format,
            merge,
            scope: Scope::Project,
            union: ["deny", "denyRead", "hooks"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            executable: false,
            template: "settings.tmpl".into(),
            body: String::new(),
        }
    }

    fn twice(write: &Write, existing: Option<&str>, fragment: &str) -> Outcome {
        let once = apply(write, existing, fragment).unwrap();
        let again = apply(write, Some(&once.content), fragment).unwrap();
        assert!(!again.changed, "the second write changed the file");
        assert_eq!(once.content, again.content);
        once
    }

    #[test]
    fn a_first_json_write_is_the_fragment() {
        let out = twice(
            &write(Format::Json, Merge::DenyUnion),
            None,
            r#"{"permissions":{"deny":["Read(.env)"]}}"#,
        );
        assert!(out.changed);
        assert!(out.content.ends_with("\n"));
        assert!(out.content.contains("Read(.env)"));
    }

    #[test]
    fn a_named_array_gains_entries_and_loses_none() {
        let out = twice(
            &write(Format::Json, Merge::DenyUnion),
            Some(r#"{"permissions":{"deny":["Read(secrets.json)"]}}"#),
            r#"{"permissions":{"deny":["Read(.env)"]}}"#,
        );
        let merged: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(
            merged["permissions"]["deny"],
            serde_json::json!(["Read(secrets.json)", "Read(.env)"])
        );
    }

    #[test]
    fn an_existing_setting_is_never_overwritten() {
        let out = apply(
            &write(Format::Json, Merge::DenyUnion),
            Some(r#"{"sandbox":{"enabled":false}}"#),
            r#"{"sandbox":{"enabled":true,"filesystem":{"denyRead":[".env"]}}}"#,
        )
        .unwrap();
        let merged: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(merged["sandbox"]["enabled"], false);
        assert_eq!(
            merged["sandbox"]["filesystem"]["denyRead"],
            serde_json::json!([".env"])
        );
    }

    #[test]
    fn an_array_nobody_named_is_left_where_it_is() {
        let out = apply(
            &write(Format::Json, Merge::DenyUnion),
            Some(r#"{"allow":["Bash"]}"#),
            r#"{"allow":["Read"]}"#,
        )
        .unwrap();
        assert!(!out.changed);
    }

    #[test]
    fn replace_if_absent_only_fills_in_what_is_missing() {
        let out = apply(
            &write(Format::Json, Merge::ReplaceIfAbsent),
            Some(r#"{"amp.guardedFiles.allowlist":["notes.md"]}"#),
            r#"{"amp.guardedFiles.allowlist":[]}"#,
        )
        .unwrap();
        assert!(!out.changed);
    }

    #[test]
    fn a_json_file_that_is_not_json_says_so_instead_of_clobbering_it() {
        let error = apply(
            &write(Format::Json, Merge::DenyUnion),
            Some("// comments are not JSON\n{}"),
            "{}",
        )
        .unwrap_err();
        assert!(error.to_string().contains("not JSON"));
    }

    #[test]
    fn toml_tables_merge_and_arrays_append() {
        let out = twice(
            &write(Format::Toml, Merge::AppendUnique),
            Some("[sandbox_workspace_write]\ndeny_read = [\"**/secrets\"]\n"),
            "[sandbox_workspace_write]\ndeny_read = [\"**/.env\"]\n\n[shell_environment_policy]\ninherit = \"core\"\n",
        );
        let merged: toml::Value = toml::from_str(&out.content).unwrap();
        assert_eq!(
            merged["sandbox_workspace_write"]["deny_read"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            merged["shell_environment_policy"]["inherit"].as_str(),
            Some("core")
        );
    }

    #[test]
    fn text_appends_the_lines_that_are_not_there_yet() {
        let out = twice(
            &write(Format::Text, Merge::AppendUnique),
            Some("#!/bin/sh\n"),
            "#!/bin/sh\nexec penv hook cline\n",
        );
        assert_eq!(out.content, "#!/bin/sh\nexec penv hook cline\n");
    }

    const HOOK: &str = "#!/bin/sh\nexec penv hook cline \"$@\"\n";

    #[test]
    fn a_script_gains_penvs_lines_right_after_its_shebang_and_never_a_second_one() {
        let write = write(Format::Text, Merge::AppendUnique);
        let out = twice(&write, Some("#!/bin/bash\n# my hook\n"), HOOK);
        assert_eq!(
            out.content,
            "#!/bin/bash\nexec penv hook cline \"$@\"\n# my hook\n"
        );
        assert_eq!(out.content.matches("#!").count(), 1);
        assert_eq!(twice(&write, None, HOOK).content, HOOK);
    }

    #[test]
    fn a_script_that_runs_something_else_first_is_reported_and_left_alone() {
        let write = write(Format::Text, Merge::AppendUnique);
        for existing in [
            "#!/bin/sh\nexit 0\nexec penv hook cline \"$@\"\n",
            "#!/bin/sh\nexec other-hook\n",
            "echo hi\n",
        ] {
            let out = apply(&write, Some(existing), HOOK).unwrap();
            assert!(!out.changed, "{existing}");
            assert_eq!(out.content, existing);
            assert!(!out.blocked.is_empty(), "{existing} reads as current");
        }
        let placed = apply(&write, Some(HOOK), HOOK).unwrap();
        assert!(placed.blocked.is_empty() && !placed.changed);
    }

    #[test]
    fn a_rule_an_existing_value_stands_in_the_way_of_is_reported() {
        let amp = apply(
            &write(Format::Json, Merge::ReplaceIfAbsent),
            Some(r#"{"amp.guardedFiles.allowlist":[".env"]}"#),
            r#"{"amp.guardedFiles.allowlist":[]}"#,
        )
        .unwrap();
        assert!(!amp.changed);
        assert_eq!(amp.blocked, ["amp.guardedFiles.allowlist"]);

        let deny = apply(
            &write(Format::Json, Merge::DenyUnion),
            Some(r#"{"permissions":{"deny":"Read(x)"}}"#),
            r#"{"permissions":{"deny":["Read(.env)"]}}"#,
        )
        .unwrap();
        assert_eq!(deny.blocked, ["permissions.deny"]);
        assert!(!deny.changed, "never weakened");

        let codex = apply(
            &write(Format::Toml, Merge::AppendUnique),
            Some("[shell_environment_policy]\nignore_default_excludes = true\n"),
            "[shell_environment_policy]\nignore_default_excludes = false\n",
        )
        .unwrap();
        assert!(!codex.changed);
        assert_eq!(
            codex.blocked,
            ["shell_environment_policy.ignore_default_excludes"]
        );

        let same = apply(
            &write(Format::Json, Merge::DenyUnion),
            Some(r#"{"sandbox":{"enabled":true}}"#),
            r#"{"sandbox":{"enabled":true}}"#,
        )
        .unwrap();
        assert!(same.blocked.is_empty());
    }

    #[test]
    fn an_array_of_tables_is_unioned_and_an_equal_entry_is_current() {
        let fragment = "[[hooks]]\nevent = \"pre\"\ncommand = \"penv hook x\"\n";
        let rule = write(Format::Toml, Merge::AppendUnique);
        let same = apply(&rule, Some(fragment), fragment).unwrap();
        assert!(!same.changed && same.blocked.is_empty(), "{same:?}");
        let other = "[[hooks]]\nevent = \"post\"\ncommand = \"mine\"\n";
        let added = twice(&rule, Some(other), fragment);
        assert!(added.blocked.is_empty(), "{added:?}");
        assert!(added.content.contains("mine") && added.content.contains("penv hook x"));
    }

    #[test]
    fn a_merged_file_keeps_its_comments_and_its_order() {
        let toml = twice(
            &write(Format::Toml, Merge::AppendUnique),
            Some(
                "# mine\nmodel = \"o3\" # pinned\n\n[sandbox_workspace_write]\n# keep\ndeny_read = [\"**/secrets\"]\n",
            ),
            "[sandbox_workspace_write]\ndeny_read = [\"**/.env\"]\n\n[shell_environment_policy]\ninherit = \"core\"\n",
        );
        for kept in ["# mine", "# pinned", "# keep"] {
            assert!(toml.content.contains(kept), "{kept} lost: {}", toml.content);
        }
        assert!(toml.content.starts_with("# mine\nmodel = \"o3\""));
        assert!(toml.content.contains("**/.env"));

        let json = twice(
            &write(Format::Json, Merge::DenyUnion),
            Some(r#"{"zeta":1,"alpha":2,"permissions":{"deny":[]}}"#),
            r#"{"permissions":{"deny":["Read(.env)"]}}"#,
        );
        let zeta = json.content.find("zeta").unwrap();
        assert!(
            zeta < json.content.find("alpha").unwrap(),
            "{}",
            json.content
        );
    }
}
