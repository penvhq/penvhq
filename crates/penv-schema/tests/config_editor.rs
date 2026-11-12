use std::collections::BTreeSet;

use penv_schema::config_editor::{
    config_complete, config_hover, config_problems, config_vocabulary,
};
use penv_schema::editor::{Note, Position, Severity};

fn keys() -> Vec<String> {
    ["DATABASE_URL", "GCP_SA_JSON", "API_KEY_FP"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn problems(text: &str) -> Vec<Note> {
    config_problems(text, Some(&keys()))
}

fn at(line: u32, character: u32) -> Position {
    Position { line, character }
}

fn labels(text: &str, pos: Position) -> Vec<String> {
    config_complete(text, pos, &keys())
        .into_iter()
        .map(|c| c.label)
        .collect()
}

/// `config.rs`'s template with this build's schema version.
fn template() -> String {
    let src = include_str!("../../penv/src/config.rs");
    let start = src.find("TEMPLATE: &str = r#\"").unwrap() + "TEMPLATE: &str = r#\"".len();
    let end = start + src[start..].find("\"#;").unwrap();
    src[start..end].replace("{version}", &penv_schema::SCHEMA_VERSION.to_string())
}

#[test]
fn the_template_has_no_problems() {
    assert_eq!(problems(&template()), []);
}

#[test]
fn a_syntax_error_sits_where_the_parser_stopped() {
    let found = problems("[run]\npreload = true\n\n[url\npublic = []\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "invalid_config");
    assert_eq!(found[0].severity, Severity::Error);
    assert_eq!(found[0].range.start.line, 3);
    assert!(
        found[0].message.starts_with("Not valid TOML"),
        "{}",
        found[0].message
    );
}

#[test]
fn an_unknown_setting_is_a_warning_that_names_the_nearest() {
    let found = problems("[run]\npreloda = true\n\n[futrue]\nx = 1\n\n[urll]\npublic = []\n");
    let messages: Vec<&str> = found.iter().map(|n| n.message.as_str()).collect();
    assert!(
        found.iter().all(|n| n.severity == Severity::Warning),
        "{found:?}"
    );
    assert!(
        messages[0].contains("did you mean preload?"),
        "{messages:?}"
    );
    assert_eq!(found[0].range.start, at(1, 0));
    assert!(!messages[1].contains("did you mean"), "{messages:?}");
    assert!(messages[2].contains("did you mean [url]?"), "{messages:?}");
}

#[test]
fn a_wrong_type_is_an_error_only_where_penv_refuses_it() {
    let found = problems("[schema]\nversion = \"1\"\n\n[run]\npreload = \"yes\"\n");
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].severity, Severity::Error);
    assert!(found[0].message.contains("whole number"));
    assert_eq!(found[0].range.start, at(1, 10));
    assert_eq!(found[1].severity, Severity::Warning);
    assert!(
        found[1]
            .message
            .contains("[run] preload takes true or false")
    );
    let newer = problems("[schema]\nversion = 99\n");
    assert_eq!(newer[0].code, "unsupported_schema");
}

#[test]
fn public_lists_follow_check() {
    let text = "[url]\npublic = [\"host\", \"password\", \"hots\", 3]\n\n[hash]\npublic = [\"API_KEY_FP\", \"NOT_DECLARED\"]\n\n[json.GCP_SA_JSON]\npublic = [\"client_email\"]\n\n[json.MISSING_KEY]\npublic = [\"a\"]\n\n[json.GCP_SA_JSON_2]\npaths = 1\n";
    let found = config_problems(text, Some(&keys()));
    let codes: Vec<(&str, u32)> = found
        .iter()
        .map(|n| (n.code.as_str(), n.range.start.line))
        .collect();
    assert!(codes.contains(&("public_list", 1)), "{codes:?}");
    assert!(codes.contains(&("wrong_type", 1)), "{codes:?}");
    assert!(codes.contains(&("unknown_key", 4)), "{codes:?}");
    assert!(codes.contains(&("unknown_key", 9)), "{codes:?}");
    assert!(
        found
            .iter()
            .any(|n| n.code == "unknown_key" && n.message.contains("rename [json.MISSING_KEY] to")),
        "{found:?}"
    );
    assert!(codes.contains(&("public_list", 12)), "{codes:?}");
    assert!(!codes.iter().any(|(_, line)| *line == 6), "{codes:?}");
    // Without a schema, references go unchecked and a pasted entry is never echoed.
    let found = config_problems(text, None);
    assert!(!found.iter().any(|n| n.code == "unknown_key"), "{found:?}");
    assert!(found.iter().all(|n| !n.message.contains("hots")));
}

#[test]
fn completes_sections_settings_and_values() {
    assert!(labels("[", at(0, 1)).contains(&"url".to_string()));
    assert_eq!(labels("[json.", at(0, 6)), keys());
    let settings = labels("[run]\n\n", at(1, 0));
    assert_eq!(settings, ["preload"]);
    assert_eq!(labels("[run]\npreload = ", at(1, 10)), ["true", "false"]);
    let parts = labels("[url]\npublic = [\"", at(1, 11));
    assert!(parts.contains(&"host".to_string()) && parts.contains(&"query.NAME".to_string()));
    assert!(!parts.contains(&"password".to_string()));
    let across = labels("[hash]\npublic = [\n  ", at(2, 2));
    assert_eq!(across, keys());
    assert_eq!(labels("[hosts]\n", at(1, 0)), keys());
    assert!(labels("", at(0, 0)).contains(&"[local]".to_string()));
}

#[test]
fn hover_explains_sections_and_settings() {
    let tip = config_hover("[url]\npublic = []\n", at(1, 2)).unwrap();
    assert!(
        tip.markdown.contains("url($DATABASE_URL, host)"),
        "{}",
        tip.markdown
    );
    let tip = config_hover("[json.GCP_SA_JSON]\n", at(0, 2)).unwrap();
    assert!(tip.markdown.starts_with("`[json.KEY]`"), "{}", tip.markdown);
    assert!(config_hover("[run]\npreload = true\n", at(1, 12)).is_none());
}

#[test]
fn every_setting_config_rs_reads_is_in_the_vocabulary() {
    let src = include_str!("../../penv/src/config.rs");
    let src = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
    let named: BTreeSet<&str> = config_vocabulary()
        .iter()
        .flat_map(|s| {
            std::iter::once(s.name.as_str()).chain(s.keys.iter().map(|k| k.name.as_str()))
        })
        .collect();
    for call in [".get(\"", ".get_mut(\"", ".entry(\""] {
        for piece in src.split(call).skip(1) {
            let name = &piece[..piece.find('"').unwrap()];
            assert!(
                named.contains(name),
                "config.rs reads {name}; add it to config_vocabulary.json"
            );
        }
    }
}

#[test]
fn every_entry_has_a_doc_and_every_check_a_message() {
    for section in config_vocabulary() {
        assert!(!section.doc.is_empty(), "{}", section.name);
        for key in section.keys.iter().chain(section.entries.iter()) {
            assert!(!key.doc.is_empty(), "{}.{}", section.name, key.name);
            assert!(!key.if_wrong.is_empty(), "{}.{}", section.name, key.name);
        }
    }
}

#[test]
fn a_schema_that_is_not_a_table_is_ignored_as_penv_ignores_it() {
    let found = problems("schema = 1\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].severity, Severity::Warning);
    assert!(found[0].message.contains("penv ignores it"), "{found:?}");
    for refused in ["url = 1\n", "hash = 1\n", "json = 1\n"] {
        assert_eq!(problems(refused)[0].severity, Severity::Error, "{refused}");
    }
}

#[test]
fn a_list_directly_under_json_is_one_error() {
    let found = problems("[json]\npublic = [\"a\"]\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "wrong_type");
}

#[test]
fn json_messages_name_the_section_or_its_position() {
    let found = problems("[json.GCP_SA_JSON]\npaths = 1\n");
    let messages: Vec<&str> = found.iter().map(|n| n.message.as_str()).collect();
    assert!(
        messages.contains(
            &"[json.GCP_SA_JSON] public is missing; write a list of read paths, such as [\"client_email\"]"
        ),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("in [json.GCP_SA_JSON] and ignores it")),
        "{messages:?}"
    );
    // An undeclared name may be a pasted value, so it is shown by position.
    let found = config_problems("[json.sk_fake_123]\npaths = 1\n", None);
    assert!(
        found.iter().all(|n| !n.message.contains("sk_fake_123")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|n| n.message.starts_with("[json.entry 1] public is missing")),
        "{found:?}"
    );
}
