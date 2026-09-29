use penv_agent::{
    AGENT_CREDENTIAL_TTL_SECS, Ancestry, Confidence, Context, Detection, Env,
    HUMAN_CREDENTIAL_TTL_SECS, NON_INTERACTIVE, Policy, detect, from_parent_names,
    git_editor_noninteractive,
};

fn env(pairs: &[(&str, &str)]) -> Env {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// One row of the ladder table: the environment, the name it must answer with,
/// and how much that answer is worth.
type Rung = (
    &'static [(&'static str, &'static str)],
    &'static str,
    Confidence,
);

fn detected(pairs: &[(&str, &str)]) -> Detection {
    let env = env(pairs);
    detect(&Context::new(&env))
}

struct Parents(Vec<String>);

impl Ancestry for Parents {
    fn parent_names(&self) -> Vec<String> {
        self.0.clone()
    }
}

#[test]
fn every_rung_of_the_ladder_names_its_agent() {
    let cases: &[Rung] = &[
        (&[("AGENT", "amp")], "amp", Confidence::High),
        (&[("COPILOT_CLI", "1")], "copilot", Confidence::High),
        (
            &[("CLAUDE_CODE_CHILD_SESSION", "1")],
            "claude-code",
            Confidence::High,
        ),
        (&[("CLAUDECODE", "1")], "claude-code", Confidence::High),
        (&[("CODEX_THREAD_ID", "th_1")], "codex", Confidence::High),
        (&[("CODEX_SESSION_ID", "se_1")], "codex", Confidence::High),
        (&[("GEMINI_CLI", "1")], "gemini", Confidence::High),
        (&[("CURSOR_SANDBOX", "1")], "cursor", Confidence::High),
        (&[("CURSOR_AGENT", "1")], "cursor", Confidence::High),
        (&[("CLINE_ACTIVE", "1")], "cline", Confidence::High),
        (&[("ROO_ACTIVE", "1")], "roo", Confidence::High),
        (&[("ROO_CLI_RUNTIME", "1")], "roo", Confidence::High),
        (&[("OR_APP_NAME", "Aider")], "aider", Confidence::High),
        (
            &[("PS1", "###PS1JSON###{\"pwd\":\"/w\"}")],
            "openhands",
            Confidence::Medium,
        ),
        (
            &[("AI_AGENT", "cursor_1.2.3_agent")],
            "cursor",
            Confidence::Medium,
        ),
        (&[("AI_AGENT", "aider@0.50")], "aider", Confidence::Medium),
        (&[("AGENT", "windsurf")], "windsurf", Confidence::Medium),
    ];

    for (pairs, name, confidence) in cases {
        let detection = detected(pairs);
        assert_eq!(detection.name(), Some(*name), "{pairs:?}");
        assert_eq!(detection.confidence, *confidence, "{pairs:?}");
        assert!(!detection.markers.is_empty(), "{pairs:?}");
    }
}

#[test]
fn the_ladder_settles_the_collisions() {
    // Copilot CLI sets Claude Code's project variable; rung 2 answers first.
    assert_eq!(
        detected(&[("COPILOT_CLI", "1"), ("CLAUDE_PROJECT_DIR", "/w")]).name(),
        Some("copilot")
    );
    // Amp announces itself as Claude Code as well; rung 1 answers first.
    assert_eq!(
        detected(&[("AGENT", "amp"), ("CLAUDECODE", "1")]).name(),
        Some("amp")
    );
    // A child session is still Claude Code, and beats the plain marker.
    let child = detected(&[("CLAUDE_CODE_CHILD_SESSION", "1"), ("CLAUDECODE", "1")]);
    assert_eq!(child.name(), Some("claude-code"));
    assert_eq!(child.markers, ["CLAUDE_CODE_CHILD_SESSION"]);
    // Codex before Gemini before Cursor, all set at once.
    assert_eq!(
        detected(&[
            ("CODEX_THREAD_ID", "th_1"),
            ("GEMINI_CLI", "1"),
            ("CURSOR_AGENT", "1"),
        ])
        .name(),
        Some("codex")
    );
}

#[test]
fn claude_project_dir_alone_is_not_an_agent() {
    assert_eq!(detected(&[("CLAUDE_PROJECT_DIR", "/w")]).agent, None);
}

#[test]
fn ci_does_not_decide_anything() {
    assert_eq!(
        detected(&[("CI", "1"), ("CURSOR_AGENT", "1")]).name(),
        Some("cursor")
    );
    assert_eq!(detected(&[("CI", "1")]).agent, None);
    assert_eq!(detected(&[("CI", "true"), ("TERM", "dumb")]).agent, None);
}

#[test]
fn a_missing_marker_never_loosens_anything() {
    let empty = detected(&[]);
    assert_eq!(empty.agent, None);
    assert_eq!(empty.confidence, Confidence::Low);
    assert!(empty.markers.is_empty());
    assert_eq!(Policy::for_(&empty, false), Policy::human());

    // Switching a marker off is the same as not setting it.
    for pairs in [
        &[("COPILOT_CLI", "0")][..],
        &[("CLAUDECODE", "false")][..],
        &[("GEMINI_CLI", "")][..],
        &[("AGENT", "")][..],
        &[("AGENT", "make")][..],
        &[("AI_AGENT", "")][..],
        &[("PS1", "$ ")][..],
        &[("OR_APP_NAME", "OpenRouter")][..],
    ] {
        assert_eq!(detected(pairs).agent, None, "{pairs:?}");
    }
}

#[test]
fn the_session_id_comes_from_the_vendors_own_variable() {
    let cases: &[(&[(&str, &str)], &str)] = &[
        (
            &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "cs_1")],
            "cs_1",
        ),
        (&[("CODEX_THREAD_ID", "th_1")], "th_1"),
        (
            &[("CODEX_SESSION_ID", "se_1"), ("CODEX_THREAD_ID", "th_1")],
            "th_1",
        ),
        (
            &[("CURSOR_AGENT", "1"), ("CURSOR_TRACE_ID", "tr_1")],
            "tr_1",
        ),
        (&[("AGENT", "amp"), ("AMP_CURRENT_THREAD_ID", "T-1")], "T-1"),
        (&[("AGENT", "amp"), ("AGENT_THREAD_ID", "T-2")], "T-2"),
        (
            &[("COPILOT_CLI", "1"), ("COPILOT_AGENT_SESSION_ID", "co_1")],
            "co_1",
        ),
    ];
    for (pairs, id) in cases {
        assert_eq!(
            detected(pairs).session_id.as_deref(),
            Some(*id),
            "{pairs:?}"
        );
    }
    assert_eq!(detected(&[("CLINE_ACTIVE", "1")]).session_id, None);
}

#[test]
fn ai_agent_is_read_in_both_dialects() {
    let underscored = detected(&[("AI_AGENT", "claude_1.4.0_review")]);
    let agent = underscored.agent.unwrap();
    assert_eq!(agent.name, "claude-code");
    assert_eq!(agent.version.as_deref(), Some("1.4.0"));
    assert_eq!(agent.mode.as_deref(), Some("review"));

    let at = detected(&[("AI_AGENT", "gemini@2.0")]).agent.unwrap();
    assert_eq!(at.name, "gemini");
    assert_eq!(at.version.as_deref(), Some("2.0"));
    assert_eq!(at.mode, None);

    let bare = detected(&[("AI_AGENT", "openhands")]).agent.unwrap();
    assert_eq!(bare.name, "openhands");
    assert_eq!(bare.version, None);
}

#[test]
fn devin_is_a_path_the_caller_looks_up() {
    let env = env(&[]);
    let mut cx = Context::new(&env);
    let exists = |path: &str| path == "/opt/.devin";
    cx.path_exists = &exists;
    let detection = detect(&cx);
    assert_eq!(detection.name(), Some("devin"));
    assert_eq!(detection.confidence, Confidence::High);
}

#[test]
fn the_process_table_is_the_last_rung_and_may_be_empty() {
    let env = env(&[]);
    let parents = Parents(vec!["node".into(), "C:\\bin\\claude.EXE".into()]);
    let mut cx = Context::new(&env);
    cx.ancestry = &parents;
    let detection = detect(&cx);
    assert_eq!(detection.name(), Some("claude-code"));
    assert_eq!(detection.confidence, Confidence::Low);

    let none = Parents(vec!["bash".into(), "sshd".into()]);
    cx.ancestry = &none;
    assert_eq!(detect(&cx).agent, None);

    assert_eq!(from_parent_names(&[]), None);
    assert_eq!(
        from_parent_names(&["/usr/local/bin/codex".to_string()]).map(|(n, _)| n),
        Some("codex")
    );
}

#[test]
fn an_environment_marker_beats_the_process_table() {
    let env = env(&[("CLAUDECODE", "1")]);
    let parents = Parents(vec!["aider".into()]);
    let mut cx = Context::new(&env);
    cx.ancestry = &parents;
    assert_eq!(detect(&cx).name(), Some("claude-code"));
}

#[test]
fn a_non_interactive_session_tightens_and_names_nobody() {
    let env = env(&[]);
    let mut cx = Context::new(&env);
    cx.tty = false;
    cx.git_editor_noninteractive = true;
    let detection = detect(&cx);

    assert_eq!(detection.agent, None);
    assert!(detection.tightened());
    assert!(detection.markers.contains(&NON_INTERACTIVE));

    let policy = Policy::for_(&detection, false);
    assert!(policy.mask);
    assert!(!policy.json);
    assert!(policy.reveal_allowed);
    assert!(policy.pull_allowed);
    assert_eq!(policy.credential_ttl_secs, HUMAN_CREDENTIAL_TTL_SECS);
}

#[test]
fn one_half_of_the_tightening_is_not_enough() {
    let env = env(&[]);
    for (tty, editor) in [(false, false), (true, true), (true, false)] {
        let mut cx = Context::new(&env);
        cx.tty = tty;
        cx.git_editor_noninteractive = editor;
        assert!(!detect(&cx).tightened(), "{tty} {editor}");
    }
}

#[test]
fn git_editor_is_read_for_whether_it_can_prompt() {
    assert!(git_editor_noninteractive(&env(&[("GIT_EDITOR", "true")])));
    assert!(git_editor_noninteractive(&env(&[(
        "GIT_EDITOR",
        "/bin/true"
    )])));
    assert!(git_editor_noninteractive(&env(&[("GIT_EDITOR", ":")])));
    assert!(!git_editor_noninteractive(&env(&[("GIT_EDITOR", "vim")])));
    assert!(!git_editor_noninteractive(&env(&[(
        "GIT_EDITOR",
        "code --wait"
    )])));
    assert!(!git_editor_noninteractive(&env(&[])));
}

#[test]
fn an_agent_session_flips_the_defaults() {
    let detection = detected(&[("CLAUDECODE", "1")]);
    let policy = Policy::for_(&detection, false);
    assert_eq!(policy, Policy::agent());
    assert!(policy.json);
    assert!(policy.mask);
    assert!(!policy.reveal_allowed);
    assert!(!policy.pull_allowed);
    assert_eq!(policy.credential_ttl_secs, AGENT_CREDENTIAL_TTL_SECS);
    const { assert!(AGENT_CREDENTIAL_TTL_SECS < HUMAN_CREDENTIAL_TTL_SECS) };
}

#[test]
fn the_agent_flag_is_as_good_as_a_marker() {
    let nobody = detected(&[]);
    assert_eq!(Policy::for_(&nobody, true), Policy::agent());
    assert_eq!(Policy::for_(&nobody, false), Policy::human());
}

#[test]
fn a_credential_is_reused_only_inside_the_session_s_lifetime() {
    let agent = Policy::agent();
    assert!(agent.reuses(1_000, 1_000));
    assert!(agent.reuses(1_000, 1_299));
    assert!(!agent.reuses(1_000, 1_300), "five minutes, not fifteen");
    assert!(
        !agent.reuses(1_000, 999),
        "minted after now is a clock that moved back"
    );
    assert!(Policy::human().reuses(1_000, 1_899));
    assert!(!Policy::human().reuses(1_000, 1_900));
}
