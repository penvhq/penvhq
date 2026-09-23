use minijinja::context;
use minijinja::value::Value as Jinja;
use penv_targets::folder::{describe, environment};
use serde_json::{Value, json};

use crate::error::Error;
use crate::guard::{Guard, Hook, Write};

/// A guard template sees the schema JSON and `penv.version`. Key names are all a
/// harness needs; values never enter the context.
pub fn render(
    guard: &Guard,
    write: &Write,
    schema: &Value,
    penv_version: &str,
) -> Result<String, Error> {
    let mut context = schema.clone();
    context["penv"] = json!({ "version": penv_version });

    environment()
        .render_str(&write.body, Jinja::from_serialize(&context))
        .map_err(|e| Error::Render {
            guard: guard.name.clone(),
            message: format!("{}: {}", write.template, describe(&e)),
        })
}

/// The refusal, in the shape the harness's folder declares, over `reason`.
pub fn deny(name: &str, hook: &Hook, reason: &str) -> Result<String, Error> {
    let body = hook
        .deny
        .stdout
        .as_deref()
        .or(hook.deny.stderr.as_deref())
        .unwrap_or("{{ reason }}");
    environment()
        .render_str(body, context! { reason })
        .map_err(|e| Error::Render {
            guard: name.to_string(),
            message: format!("the [hook] deny template: {}", describe(&e)),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::{Roots, Tree, load};

    struct Empty;

    impl Tree for Empty {
        fn read(&self, _path: &str) -> Option<String> {
            None
        }
        fn dirs(&self, _path: &str) -> Vec<String> {
            Vec::new()
        }
    }

    fn schema() -> Value {
        json!({
            "schemaVersion": 1,
            "org": "acme",
            "project": "api-gateway",
            "defaultSensitive": true,
            "defaultRequired": true,
            "keys": [
                key("STRIPE_SECRET_KEY", true),
                key("NEXT_PUBLIC_APP_URL", false),
                key("DATABASE_URL", true),
            ],
        })
    }

    fn key(name: &str, sensitive: bool) -> Value {
        json!({
            "name": name,
            "description": null,
            "type": { "name": "string", "raw": "string", "members": [], "constraints": {} },
            "required": true,
            "sensitive": sensitive,
            "default": "not-a-real-value",
            "example": null,
            "docs": null,
            "since": null,
            "deprecated": null,
            "rotate": null,
            "dynamic": null,
        })
    }

    fn guard(name: &str) -> Guard {
        load(&Empty, &Roots::new("/repo", None), name).unwrap()
    }

    fn rendered(name: &str, index: usize) -> String {
        let guard = guard(name);
        render(&guard, &guard.writes[index], &schema(), "9.9.9").unwrap()
    }

    #[test]
    fn every_built_in_guard_renders_the_format_it_declares() {
        for built_in in crate::BUILT_IN {
            let guard = guard(built_in.name);
            for (index, write) in guard.writes.iter().enumerate() {
                let out = rendered(built_in.name, index);
                match write.format {
                    crate::Format::Json => {
                        serde_json::from_str::<Value>(&out).unwrap_or_else(|e| {
                            panic!("{}/{} is not JSON: {e}", built_in.name, write.template)
                        });
                    }
                    crate::Format::Toml => {
                        toml::from_str::<toml::Value>(&out).unwrap_or_else(|e| {
                            panic!("{}/{} is not TOML: {e}", built_in.name, write.template)
                        });
                    }
                    crate::Format::Text => assert!(!out.is_empty()),
                }
            }
        }
    }

    #[test]
    fn the_deny_patterns_are_the_three_the_design_names_and_never_a_list() {
        let claude: Value = serde_json::from_str(&rendered("claude-code", 0)).unwrap();
        assert_eq!(
            claude["permissions"]["deny"],
            json!([
                "Read(./.env)",
                "Read(./.env.*)",
                "Read(~/.config/penv/local.key)"
            ])
        );
        assert_eq!(
            claude["sandbox"]["filesystem"]["denyRead"],
            json!(["./.env", "./.env.*", "~/.config/penv/local.key"])
        );
        let cursor: Value = serde_json::from_str(&rendered("cursor", 0)).unwrap();
        assert_eq!(
            cursor["permissions"]["deny"],
            json!([
                "Read(.env)",
                "Read(.env.*)",
                "Read(~/.config/penv/local.key)"
            ])
        );
        let codex: toml::Value = toml::from_str(&rendered("codex", 0)).unwrap();
        assert_eq!(
            codex["sandbox_workspace_write"]["deny_read"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        for name in ["claude-code", "cursor", "codex", "copilot"] {
            let out = rendered(name, 0);
            assert!(!out.contains(".env.local"), "{name} enumerates filenames");
            assert!(!out.contains(".env.schema"), "{name} names the schema");
        }
    }

    #[test]
    fn the_claude_code_hook_runs_the_binary_itself_for_every_tool() {
        let out: Value = serde_json::from_str(&rendered("claude-code", 0)).unwrap();
        let hook = &out["hooks"]["PreToolUse"][0];
        assert_eq!(hook["matcher"], ".*");
        assert_eq!(hook["hooks"][0]["command"], "penv hook claude-code");
    }

    #[test]
    fn the_cursor_hook_covers_a_shell_call_as_well_as_a_read() {
        let out: Value = serde_json::from_str(&rendered("cursor", 1)).unwrap();
        for event in ["beforeReadFile", "beforeShellExecution"] {
            let entry = &out["hooks"][event][0];
            assert_eq!(entry["command"], "penv hook cursor", "{event}");
            assert_eq!(entry["failClosed"], json!(true), "{event}");
        }
    }

    #[test]
    fn the_cursor_refusal_carries_the_reason_under_both_spellings() {
        let guard = guard("cursor");
        let out: Value =
            serde_json::from_str(&deny("cursor", &guard.hook.unwrap(), "no").unwrap()).unwrap();
        assert_eq!(out["permission"], "deny");
        assert_eq!(out["userMessage"], "no");
        assert_eq!(out["user_message"], "no");
    }

    #[test]
    fn a_harness_with_no_folder_answers_on_stderr_with_the_refusal_code() {
        let hook = crate::Hook::generic();
        assert_eq!(hook.deny.exit, 2);
        assert_eq!(
            deny("nano", &hook, "no reading .env").unwrap(),
            "no reading .env"
        );
    }

    #[test]
    fn a_deny_template_quotes_a_reason_that_would_break_the_json() {
        let guard = guard("claude-code");
        let rendered = deny("claude-code", &guard.hook.unwrap(), "a \"quoted\" reason").unwrap();
        let out: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(
            out["hookSpecificOutput"]["permissionDecisionReason"],
            "a \"quoted\" reason"
        );
    }

    #[test]
    fn the_user_block_masks_every_sensitive_key_and_no_other() {
        let out: Value = serde_json::from_str(&rendered("claude-code", 1)).unwrap();
        let vars = out["sandbox"]["credentials"]["envVars"].as_array().unwrap();
        let names: Vec<&str> = vars.iter().map(|v| v["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["STRIPE_SECRET_KEY", "DATABASE_URL"]);
        assert!(vars.iter().all(|v| v["mode"] == "mask"));
        assert_eq!(out["sandbox"]["credentials"]["injectHosts"], json!([]));
    }

    #[test]
    fn no_rendered_guard_carries_a_value() {
        for built_in in crate::BUILT_IN {
            let guard = guard(built_in.name);
            for index in 0..guard.writes.len() {
                let out = rendered(built_in.name, index);
                assert!(
                    !out.contains("not-a-real-value"),
                    "{} wrote a value into {}",
                    built_in.name,
                    guard.writes[index].path
                );
            }
        }
    }
}
