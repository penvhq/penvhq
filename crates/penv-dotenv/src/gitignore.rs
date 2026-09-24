/// The lines that keep a `.env` out of the repository. `.env.*` would swallow
/// `.env.schema`, the one penv file that is committed, so the negation is part
/// of the block.
pub const IGNORE_LINES: [&str; 3] = [".env", ".env.*", "!.env.schema"];

const KEEP_SCHEMA: &str = "!.env.schema";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitignoreUpdate {
    pub content: String,
    pub added: Vec<&'static str>,
}

impl GitignoreUpdate {
    pub fn changed(&self) -> bool {
        !self.added.is_empty()
    }
}

/// Add the ignore lines that are missing. Running it again adds nothing.
pub fn ensure_ignored(existing: &str) -> GitignoreUpdate {
    let present: Vec<&str> = existing
        .lines()
        .map(|l| l.trim().trim_start_matches('/'))
        .collect();
    let mut added: Vec<&'static str> = IGNORE_LINES
        .iter()
        .copied()
        .filter(|want| !present.contains(want))
        .collect();
    // The last matching line wins in git, so the negation has to come after
    // every pattern that reaches `.env.schema`, the ones added here included.
    let swallows = |line: &str| matches!(line, ".env.*" | ".env*");
    let last_negation = present.iter().rposition(|l| *l == KEEP_SCHEMA);
    let last_swallow = present.iter().rposition(|l| swallows(l));
    let buried = match (last_negation, last_swallow) {
        (Some(negation), Some(swallow)) => negation < swallow,
        _ => false,
    };
    if !added.contains(&KEEP_SCHEMA) && (buried || added.iter().any(|l| swallows(l))) {
        added.push(KEEP_SCHEMA);
    }

    let mut content = existing.to_string();
    if !added.is_empty() {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        if !content.is_empty() {
            content.push('\n');
        }
        content.push_str("# penv keeps values out of the repository\n");
        for line in &added {
            content.push_str(line);
            content.push('\n');
        }
    }
    GitignoreUpdate { content, added }
}
