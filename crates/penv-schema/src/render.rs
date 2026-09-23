use std::fmt::Write as _;

use crate::ir::{Key, RequiredDefault, Schema, quote};

/// Write a schema back out canonically. Parsing the result yields an equal `Schema`.
pub fn render(schema: &Schema) -> String {
    let mut out = String::new();
    let mut header = String::from("#");
    if let (Some(org), Some(project)) = (&schema.org, &schema.project) {
        let _ = write!(header, " @penv={org}/{project}");
    }
    // The version lives in `.penv/config.toml`; a file says it only when it is
    // not the one penv writes, because varlock rejects `@schema`.
    if schema.schema_version != crate::ir::SCHEMA_VERSION {
        let _ = write!(header, " @schema={}", schema.schema_version);
    }
    if header != "#" {
        out.push_str(&header);
        out.push('\n');
    }
    if !schema.default_sensitive {
        out.push_str("# @defaultSensitive=false\n");
    }
    match schema.default_required {
        RequiredDefault::Yes => {}
        RequiredDefault::No => out.push_str("# @defaultRequired=false\n"),
        RequiredDefault::Infer => out.push_str("# @defaultRequired=infer\n"),
    }
    if let Some(key) = &schema.current_env {
        let _ = writeln!(out, "# @currentEnv=${key}");
    }
    for assert in &schema.asserts {
        let message = assert.message.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = writeln!(out, "# @assert({}, \"{message}\")", assert.expr);
    }
    for import in &schema.imports {
        let args: Vec<String> = std::iter::once(&import.path)
            .chain(&import.keys)
            .map(|a| quote(a))
            .collect();
        let _ = writeln!(out, "# @import({})", args.join(", "));
    }

    for key in &schema.keys {
        out.push('\n');
        out.push_str(&render_key(key));
    }
    // No header line means no blank line before the first key.
    out.trim_start_matches('\n').to_string()
}

/// One key's block, the way `render` writes it. `set` appends it to a file it
/// must not otherwise reformat.
pub fn render_key(key: &Key) -> String {
    let mut out = String::new();
    let out = &mut out;
    if let Some(description) = &key.description {
        let _ = writeln!(out, "# {description}");
    }

    let mut decorators = vec![format!("@type={}", key.ty)];
    match (key.required_decorator, &key.required_in) {
        (Some(true), _) => decorators.push("@required".into()),
        (Some(false), _) => decorators.push("@optional".into()),
        (None, Some((envs, required))) => decorators.push(format!(
            "@{}=forEnv({})",
            if *required { "required" } else { "optional" },
            envs.join(",")
        )),
        (None, None) => {}
    }
    match key.sensitive_decorator {
        Some(true) => decorators.push("@sensitive".into()),
        Some(false) => decorators.push("@sensitive=false".into()),
        None => {}
    }
    if let Some(v) = &key.example {
        decorators.push(format!("@example={}", quote(v)));
    }
    if let Some(v) = &key.docs {
        // varlock accepts only the call form.
        decorators.push(format!("@docs({})", quote(v)));
    }
    if let Some(v) = &key.deprecated {
        if v.is_empty() {
            decorators.push("@deprecated".into());
        } else {
            decorators.push(format!("@deprecated={}", quote(v)));
        }
    }
    match key.hosts.as_slice() {
        [] => {}
        [one] => decorators.push(format!("@hosts={one}")),
        many => decorators.push(format!(
            "@hosts({})",
            many.iter()
                .map(|h| if h.starts_with('*') {
                    format!("\"{h}\"")
                } else {
                    h.clone()
                })
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
    if let Some(v) = &key.rotate {
        decorators.push(format!("@rotate={v}"));
    }
    match key.dynamic {
        Some(true) => decorators.push("@dynamic".into()),
        Some(false) => decorators.push("@static".into()),
        None => {}
    }

    let _ = writeln!(out, "# {}", decorators.join(" "));
    let _ = writeln!(
        out,
        "{}={}",
        key.name,
        key.default
            .as_deref()
            .map(|v| default_text(v, key.default_expr))
            .unwrap_or_default()
    );
    std::mem::take(out)
}

/// A computed default is written bare so it stays computed; a literal holding a
/// `$` is single-quoted so it stays literal.
fn default_text(v: &str, computed: bool) -> String {
    if computed {
        return v.to_string();
    }
    if crate::resolve::is_expression(v) && !v.contains('\'') {
        return format!("'{v}'");
    }
    quote(v)
}

/// Point the file at `org/project` and change nothing else in it: comments,
/// varlock-only decorators and key blocks stay as written. Only the first
/// comment block, the header, is looked at, so a key's comment that mentions
/// `@penv=` is never rewritten.
pub fn set_header(source: &str, org: &str, project: &str) -> String {
    const HEADER: [&str; 13] = [
        "@penv=",
        "@schema=",
        "@currentEnv",
        "@defaultSensitive",
        "@defaultRequired",
        "@import(",
        "@generateTypes",
        "@plugin",
        "@redactLogs",
        "@preventLeaks",
        "@envFlag",
        "@setValuesBulk",
        "@disable",
    ];
    let mut token = format!("@penv={org}/{project}");
    let mut lines: Vec<String> = source.split('\n').map(str::to_string).collect();
    let block: Vec<usize> = lines
        .iter()
        .enumerate()
        .skip_while(|(_, l)| l.trim().is_empty())
        .take_while(|(_, l)| l.trim_start().starts_with('#'))
        .map(|(i, _)| i)
        .collect();
    // A first block sitting directly on a key belongs to it unless it carries a
    // header decorator, the parser's own rule.
    let is_header = block
        .iter()
        .any(|&i| HEADER.iter().any(|d| lines[i].contains(d)));
    if is_header {
        if let Some(&i) = block.iter().find(|&&i| lines[i].contains("@penv=")) {
            let line = &mut lines[i];
            let at = line.find("@penv=").unwrap_or(0);
            let end = line[at..]
                .find(char::is_whitespace)
                .map_or(line.len(), |e| at + e);
            // The provider the header names stays; only the address changes.
            if let Some((provider, _)) = line[at + "@penv=".len()..end].split_once(':') {
                token = format!("@penv={provider}:{org}/{project}");
            }
            line.replace_range(at..end, &token);
            return lines.join("\n");
        }
        let first = block
            .iter()
            .copied()
            .find(|&i| HEADER.iter().any(|d| lines[i].contains(d)))
            .unwrap_or(block[0]);
        let line = &mut lines[first];
        let hash = line.find('#').unwrap_or(0);
        line.insert_str(hash + 1, &format!(" {token}"));
        return lines.join("\n");
    }
    format!("# {token}\n\n{source}")
}
