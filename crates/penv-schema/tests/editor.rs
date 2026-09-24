use std::collections::BTreeSet;

use penv_schema::editor::{
    KeyNote, Position, Severity, complete, definition, diagnostics, hover, symbols, vocabulary,
};
use penv_schema::parse;
use penv_schema::resolve::{FILTERS, FUNCTIONS};

const SCHEMA: &str = "\
# @penv=acme/api
# @defaultSensitive=true
# ---

# The port the API listens on
# @type=port @sensitive=false
API_PORT=8080

# @type=url @sensitive=false
API_URL=http://localhost:${API_PORT}

# Stripe's secret key 🔑
# @type=string(startsWith=sk_) @hosts(api.stripe.com) @rotate=90d
STRIPE_SECRET_KEY=
";

fn at(line: u32, character: u32) -> Position {
    Position { line, character }
}

/// The position just after the first occurrence of `needle` on `line`.
fn after(text: &str, line: u32, needle: &str) -> Position {
    let row = text.split('\n').nth(line as usize).unwrap();
    let byte = row.find(needle).unwrap() + needle.len();
    at(line, row[..byte].encode_utf16().count() as u32)
}

fn labels(text: &str, pos: Position) -> Vec<String> {
    complete(text, pos).into_iter().map(|c| c.label).collect()
}

// ------------------------------------------------------------ vocabulary

/// One real use of every decorator the vocabulary names.
fn example(name: &str) -> (&'static str, bool) {
    // (text, header)
    match name {
        "penv" => ("@penv=acme/api", true),
        "schema" => ("@schema=1", true),
        "defaultSensitive" => ("@defaultSensitive=false", true),
        "defaultRequired" => ("@defaultRequired=infer", true),
        "currentEnv" => ("@currentEnv=$KEY", true),
        "import" => ("@import(../.env.schema)", true),
        "assert" => ("@assert(not(isEmpty($KEY)), \"KEY is empty\")", false),
        "type" => ("@type=number(min=1)", false),
        "required" => ("@required=forEnv(production)", false),
        "optional" => ("@optional", false),
        "sensitive" => ("@sensitive=false", false),
        "example" => ("@example=abc", false),
        "docs" => ("@docs=https://example.com", false),
        "deprecated" => ("@deprecated", false),
        "dynamic" => ("@dynamic", false),
        "static" => ("@static", false),
        "hosts" => ("@hosts(api.example.com)", false),
        "rotate" => ("@rotate=1y6m", false),
        "generateTypes" => ("@generateTypes(lang=ts, path=env.d.ts)", true),
        "plugin" => ("@plugin(@varlock/1password-plugin)", true),
        "redactLogs" => ("@redactLogs", true),
        "preventLeaks" => ("@preventLeaks", true),
        "envFlag" => ("@envFlag=APP_ENV", true),
        "setValuesBulk" => ("@setValuesBulk(x)", true),
        "disable" => ("@disable", true),
        other => panic!("vocabulary.json names @{other}; give it an example here"),
    }
}

#[test]
fn every_decorator_the_vocabulary_names_reads_the_way_its_origin_says() {
    for word in &vocabulary().decorators {
        let (use_, header) = example(&word.name);
        let text = if header {
            format!("# {use_}\n# ---\n\nKEY=x\n")
        } else {
            format!("# @defaultRequired=false\n# ---\n\n# {use_}\nKEY=x\n")
        };
        let schema =
            parse(&text).unwrap_or_else(|e| panic!("@{} does not parse: {e:?}", word.name));
        let ignored = schema.warnings.iter().any(|w| {
            w.message.contains(&format!("@{}", word.name))
                && (w.code == "unknown_decorator" || w.code == "varlock_only")
        });
        assert_eq!(
            ignored,
            word.origin == "varlock",
            "@{} is {} in vocabulary.json",
            word.name,
            word.origin
        );
    }
}

#[test]
fn functions_and_filters_are_exactly_the_ones_penv_runs() {
    let named: BTreeSet<&str> = vocabulary()
        .functions
        .iter()
        .map(|w| w.name.as_str())
        .collect();
    assert_eq!(named, FUNCTIONS.into_iter().collect());
    let named: BTreeSet<&str> = vocabulary()
        .filters
        .iter()
        .map(|w| w.name.as_str())
        .collect();
    assert_eq!(named, FILTERS.into_iter().collect());
}

#[test]
fn types_are_exactly_the_ones_the_parser_checks() {
    for word in &vocabulary().types {
        assert!(
            penv_schema::BaseType::from_name(&word.name).is_some(),
            "{}",
            word.name
        );
    }
    for name in [
        "string", "number", "boolean", "url", "email", "port", "enum",
    ] {
        assert!(vocabulary().types.iter().any(|w| w.name == name), "{name}");
    }
}

#[test]
fn every_rotate_suggestion_is_a_period_the_parser_takes() {
    for period in &vocabulary().rotate {
        assert!(
            penv_schema::rotate::Span::parse(period).is_some(),
            "{period}"
        );
    }
}

// ------------------------------------------------------------ diagnostics

#[test]
fn a_clean_schema_has_no_problems() {
    assert_eq!(diagnostics(SCHEMA, &[]), vec![]);
}

#[test]
fn an_error_sits_on_its_decorator_without_the_terminal_line_prefix() {
    let text = "# @defaultRequired=false\n# ---\n\n# 🔑 key\n# @type=port @rotate=soon\nPORT=\n";
    let notes = diagnostics(text, &[]);
    assert_eq!(notes.len(), 1, "{notes:?}");
    let note = &notes[0];
    assert_eq!(note.severity, Severity::Error);
    assert!(
        note.message.starts_with("@rotate takes"),
        "{}",
        note.message
    );
    assert_eq!(note.range.start, after(text, 4, "@type=port "));
    assert_eq!(note.range.end, after(text, 4, "@rotate=soon"));
}

#[test]
fn positions_count_utf16_units_after_wide_characters() {
    let text = "# @defaultRequired=false\n# ---\n\n# @example=🔑 @nope\nPORT=\n";
    let notes = diagnostics(text, &[]);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].severity, Severity::Warning);
    // The emoji is one char and two UTF-16 units, so "@nope" starts at 14, not 13.
    assert_eq!(notes[0].range.start, at(3, 14));
    assert_eq!(notes[0].range.end, at(3, 19));
}

#[test]
fn a_byte_order_mark_does_not_shift_first_line_positions() {
    let text = "\u{feff}# @defaultRequired=nope\n# ---\n\nKEY=\n";
    let notes = diagnostics(text, &[]);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].range.start, at(0, 3));
}

#[test]
fn a_key_line_error_covers_the_line() {
    let text = "# @defaultRequired=false\n# ---\n\n  not a key\n";
    let notes = diagnostics(text, &[]);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].range.start, at(3, 2));
    assert_eq!(notes[0].range.end, at(3, 11));
}

#[test]
fn a_note_from_the_binary_lands_on_the_decorator_it_names() {
    let note = KeyNote {
        key: "STRIPE_SECRET_KEY".into(),
        decorator: Some("rotate".into()),
        severity: Severity::Warning,
        code: "rotate_overdue".into(),
        message: "rotate STRIPE_SECRET_KEY was due 2026-01-01".into(),
    };
    let notes = diagnostics(SCHEMA, &[note]);
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes[0].range.start,
        after(SCHEMA, 12, "@hosts(api.stripe.com) ")
    );
    assert_eq!(notes[0].range.end, after(SCHEMA, 12, "@rotate=90d"));
    assert_eq!(notes[0].to_json()["source"], "penv");
}

#[test]
fn a_note_for_a_key_without_that_decorator_lands_on_the_key() {
    let note = KeyNote {
        key: "API_PORT".into(),
        decorator: Some("rotate".into()),
        severity: Severity::Information,
        code: "x".into(),
        message: "m".into(),
    };
    let notes = diagnostics(SCHEMA, &[note]);
    assert_eq!(notes[0].range.start, at(6, 0));
    assert_eq!(notes[0].range.end, at(6, 8));
}

// ------------------------------------------------------------ completion

#[test]
fn the_header_offers_header_and_key_decorators() {
    let text = "# @\n# ---\n\nKEY=\n";
    let got = labels(text, at(0, 3));
    assert!(got.contains(&"@penv".to_string()), "{got:?}");
    assert!(got.contains(&"@rotate".to_string()));
    assert!(
        !got.contains(&"@schema".to_string()),
        "a retired decorator is not offered"
    );
    assert!(
        !got.contains(&"@plugin".to_string()),
        "varlock's are not offered"
    );
}

#[test]
fn a_key_block_offers_only_key_decorators() {
    let text = "# @penv=a/b\n# ---\n\n# @ho\nKEY=\n";
    let items = complete(text, at(3, 5));
    let got: Vec<&str> = items.iter().map(|c| c.label.as_str()).collect();
    assert!(got.contains(&"@hosts"));
    assert!(!got.contains(&"@penv"));
    let hosts = items.iter().find(|c| c.label == "@hosts").unwrap();
    assert_eq!(hosts.replace.start, at(3, 3));
    assert_eq!(hosts.replace.end, at(3, 5));
    assert!(hosts.snippet);
    assert!(hosts.detail.contains("penv"));
}

#[test]
fn a_bare_comment_offers_decorators_with_their_at_sign() {
    let text = "# @penv=a/b\n# ---\n\n# \nKEY=\n";
    let items = complete(text, at(3, 2));
    let rotate = items.iter().find(|c| c.label == "@rotate").unwrap();
    assert!(rotate.insert.starts_with("@rotate="));
}

#[test]
fn a_description_comment_offers_nothing() {
    let text = "# @penv=a/b\n# ---\n\n# The key for \nKEY=\n";
    assert!(labels(text, at(3, 14)).is_empty());
}

#[test]
fn type_values_and_their_constraints() {
    let text = "# @penv=a/b\n# ---\n\n# @type=\nKEY=\n";
    assert!(labels(text, at(3, 8)).contains(&"enum".to_string()));

    let text = "# @penv=a/b\n# ---\n\n# @type=string(\nKEY=\n";
    let items = complete(text, at(3, 15));
    let got: Vec<&str> = items.iter().map(|c| c.label.as_str()).collect();
    assert!(
        got.contains(&"startsWith") && got.contains(&"maxLength"),
        "{got:?}"
    );
    assert_eq!(items[0].insert, format!("{}=", items[0].label));

    let text = "# @penv=a/b\n# ---\n\n# @type=port(m\nKEY=\n";
    assert_eq!(labels(text, at(3, 14)), vec!["min", "max"]);
}

#[test]
fn rotate_and_flag_values() {
    let text = "# @penv=a/b\n# ---\n\n# @rotate=\nKEY=\n";
    assert!(labels(text, at(3, 10)).contains(&"90d".to_string()));
    let text = "# @penv=a/b\n# ---\n\n# @required=\nKEY=\n";
    assert!(labels(text, at(3, 12)).contains(&"forEnv(...)".to_string()));
    let text = "# @defaultRequired=\n# ---\n\nKEY=\n";
    assert_eq!(labels(text, at(0, 19)), vec!["true", "false", "infer"]);
}

#[test]
fn current_env_offers_keys_as_references() {
    let text = "# @currentEnv=\n# ---\n\nAPP_ENV=dev\nOTHER=\n";
    assert_eq!(labels(text, at(0, 14)), vec!["$APP_ENV", "$OTHER"]);
}

#[test]
fn references_filters_and_addresses_in_values() {
    let text = "# @penv=a/b\n# ---\n\nHOST=x\nURL=https://${H\n";
    assert_eq!(labels(text, at(4, 15)), vec!["HOST", "URL"]);

    let text = "# @penv=a/b\n# ---\n\nHOST=x\nURL=${HOST | u\n";
    let got = labels(text, at(4, 14));
    assert!(got.contains(&"urlencode".to_string()) && !got.contains(&"HOST".to_string()));

    let text = "# @penv=a/b\n# ---\n\nHOST=x\nURL=penv(production/\n";
    assert_eq!(labels(text, at(4, 20)), vec!["HOST", "URL"]);

    let text = "# @penv=a/b\n# ---\n\nHOST=x\nURL=$\n";
    assert_eq!(labels(text, at(4, 5)), vec!["HOST", "URL"]);
}

#[test]
fn functions_where_a_value_or_argument_starts_and_nowhere_else() {
    let text = "# @penv=a/b\n# ---\n\nURL=con\n";
    let items = complete(text, at(3, 7));
    let concat = items.iter().find(|c| c.label == "concat").unwrap();
    assert_eq!(concat.replace.start, at(3, 4));
    assert_eq!(concat.kind, 3);

    let text = "# @penv=a/b\n# ---\n\nURL=concat(a, re\n";
    assert!(labels(text, at(3, 16)).contains(&"ref".to_string()));

    let text = "# @penv=a/b\n# ---\n\nURL=http://localhost\n";
    assert!(labels(text, at(3, 20)).is_empty());
}

#[test]
fn nothing_is_offered_before_a_keys_equals_sign() {
    let text = "# @penv=a/b\n# ---\n\nURL=x\n";
    assert!(labels(text, at(3, 2)).is_empty());
}

#[test]
fn renaming_a_decorator_with_a_value_replaces_only_its_name() {
    let text = "# @penv=a/b\n# ---\n\n# @ty=url\nKEY=\n";
    let items = complete(text, at(3, 5));
    let ty = items.iter().find(|c| c.label == "@type").unwrap();
    assert_eq!(ty.insert, "type");
    assert!(!ty.snippet);
    assert_eq!(ty.replace.end, at(3, 5));
}

// ------------------------------------------------------------ hover

#[test]
fn schema_text_in_a_hover_cannot_render_markdown() {
    let text = "# @defaultRequired=false\n# ---\n\n# see ![x](https://t.example/p.png) <img src=x>\n# @type=string(matches=\"a`b\")\nKEY=\n";
    let md = hover(text, at(5, 1), &[]).unwrap().markdown;
    assert!(md.contains("\\!\\[x\\]\\(https"), "{md}");
    assert!(md.contains("\\<img"), "{md}");
    // A backtick in the type is quoted by the schema's own rule and cannot close the span.
    assert!(md.contains("``string(matches=\"a`b\")``"), "{md}");
}

#[test]
fn hovering_a_decorator_explains_it_and_where_it_comes_from() {
    let tip = hover(SCHEMA, at(12, 32), &[]).unwrap();
    assert!(tip.markdown.contains("penv extension"), "{}", tip.markdown);
    assert!(tip.markdown.contains("Seals the value"));
    let tip = hover(SCHEMA, at(12, 3), &[]).unwrap();
    assert!(tip.markdown.contains("@env-spec"));
}

#[test]
fn hovering_a_key_summarises_the_schema_and_the_binarys_notes() {
    let note = KeyNote {
        key: "STRIPE_SECRET_KEY".into(),
        decorator: Some("rotate".into()),
        severity: Severity::Information,
        code: "rotate_due".into(),
        message: "rotate STRIPE_SECRET_KEY by 2026-12-01".into(),
    };
    let tip = hover(SCHEMA, at(13, 4), &[note]).unwrap();
    let md = &tip.markdown;
    assert!(
        md.starts_with("**STRIPE_SECRET_KEY**: `string(startsWith=sk_)`"),
        "{md}"
    );
    assert!(
        md.contains("required · sensitive · sealed to `api.stripe.com` · rotate every `90d`"),
        "{md}"
    );
    assert!(md.contains("Stripe\\'s secret key 🔑"), "{md}");
    assert!(md.ends_with("rotate STRIPE_SECRET_KEY by 2026-12-01"));
    assert_eq!(tip.range.start, at(13, 0));
    assert_eq!(tip.range.end, at(13, 17));
}

#[test]
fn hovering_a_reference_function_or_type_explains_it() {
    let text = "# @penv=a/b\n# ---\n\nPORT=1\nURL=concat(\"x\", ${PORT | trim})\n# @type=url\nX=\n";
    assert!(
        hover(text, at(4, 20), &[])
            .unwrap()
            .markdown
            .starts_with("**PORT**")
    );
    assert!(
        hover(text, at(4, 6), &[])
            .unwrap()
            .markdown
            .contains("Joins")
    );
    assert!(
        hover(text, at(4, 28), &[])
            .unwrap()
            .markdown
            .contains("without surrounding")
    );
    assert!(
        hover(text, at(5, 9), &[])
            .unwrap()
            .markdown
            .contains("absolute URL")
    );
}

#[test]
fn an_error_in_another_block_still_leaves_each_key_its_summary() {
    let text = "# @defaultSensitive=false\n# ---\n\n# @rotate=soon\nA=\n\n# @type=url @hosts(x.example.com)\nB=\n";
    assert!(parse(text).is_err());
    let md = hover(text, at(7, 0), &[]).unwrap().markdown;
    assert!(md.starts_with("**B**: `url`"), "{md}");
    assert!(md.contains("sealed to `x.example.com`"), "{md}");
    assert!(
        !md.contains("sensitive ·"),
        "the header still applies: {md}"
    );
    assert_eq!(hover(text, at(4, 0), &[]).unwrap().markdown, "**A**");
}

#[test]
fn hovering_plain_text_shows_nothing() {
    assert!(hover(SCHEMA, at(4, 6), &[]).is_none());
    assert!(hover(SCHEMA, at(9, 12), &[]).is_none());
}

// ------------------------------------------------------------ definition and outline

#[test]
fn a_reference_goes_to_its_key() {
    let target = definition(SCHEMA, after(SCHEMA, 9, "${API_P")).unwrap();
    assert_eq!(target.start, at(6, 0));
    assert_eq!(target.end, at(6, 8));
    let text = "# @currentEnv=$APP_ENV\n# ---\n\nAPP_ENV=dev\nX=ref(APP_ENV)\n";
    assert_eq!(definition(text, at(0, 17)).unwrap().start, at(3, 0));
    assert_eq!(definition(text, at(4, 8)).unwrap().start, at(3, 0));
    assert!(
        definition(SCHEMA, at(9, 10)).is_none(),
        "plain text is not a reference"
    );
}

#[test]
fn the_outline_lists_keys_spanning_their_blocks() {
    let got = symbols(SCHEMA);
    let names: Vec<&str> = got.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["API_PORT", "API_URL", "STRIPE_SECRET_KEY"]);
    assert_eq!(got[0].range.start, at(4, 0));
    assert_eq!(got[0].range.end.line, 6);
    assert_eq!(got[0].detail, "port");
    assert_eq!(got[2].detail, "string(startsWith=sk_)");
}
