//! Agent detection and policy: pure functions over an environment map. Nothing
//! here reads the filesystem or the process table; the binary passes those in as
//! a closure and a trait so every rung of the ladder is testable from a map.

use std::collections::BTreeMap;

/// The environment as a value.
pub type Env = BTreeMap<String, String>;

/// Recorded when the session is not interactive. It tightens policy and never
/// names an agent.
pub const NON_INTERACTIVE: &str = "non-interactive";

/// Names `AGENT` may carry at rung 8. `AGENT=amp` is rung 1 and never reaches it.
pub const AGENT_ALLOWLIST: &[&str] = &[
    "aider",
    "amp",
    "claude",
    "claude-code",
    "cline",
    "codex",
    "copilot",
    "cursor",
    "devin",
    "gemini",
    "openhands",
    "roo",
    "windsurf",
];

/// The marker OpenHands leaves in the prompt of the shell it drives.
pub const OPENHANDS_PS1: &str = "###PS1JSON###";

/// The path Devin's runtime leaves behind.
pub const DEVIN_PATH: &str = "/opt/.devin";

/// How much the detection is worth. Vendor markers are High, third-party
/// conventions Medium, a walk of the process table Low.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    #[default]
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Confidence::Low => "low",
            Confidence::Medium => "medium",
            Confidence::High => "high",
        }
    }
}

/// Who is driving. `name` is canonical: the same vendor always answers to one
/// name however it announced itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub name: String,
    pub version: Option<String>,
    pub mode: Option<String>,
}

impl Agent {
    fn named(name: &str) -> Agent {
        Agent {
            name: name.to_string(),
            version: None,
            mode: None,
        }
    }
}

/// What the environment said, and how loudly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detection {
    pub agent: Option<Agent>,
    pub confidence: Confidence,
    pub session_id: Option<String>,
    pub markers: Vec<&'static str>,
}

impl Detection {
    pub fn is_agent(&self) -> bool {
        self.agent.is_some()
    }

    pub fn name(&self) -> Option<&str> {
        self.agent.as_ref().map(|a| a.name.as_str())
    }

    /// A non-interactive session, which tightens policy without naming an agent.
    pub fn tightened(&self) -> bool {
        self.markers.contains(&NON_INTERACTIVE)
    }
}

/// The parent chain, nearest first. Best effort: an empty answer is normal.
pub trait Ancestry {
    fn parent_names(&self) -> Vec<String>;
}

/// The answer when the platform cannot walk the process table.
pub struct NoAncestry;

impl Ancestry for NoAncestry {
    fn parent_names(&self) -> Vec<String> {
        Vec::new()
    }
}

static NO_ANCESTRY: NoAncestry = NoAncestry;
static NO_PATHS: fn(&str) -> bool = |_| false;

/// Everything detection is allowed to look at.
pub struct Context<'a> {
    pub env: &'a Env,
    pub tty: bool,
    pub git_editor_noninteractive: bool,
    pub path_exists: &'a dyn Fn(&str) -> bool,
    pub ancestry: &'a dyn Ancestry,
}

impl<'a> Context<'a> {
    pub fn new(env: &'a Env) -> Context<'a> {
        Context {
            env,
            tty: true,
            git_editor_noninteractive: false,
            path_exists: &NO_PATHS,
            ancestry: &NO_ANCESTRY,
        }
    }
}

/// The ordered ladder of section 6, then the process table, then the
/// non-interactive tightening that never names an agent.
pub fn detect(cx: &Context<'_>) -> Detection {
    let mut detection = ladder(cx).unwrap_or_default();
    detection.session_id = detection
        .agent
        .as_ref()
        .and_then(|agent| session_id(cx.env, &agent.name));
    if !cx.tty && cx.git_editor_noninteractive {
        detection.markers.push(NON_INTERACTIVE);
    }
    detection
}

fn ladder(cx: &Context<'_>) -> Option<Detection> {
    let env = cx.env;

    if value(env, "AGENT").is_some_and(|v| v.eq_ignore_ascii_case("amp")) {
        return Some(found("amp", Confidence::High, "AGENT=amp"));
    }
    if flag(env, "COPILOT_CLI") {
        return Some(found("copilot", Confidence::High, "COPILOT_CLI"));
    }
    if flag(env, "CLAUDE_CODE_CHILD_SESSION") {
        return Some(found(
            "claude-code",
            Confidence::High,
            "CLAUDE_CODE_CHILD_SESSION",
        ));
    }
    if flag(env, "CLAUDECODE") {
        return Some(found("claude-code", Confidence::High, "CLAUDECODE"));
    }
    if value(env, "CODEX_THREAD_ID").is_some() {
        return Some(found("codex", Confidence::High, "CODEX_THREAD_ID"));
    }
    if value(env, "CODEX_SESSION_ID").is_some() {
        return Some(found("codex", Confidence::High, "CODEX_SESSION_ID"));
    }
    if flag(env, "GEMINI_CLI") {
        return Some(found("gemini", Confidence::High, "GEMINI_CLI"));
    }
    if value(env, "CURSOR_SANDBOX").is_some() {
        return Some(found("cursor", Confidence::High, "CURSOR_SANDBOX"));
    }
    if value(env, "CURSOR_AGENT").is_some() {
        return Some(found("cursor", Confidence::High, "CURSOR_AGENT"));
    }

    if flag(env, "CLINE_ACTIVE") {
        return Some(found("cline", Confidence::High, "CLINE_ACTIVE"));
    }
    if flag(env, "ROO_ACTIVE") {
        return Some(found("roo", Confidence::High, "ROO_ACTIVE"));
    }
    if flag(env, "ROO_CLI_RUNTIME") {
        return Some(found("roo", Confidence::High, "ROO_CLI_RUNTIME"));
    }
    if value(env, "OR_APP_NAME").is_some_and(|v| v.eq_ignore_ascii_case("aider")) {
        return Some(found("aider", Confidence::High, "OR_APP_NAME=Aider"));
    }
    if value(env, "PS1").is_some_and(|v| v.contains(OPENHANDS_PS1)) {
        return Some(found("openhands", Confidence::Medium, "PS1"));
    }
    if let Some(raw) = value(env, "AI_AGENT")
        && let Some(agent) = parse_ai_agent(raw)
    {
        return Some(Detection {
            agent: Some(agent),
            confidence: Confidence::Medium,
            session_id: None,
            markers: vec!["AI_AGENT"],
        });
    }
    if let Some(raw) = value(env, "AGENT") {
        let lowered = raw.to_ascii_lowercase();
        if AGENT_ALLOWLIST.contains(&lowered.as_str()) {
            return Some(found(&canonical(&lowered), Confidence::Medium, "AGENT"));
        }
    }
    if (cx.path_exists)(DEVIN_PATH) {
        return Some(found("devin", Confidence::High, "/opt/.devin"));
    }

    let names = cx.ancestry.parent_names();
    let (name, marker) = from_parent_names(&names)?;
    Some(found(name, Confidence::Low, marker))
}

fn found(name: &str, confidence: Confidence, marker: &'static str) -> Detection {
    Detection {
        agent: Some(Agent::named(name)),
        confidence,
        session_id: None,
        markers: vec![marker],
    }
}

fn value<'a>(env: &'a Env, key: &str) -> Option<&'a str> {
    env.get(key).map(String::as_str).filter(|v| !v.is_empty())
}

/// A vendor boolean: set, and not switched off.
fn flag(env: &Env, key: &str) -> bool {
    value(env, key)
        .is_some_and(|v| !v.eq_ignore_ascii_case("0") && !v.eq_ignore_ascii_case("false"))
}

/// `AI_AGENT` is written both as `name_version_mode` and as `name@version`.
fn parse_ai_agent(raw: &str) -> Option<Agent> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (name, version, mode) = match raw.split_once('@') {
        Some((name, version)) => (name, non_empty(version), None),
        None => {
            let mut parts = raw.split('_');
            let name = parts.next().unwrap_or_default();
            let version = parts.next().and_then(non_empty);
            let mode = parts.collect::<Vec<&str>>().join("_");
            (name, version, non_empty_owned(mode))
        }
    };
    let name = canonical(&name.trim().to_ascii_lowercase());
    if name.is_empty() {
        return None;
    }
    Some(Agent {
        name,
        version: version.map(str::to_string),
        mode,
    })
}

fn non_empty(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn non_empty_owned(s: String) -> Option<String> {
    non_empty(&s).map(str::to_string)
}

/// One vendor, one name, however it announced itself.
fn canonical(name: &str) -> String {
    match name {
        "claude" | "claudecode" | "claude_code" => "claude-code",
        "github-copilot" | "copilot-cli" => "copilot",
        "cursor-agent" => "cursor",
        other => other,
    }
    .to_string()
}

/// The id every cloud request is stamped with, by whoever is driving.
fn session_id(env: &Env, name: &str) -> Option<String> {
    let key = match name {
        "claude-code" => "CLAUDE_CODE_SESSION_ID",
        "codex" => {
            return value(env, "CODEX_THREAD_ID")
                .or_else(|| value(env, "CODEX_SESSION_ID"))
                .map(str::to_string);
        }
        "cursor" => "CURSOR_TRACE_ID",
        "amp" => {
            return value(env, "AMP_CURRENT_THREAD_ID")
                .or_else(|| value(env, "AGENT_THREAD_ID"))
                .map(str::to_string);
        }
        "copilot" => "COPILOT_AGENT_SESSION_ID",
        _ => return None,
    };
    value(env, key).map(str::to_string)
}

/// Executable names worth recognising in the parent chain, and the marker each
/// one records.
const KNOWN_PARENTS: &[(&str, &str, &str)] = &[
    ("claude", "claude-code", "ancestry:claude"),
    ("codex", "codex", "ancestry:codex"),
    ("cursor", "cursor", "ancestry:cursor"),
    ("cursor-agent", "cursor", "ancestry:cursor-agent"),
    ("windsurf", "windsurf", "ancestry:windsurf"),
    ("gemini", "gemini", "ancestry:gemini"),
    ("aider", "aider", "ancestry:aider"),
    ("amp", "amp", "ancestry:amp"),
    ("copilot", "copilot", "ancestry:copilot"),
    ("cline", "cline", "ancestry:cline"),
];

/// The nearest recognised parent, by executable name.
pub fn from_parent_names(names: &[String]) -> Option<(&'static str, &'static str)> {
    names.iter().find_map(|raw| {
        let stem = executable_stem(raw);
        KNOWN_PARENTS
            .iter()
            .find(|(exe, _, _)| *exe == stem)
            .map(|(_, name, marker)| (*name, *marker))
    })
}

fn executable_stem(raw: &str) -> String {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    match base.rsplit_once('.') {
        Some((stem, "exe" | "cmd" | "bat" | "com")) => stem.to_string(),
        _ => base,
    }
}

/// True when `GIT_EDITOR` names something that cannot prompt anybody.
pub fn git_editor_noninteractive(env: &Env) -> bool {
    let Some(raw) = value(env, "GIT_EDITOR") else {
        return false;
    };
    let first = raw.split_whitespace().next().unwrap_or_default();
    matches!(
        executable_stem(first).as_str(),
        "true" | ":" | "false" | "cat" | "echo" | "test"
    )
}

/// The credential lifetime a person gets.
pub const HUMAN_CREDENTIAL_TTL_SECS: u64 = 900;
/// The credential lifetime an agent session gets.
pub const AGENT_CREDENTIAL_TTL_SECS: u64 = 300;

/// What the session may do. Detection is advisory; this is what acts on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub json: bool,
    pub mask: bool,
    pub reveal_allowed: bool,
    pub pull_allowed: bool,
    pub credential_ttl_secs: u64,
}

impl Policy {
    pub const fn human() -> Policy {
        Policy {
            json: false,
            mask: false,
            reveal_allowed: true,
            pull_allowed: true,
            credential_ttl_secs: HUMAN_CREDENTIAL_TTL_SECS,
        }
    }

    pub const fn agent() -> Policy {
        Policy {
            json: true,
            mask: true,
            reveal_allowed: false,
            pull_allowed: false,
            credential_ttl_secs: AGENT_CREDENTIAL_TTL_SECS,
        }
    }

    /// Whether a credential minted at `minted_at` may still be used at `now`:
    /// never past this session's lifetime, whatever the server granted.
    pub const fn reuses(&self, minted_at: u64, now: u64) -> bool {
        now >= minted_at && now - minted_at < self.credential_ttl_secs
    }

    /// An agent, detected or declared, takes the agent policy. A non-interactive
    /// session only turns masking on.
    pub fn for_(detection: &Detection, explicit_agent_flag: bool) -> Policy {
        if explicit_agent_flag || detection.is_agent() {
            return Policy::agent();
        }
        Policy {
            mask: detection.tightened(),
            ..Policy::human()
        }
    }
}
