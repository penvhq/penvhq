//! The hook matcher, as a table. It is a pure function of the request, so the
//! whole policy is readable here.

use penv::commands::hook::{
    DUMPS_ENV, Decision, PULLS_VALUES, READS_ENV, Request, decide, extract,
};
use penv_guards::Payload;

fn on(command: &str) -> Decision {
    decide(&Request::command(command))
}

fn denied(command: &str, reason: &'static str) {
    assert_eq!(on(command), Decision::Deny(reason), "{command}");
}

fn allowed(command: &str) {
    assert_eq!(on(command), Decision::Allow, "{command}");
}

#[test]
fn reading_a_value_file_is_refused() {
    for command in [
        "cat .env",
        "cat ./.env",
        "cat /repo/.env.production",
        "type .\\.env.local",
        "grep DATABASE_URL .env",
        "head -n 5 .env.staging.local",
        "cp .env /tmp/x",
        "sed -n 1p apps/api/.env",
        "cat --file=.env",
        "cat \".env\"",
        "echo hi > .env",
    ] {
        denied(command, READS_ENV);
    }
}

#[test]
fn a_glob_that_could_name_a_value_file_counts_as_naming_it() {
    for command in [
        "cat .env*",
        "cat .en?",
        "cat *.env",
        "cat .e*",
        "grep -r sk_ .env.*",
        "rg key apps/api/.env*",
        "cat .[e]nv",
    ] {
        denied(command, READS_ENV);
    }
}

#[test]
fn a_glob_that_names_nothing_of_ours_is_left_alone() {
    for command in [
        "ls *",
        "rm -rf build/*",
        "prettier --write src/**/*.ts",
        "cat .env.schema",
        "cat *.json",
    ] {
        allowed(command);
    }
}

#[test]
fn the_schema_stays_readable_because_it_holds_no_values() {
    for command in [
        "cat .env.schema",
        "grep PORT .env.schema",
        "cat ./.env.schema",
        "penv check",
        "penv ls",
        "penv gen ts",
        "cat package.json",
        "node scripts/build.js",
    ] {
        allowed(command);
    }
}

#[test]
fn a_value_file_in_another_case_is_the_same_file() {
    for command in [
        "cat .ENV",
        "cat .Env.Local",
        "less config/.ENV.PRODUCTION",
        "cat .E*",
    ] {
        denied(command, READS_ENV);
    }
    let read = Request::path("/repo/.Env");
    assert_eq!(decide(&read), Decision::Deny(READS_ENV));
    for command in ["cat .ENV.SCHEMA", "cat .Env.Schema"] {
        allowed(command);
    }
}

#[test]
fn every_gemini_before_tool_payload_names_what_it_touches() {
    for (tool, input) in [
        ("read_file", r#"{"file_path":"/repo/.env"}"#),
        ("read_file", r#"{"absolute_path":"/repo/.env.local"}"#),
        (
            "read_many_files",
            r#"{"paths":["README.md","config/.env.production"]}"#,
        ),
        (
            "read_many_files",
            r#"{"paths":["src"],"include":[".env*"]}"#,
        ),
        ("glob", r#"{"pattern":"**/.env*"}"#),
        (
            "search_file_content",
            r#"{"pattern":"KEY","path":".env","include":"*"}"#,
        ),
        ("run_shell_command", r#"{"command":"cat .env"}"#),
        (
            "search_file_content",
            r#"{"pattern":"KEY","include":".env*"}"#,
        ),
    ] {
        let payload = format!(
            r#"{{"session_id":"s","hook_event_name":"BeforeTool","cwd":"/repo","tool_name":"{tool}","tool_input":{input}}}"#
        );
        assert_eq!(
            decide(&extract(Payload::Generic, &payload)),
            Decision::Deny(READS_ENV),
            "{tool} {input}"
        );
    }
    let fine = r#"{"hook_event_name":"BeforeTool","tool_name":"read_many_files","tool_input":{"paths":["README.md",".env.schema"]}}"#;
    assert_eq!(decide(&extract(Payload::Generic, fine)), Decision::Allow);
}

#[test]
fn a_command_line_or_a_file_pattern_reaches_the_matcher() {
    for payload in [
        r#"{"agent_action_name":"pre_run_command","tool_info":{"command_line":"cat .env","cwd":"/repo"}}"#,
        r#"{"tool":"search_files","parameters":{"path":".","regex":"KEY","file_pattern":".env*"}}"#,
    ] {
        assert_eq!(
            decide(&extract(Payload::Generic, payload)),
            Decision::Deny(READS_ENV),
            "{payload}"
        );
    }
}

#[test]
fn dumping_the_environment_is_refused() {
    for command in [
        "printenv",
        "printenv | sort",
        "printenv DATABASE_URL",
        "env",
        "env -0",
        "set",
        "Get-ChildItem Env:",
        "gci env:",
        "ls Env:",
        "node -e \"console.log(process.env)\"",
        "node -p process.env",
        "python3 -c \"import os; print(os.environ)\"",
    ] {
        denied(command, DUMPS_ENV);
    }
}

/// A shell handed a command is that command; without this, every matcher above
/// reads only the word `bash`.
#[test]
fn a_shell_running_something_else_is_decided_on_what_it_runs() {
    for (command, reason) in [
        ("bash -c printenv", DUMPS_ENV),
        ("sh -c \"printenv\"", DUMPS_ENV),
        ("pwsh -Command Get-ChildItem Env:", DUMPS_ENV),
        ("powershell -Command \"gci env:\"", DUMPS_ENV),
        ("cmd /c type .env", READS_ENV),
        ("cmd.exe /C \"type .env.local\"", READS_ENV),
        ("/bin/sh -c \"cat .env\"", READS_ENV),
        ("zsh -c \"penv pull --env production\"", PULLS_VALUES),
        ("bash -c \"sh -c printenv\"", DUMPS_ENV),
    ] {
        denied(command, reason);
    }
}

#[test]
fn a_shell_running_an_ordinary_command_still_runs_it() {
    for command in [
        "bash -c \"npm run build\"",
        "sh -c 'echo hi'",
        "cmd /c dir src",
        "bash",
        "pwsh -Command",
    ] {
        allowed(command);
    }
}

#[test]
fn running_a_program_with_the_environment_is_not_dumping_it() {
    for command in [
        "env NODE_ENV=test npm run build",
        "set -e",
        "ls src",
        "grep -e process.env -r src",
        "npm set registry https://example.test",
    ] {
        allowed(command);
    }
}

#[test]
fn pull_is_refused_and_reveal_goes_to_its_approval() {
    for command in [
        "penv pull",
        "penv --json pull",
        "/usr/local/bin/penv pull --env staging",
    ] {
        denied(command, PULLS_VALUES);
    }
    for command in [
        "penv reveal STRIPE_SECRET_KEY",
        "/usr/local/bin/penv reveal PORT --approval apr_123",
        "penv push",
    ] {
        allowed(command);
    }
}

#[test]
fn a_read_of_a_value_file_by_path_is_refused() {
    assert_eq!(
        decide(&Request::path("/repo/.env.local")),
        Decision::Deny(READS_ENV)
    );
    assert_eq!(decide(&Request::path("/repo/.env.schema")), Decision::Allow);
    assert_eq!(decide(&Request::default()), Decision::Allow);
}

#[test]
fn one_refusal_in_a_chain_refuses_the_chain() {
    denied("npm run build && cat .env", READS_ENV);
    denied("mkdir x; printenv", DUMPS_ENV);
    denied("ls | cat .env", READS_ENV);
}

#[test]
fn the_claude_code_payload_reaches_the_matcher() {
    let request = extract(
        Payload::ClaudeCode,
        r#"{"tool_name":"Bash","tool_input":{"command":"cat .env"}}"#,
    );
    assert_eq!(request.command.as_deref(), Some("cat .env"));
    assert_eq!(decide(&request), Decision::Deny(READS_ENV));
}

/// The matcher is `.*`, so Read, Grep and Glob arrive here too and each names
/// what it is about to touch in its own field.
#[test]
fn every_claude_code_tool_shape_names_what_it_touches() {
    for payload in [
        r#"{"tool_name":"Read","tool_input":{"file_path":"/repo/.env"}}"#,
        r#"{"tool_name":"Grep","tool_input":{"pattern":"sk_","path":"/repo/.env"}}"#,
        r#"{"tool_name":"Glob","tool_input":{"pattern":".env*"}}"#,
        r#"{"tool_name":"mcp__files__read","tool_input":{"path":"apps/api/.env.local"}}"#,
        r#"{"tool_name":"Bash","tool_input":{"command":"bash -c printenv"}}"#,
    ] {
        assert!(
            matches!(
                decide(&extract(Payload::ClaudeCode, payload)),
                Decision::Deny(_)
            ),
            "{payload}"
        );
    }
    assert_eq!(
        decide(&extract(
            Payload::ClaudeCode,
            r#"{"tool_name":"Read","tool_input":{"file_path":"/repo/.env.schema"}}"#
        )),
        Decision::Allow
    );
}

#[test]
fn the_cursor_payload_reaches_the_matcher_for_a_read_and_for_a_shell_call() {
    let read = extract(
        Payload::Cursor,
        r#"{"hook_event_name":"beforeReadFile","file_path":"/repo/.env"}"#,
    );
    assert_eq!(read.path.as_deref(), Some("/repo/.env"));
    assert_eq!(decide(&read), Decision::Deny(READS_ENV));

    let shell = extract(
        Payload::Cursor,
        r#"{"hook_event_name":"beforeShellExecution","command":"printenv"}"#,
    );
    assert_eq!(shell.command.as_deref(), Some("printenv"));
    assert_eq!(decide(&shell), Decision::Deny(DUMPS_ENV));
}

#[test]
fn an_undocumented_payload_is_read_for_what_it_has() {
    assert_eq!(
        decide(&extract(Payload::Generic, r#"{"args":{"cmd":"printenv"}}"#)),
        Decision::Deny(DUMPS_ENV)
    );
    assert_eq!(
        decide(&extract(Payload::Generic, "cat .env")),
        Decision::Deny(READS_ENV),
        "plain text falls back to the command"
    );
    assert_eq!(decide(&extract(Payload::Generic, "")), Decision::Allow);
    assert_eq!(
        decide(&extract(Payload::Generic, "{not json but cat .env")),
        Decision::Deny(READS_ENV),
        "a payload that will not parse is still read for what it names"
    );
}

/// The matcher reads a command as text. These get through, and the design says
/// so: detection changes defaults and friction, it is never the last line of
/// defence. The sandbox deny rules and short-lived cloud values are.
#[test]
fn the_known_evasions_are_known() {
    for command in [
        "echo Y2F0IC5lbnYK | base64 -d | sh",
        "n=env; cat \".$n\"",
        "python3 -c \"print(open('.en' + 'v').read())\"",
        "node -e \"console.log(require('fs').readFileSync('.env','utf8'))\"",
    ] {
        allowed(command);
    }
}

#[test]
fn reading_penvs_local_key_file_is_refused() {
    for command in [
        "cat ~/.config/penv/local.key",
        "cat /home/me/.config/penv/local.key",
        "type %APPDATA%\\penv\\local.key",
        "cp \"$XDG_CONFIG_HOME/penv/local.key\" /tmp/k",
    ] {
        denied(command, READS_ENV);
    }
    let read = Request {
        path: Some("/Users/me/.config/penv/local.key".into()),
        ..Request::command("")
    };
    assert_eq!(decide(&read), Decision::Deny(READS_ENV));
    // Another program's key of the same name is not penv's.
    allowed("cat certs/local.key");
}
