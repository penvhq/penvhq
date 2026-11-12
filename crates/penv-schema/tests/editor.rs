use std::collections::BTreeSet;

use penv_schema::editor::{
    KeyNote, Position, Severity, complete, definition, diagnostics, hover, symbols, vocabulary,
};
use penv_schema::resolve::{FILTERS, FUNCTIONS};
use penv_schema::{VARLOCK_HEADER, parse};

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
fn a_computed_host_shows_as_sealed_to_its_expression() {
    let text = "# @type=url @hosts=url($DATABASE_URL, host)\nDATABASE_URL=\n";
    let md = hover(text, at(1, 0), &[]).unwrap().markdown;
    assert!(md.contains("sealed to `url($DATABASE_URL, host)`"), "{md}");
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

fn call_schema(expr: &str) -> String {
    format!("# @penv=acme/api\n# ---\n\nK=abc\nV={expr}\n")
}

fn complete_in(expr: &str, needle: &str) -> Vec<String> {
    let text = call_schema(expr);
    labels(&text, after(&text, 4, needle))
}

#[test]
fn option_names_follow_the_open_paren_and_comma_without_repeats() {
    let got = complete_in("hash($K, ", "hash($K, ");
    for want in ["algo", "encoding", "length", "key"] {
        assert!(got.contains(&want.to_string()), "{want} missing: {got:?}");
    }
    let got = complete_in("hash($K, algo=sha256, ", "sha256, ");
    assert!(!got.contains(&"algo".to_string()));
    assert!(got.contains(&"encoding".to_string()));
    let got = complete_in("random(", "random(");
    assert!(got.contains(&"kind".to_string()) && got.contains(&"length".to_string()));
}

#[test]
fn option_values_come_from_their_section() {
    let names = |s: &str| -> Vec<String> {
        vocabulary()
            .section(s)
            .iter()
            .map(|e| e.name.clone())
            .collect()
    };
    assert_eq!(complete_in("hash($K, algo=)", "algo="), names("algorithms"));
    assert_eq!(
        complete_in("hash($K, encoding=)", "encoding="),
        names("encodings")
    );
    assert_eq!(complete_in("random(kind=)", "kind="), names("random-kinds"));
}

#[test]
fn url_parts_follow_the_subject_and_skip_given_ones() {
    let got = complete_in("url($K, ", "url($K, ");
    assert!(got.contains(&"host".to_string()) && got.contains(&"scheme".to_string()));
    let got = complete_in("url($K, host=x, ", "host=x, ");
    assert!(!got.contains(&"host".to_string()) && got.contains(&"port".to_string()));
}

#[test]
fn hover_on_a_value_or_part_shows_its_doc() {
    let text = call_schema("hash($K, algo=sha256) url($K, host)");
    let doc = |e: &str| {
        vocabulary()
            .section(e)
            .iter()
            .find(|x| x.name == "sha256" || x.name == "host")
            .unwrap()
            .doc
            .clone()
    };
    let tip = hover(&text, after(&text, 4, "algo=sha2"), &[]).unwrap();
    assert!(tip.markdown.contains(&doc("algorithms")));
    let tip = hover(&text, after(&text, 4, "url($K, ho"), &[]).unwrap();
    assert!(tip.markdown.contains(&doc("url-parts")));
}

#[test]
fn diagnostics_suggest_the_nearest_part_and_name_a_missing_option() {
    let text = call_schema("url($K, hots)");
    let notes = diagnostics(&text, &[]);
    assert!(
        notes
            .iter()
            .any(|n| n.message.contains("did you mean host?")),
        "{notes:?}"
    );
    let text = call_schema("hash($K)");
    let notes = diagnostics(&text, &[]);
    assert!(
        notes.iter().any(|n| n.message.contains("needs algo=")),
        "{notes:?}"
    );
}

#[test]
fn every_values_from_names_an_existing_section() {
    let v = vocabulary();
    for w in &v.functions {
        let slots = [&w.address, &w.labels, &w.each, &w.cases]
            .into_iter()
            .flatten()
            .filter_map(|s| s.values_from.as_deref());
        let opts = w.args.iter().filter_map(|o| o.values_from.as_deref());
        for section in slots.chain(opts) {
            assert!(!v.section(section).is_empty(), "{}: {section}", w.name);
        }
    }
}

#[test]
fn an_option_name_already_followed_by_eq_inserts_bare() {
    let text = call_schema("hash($K, al=sha256)");
    let items = complete(&text, after(&text, 4, "hash($K, al"));
    let algo = items.iter().find(|c| c.label == "algo").unwrap();
    assert_eq!(algo.insert, "algo");
    let text = call_schema("hash($K, al)");
    let items = complete(&text, after(&text, 4, "hash($K, al"));
    assert_eq!(
        items.iter().find(|c| c.label == "algo").unwrap().insert,
        "algo="
    );
}

#[test]
fn bad_call_diagnostics_sit_on_the_offending_token() {
    let text = call_schema("url($K, hots)");
    let notes = diagnostics(&text, &[]);
    let n = notes
        .iter()
        .find(|n| n.message.contains("did you mean"))
        .unwrap();
    assert_eq!(n.range.start, at(4, 10));
    let text = call_schema("hash($K, algo=sha256, lenth=4)");
    let notes = diagnostics(&text, &[]);
    let n = notes
        .iter()
        .find(|n| n.message.contains("no option"))
        .unwrap();
    assert_eq!(n.range.start, at(4, 24));
}

// ------------------------------------------------------------ drift

/// Every string a `match` arm names in `src` after `marker`, at the arms' own indent.
fn arm_names(src: &str, marker: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (at, _) in src.match_indices(marker) {
        let mut lines = src[at..].lines().skip(1).filter(|l| !l.trim().is_empty());
        let first = lines.next().unwrap();
        let indent = first.len() - first.trim_start().len();
        for line in std::iter::once(first).chain(lines) {
            let here = line.len() - line.trim_start().len();
            if here < indent {
                break;
            }
            let trimmed = line.trim_start();
            if here > indent || !trimmed.starts_with('"') {
                continue;
            }
            let (pattern, _) = trimmed.split_once("=>").unwrap();
            for name in pattern.split('|') {
                out.insert(name.trim().trim_matches('"').to_string());
            }
        }
    }
    assert!(!out.is_empty(), "no arms after {marker}");
    out
}

/// The text of the function `name` in `src`, up to the next `fn`.
fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src.find(&format!("fn {name}(")).unwrap();
    let rest = &src[start + 3..];
    &rest[..rest.find("\nfn ").unwrap_or(rest.len())]
}

#[test]
fn every_decorator_the_parser_reads_is_in_the_vocabulary() {
    let src = include_str!("../src/parse.rs");
    let named: BTreeSet<String> = vocabulary()
        .decorators
        .iter()
        .map(|w| w.name.clone())
        .collect();
    for name in arm_names(src, "match d.name.as_str() {") {
        assert!(
            named.contains(&name),
            "the parser reads @{name}; add it to vocabulary.json"
        );
    }
    let varlock: BTreeSet<String> = vocabulary()
        .decorators
        .iter()
        .filter(|w| w.origin == "varlock")
        .map(|w| w.name.clone())
        .collect();
    let header: BTreeSet<String> = VARLOCK_HEADER.iter().map(|s| s.to_string()).collect();
    assert_eq!(varlock, header);
}

#[test]
fn url_parts_are_exactly_the_ones_url_reads_and_writes() {
    let src = include_str!("../src/functions/url.rs");
    let parts = &vocabulary().url_parts;
    let all = |marker: &str, body: &str| {
        let mut names = arm_names(body, marker);
        if body.contains("strip_prefix(\"query.\")") {
            names.insert("query.NAME".to_string());
        }
        names
    };
    let named: BTreeSet<String> = parts.iter().map(|e| e.name.clone()).collect();
    assert_eq!(all("Ok(match part {", fn_body(src, "read")), named);
    let writable: BTreeSet<String> = parts
        .iter()
        .filter(|e| !e.read_only)
        .map(|e| e.name.clone())
        .collect();
    assert_eq!(all("match *part {", fn_body(src, "write")), writable);
}

#[test]
fn random_kinds_are_exactly_the_ones_random_takes() {
    use penv_schema::functions::random::{RandomKind, spec};
    let name = |k: RandomKind| match k {
        RandomKind::Alnum => "alnum",
        RandomKind::Hex => "hex",
        RandomKind::Base64Url => "base64url",
        RandomKind::Uuid => "uuid",
    };
    let every = [
        RandomKind::Alnum,
        RandomKind::Hex,
        RandomKind::Base64Url,
        RandomKind::Uuid,
    ];
    let mut read = BTreeSet::new();
    for e in &vocabulary().random_kinds {
        let call = if e.name == "uuid" {
            "random(kind=uuid)".to_string()
        } else {
            format!("random(kind={}, length=16)", e.name)
        };
        let kind = spec(&call).unwrap().unwrap().kind;
        assert_eq!(name(kind), e.name);
        read.insert(e.name.as_str());
    }
    assert_eq!(read, every.into_iter().map(name).collect());
    assert!(spec("random(kind=nope, length=16)").is_err());
}

#[test]
fn type_options_are_exactly_each_types_constraints() {
    use penv_schema::BaseType::{self, *};
    let every = [String, Number, Boolean, Url, Email, Port, Enum];
    // A new BaseType fails to compile here until it is listed above.
    for b in every {
        match b {
            String | Number | Boolean | Url | Email | Port | Enum => {}
        }
    }
    for b in every {
        let word = vocabulary()
            .types
            .iter()
            .find(|w| w.name == b.as_str())
            .unwrap_or_else(|| panic!("vocabulary.json has no type {}", b.as_str()));
        let options: Vec<&str> = word.args.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(options, b.constraints(), "{}", b.as_str());
        assert!(
            word.args.iter().all(|o| !o.doc.is_empty()),
            "{}",
            b.as_str()
        );
    }
    assert_eq!(vocabulary().types.len(), every.len());
    assert_eq!(BaseType::from_name("port"), Some(Port));
}

// ------------------------------------------------------------ environments, options, references

#[test]
fn environment_names_complete_where_an_environment_is_named() {
    let envs: Vec<String> = vocabulary()
        .environments
        .iter()
        .map(|e| e.name.clone())
        .collect();
    for want in ["development", "staging", "production", "test"] {
        assert!(envs.contains(&want.to_string()), "{want}");
    }
    assert_eq!(complete_in("forEnv(", "forEnv("), envs);
    let got = complete_in("forEnv(production, ", "production, ");
    assert!(!got.contains(&"production".to_string()) && got.contains(&"staging".to_string()));
    assert_eq!(complete_in("if(forEnv(st", "forEnv(st"), envs);
    assert_eq!(complete_in("penv(", "penv("), envs);

    let text = "# @penv=a/b\n# ---\n\n# @required=forEnv(\nKEY=\n";
    assert_eq!(labels(text, at(3, 20)), envs);
    let text = "# @penv=a/b\n# ---\n\n# @assert(forEnv(\nKEY=\n";
    assert_eq!(labels(text, at(3, 19)), envs);
}

#[test]
fn hovering_an_environment_name_explains_it() {
    let doc = |name: &str| {
        vocabulary()
            .section("environments")
            .iter()
            .find(|e| e.name == name)
            .unwrap()
            .doc
            .clone()
    };
    let text = call_schema("if(forEnv(staging), penv(production/K), x)");
    let tip = hover(&text, after(&text, 4, "forEnv(sta"), &[]).unwrap();
    assert!(tip.markdown.contains(&doc("staging")));
    let tip = hover(&text, after(&text, 4, "penv(prod"), &[]).unwrap();
    assert!(tip.markdown.contains(&doc("production")));
    assert!(doc("development").contains("PENV_ENV"));
}

#[test]
fn type_options_carry_their_docs_in_completion_and_hover() {
    let text = "# @penv=a/b\n# ---\n\n# @type=number(\nKEY=\n";
    let items = complete(text, at(3, 15));
    let min = items.iter().find(|c| c.label == "min").unwrap();
    assert!(min.doc.contains("or more"), "{}", min.doc);
    assert_eq!(min.insert, "min=");

    let text = "# @penv=a/b\n# ---\n\n# @type=string(minLength=8)\nKEY=\n";
    let tip = hover(text, after(text, 3, "minLen"), &[]).unwrap();
    assert!(tip.markdown.contains("at least"), "{}", tip.markdown);
    let tip = hover(text, after(text, 3, "@type=str"), &[]).unwrap();
    assert!(tip.markdown.contains("Any text"), "{}", tip.markdown);
}

#[test]
fn hovering_reference_syntax_explains_the_form() {
    let form = |name: &str| {
        vocabulary()
            .section("reference-forms")
            .iter()
            .find(|e| e.name == name)
            .unwrap()
            .doc
            .clone()
    };
    let text = call_schema("x${A:-y-z} ${B-q} ${K | trim} $K");
    let on = |needle: &str| {
        let mut pos = after(&text, 4, needle);
        pos.character -= 1;
        hover(&text, pos, &[]).map(|t| t.markdown)
    };
    assert!(on("x${A:").unwrap().contains(&form("${KEY:-default}")));
    assert!(on("x${A:-").unwrap().contains(&form("${KEY:-default}")));
    assert!(on("${B-").unwrap().contains(&form("${KEY-default}")));
    assert!(on("${K |").unwrap().contains(&form("${KEY | filter}")));
    assert!(on("x$").unwrap().contains(&form("${KEY}")));
    assert!(on("trim} $").unwrap().contains(&form("$KEY")));
    // A `-` in the default text is text, not an operator.
    assert_eq!(on("y-"), None);
    // The key inside the braces still shows its summary.
    assert!(on("${K").unwrap().starts_with("**K**"));
}

#[test]
fn text_penv_takes_as_written_has_no_reference_hover() {
    let on = |text: &str, line: u32, needle: &str| {
        let mut pos = after(text, line, needle);
        pos.character -= 1;
        hover(text, pos, &[]).map(|t| t.markdown)
    };
    for (expr, needle) in [
        ("sk-test", "sk-"),
        ("${A}-${B}", "}-"),
        ("'$K'", "'$"),
        ("`${K:-x}`", "${K:-"),
        ("$5", "$"),
        ("${A:x-y}", "x-"),
        ("${A # c-d", "c-"),
        ("abc # ${K:-x}", "${K:-"),
    ] {
        let text = call_schema(expr);
        assert_eq!(on(&text, 4, needle), None, "{expr}");
    }
    let text = "# @penv=a/b\n# ---\n\n# @type=number(min=-1) @example=${A:-b}\nKEY=\n";
    assert_eq!(on(text, 3, "min=-"), None);
    assert_eq!(on(text, 3, "${A:-"), None);
    // A double-quoted value is still read.
    let text = call_schema("\"${K:-x}\" # note");
    assert!(on(&text, 4, "${K:-").is_some());
}

#[test]
fn environment_names_stay_out_of_decorators_that_take_none() {
    for decorator in [
        "@deprecated=forEnv(",
        "@type=enum(forEnv(",
        "@hosts=forEnv(",
    ] {
        let text = format!("# @penv=a/b\n# ---\n\n# {decorator}\nKEY=\n");
        let got = labels(&text, after(&text, 3, decorator));
        assert!(
            !got.contains(&"production".to_string()),
            "{decorator}: {got:?}"
        );
    }
}

fn json_schema(value: &str, expr: &str) -> String {
    format!("# @penv=a/b\n# ---\n\nCONFIG={value}\nV={expr}\n")
}

fn json_paths(value: &str, expr: &str, needle: &str) -> Vec<String> {
    let text = json_schema(value, expr);
    labels(&text, after(&text, 4, needle))
}

const DOC: &str = r#"{"db":{"host":"fake-host","hosts":["a","b"]},"a.b":1,"tls":true}"#;

#[test]
fn json_paths_complete_from_a_literal_document() {
    assert_eq!(
        json_paths(DOC, "json($CONFIG, ", "$CONFIG, "),
        ["db", r#"["a.b"]"#, "tls"]
    );
    assert_eq!(
        json_paths(DOC, "json($CONFIG, db.", "db."),
        ["host", "hosts"]
    );
    assert_eq!(
        json_paths(DOC, "json($CONFIG, db.ho)", "db.ho"),
        ["host", "hosts"]
    );
    assert_eq!(
        json_paths(DOC, "json($CONFIG, db.hosts[", "hosts["),
        ["0", "1"]
    );
    assert_eq!(
        json_paths(DOC, "${CONFIG | json(db.", "db."),
        ["host", "hosts"]
    );
    assert_eq!(
        json_paths(DOC, "json(${CONFIG}, ", ", "),
        ["db", r#"["a.b"]"#, "tls"]
    );
    let quoted = format!("'{DOC}'");
    assert_eq!(
        json_paths(&quoted, "json($CONFIG, db.", "db."),
        ["host", "hosts"]
    );
    assert_eq!(json_paths("[1,2]", "json($CONFIG, ", ", "), ["[0]", "[1]"]);

    let text = json_schema(DOC, "json($CONFIG, db.hosts[");
    let items = complete(&text, after(&text, 4, "hosts["));
    assert_eq!(items[0].insert, "0]");
    let text = json_schema(DOC, "json($CONFIG, )");
    for item in complete(&text, after(&text, 4, ", ")) {
        let shown = format!(
            "{} {} {} {}",
            item.label, item.detail, item.doc, item.insert
        );
        assert!(!shown.contains("fake-host"), "{shown}");
    }
}

#[test]
fn json_paths_offer_nothing_without_a_literal_document() {
    for (value, expr, needle) in [
        ("not-json", "json($CONFIG, ", ", "),
        ("${OTHER}", "json($CONFIG, ", ", "),
        (DOC, "json($MISSING, ", ", "),
        (DOC, "json($CONFIG, tls.", "tls."),
        (DOC, "json($CONFIG, nope.", "nope."),
        (DOC, "json($CONFIG, db.hosts.", "hosts."),
        (DOC, "${CONFIG | b64d | json(", "json("),
        (DOC, "json($CONFIG, [\"a.", "[\"a."),
    ] {
        assert_eq!(
            json_paths(value, expr, needle),
            Vec::<String>::new(),
            "{value} {expr}"
        );
    }
}

#[test]
fn an_unclosed_json_or_match_call_does_not_panic() {
    for expr in [
        "json(",
        "json($",
        "json($CONFIG,",
        "${CONFIG | json(",
        "match(",
        "match($APP_ENV,",
    ] {
        let text = json_schema(DOC, expr);
        let line = text.split('\n').nth(4).unwrap();
        for i in 0..=line.encode_utf16().count() as u32 {
            complete(&text, at(4, i));
        }
    }
}

fn match_schema(header: &str, expr: &str) -> String {
    format!("# @penv=a/b\n{header}# ---\n\nAPP_ENV=development\nOTHER=x\nV={expr}\n")
}

#[test]
fn match_labels_are_environments_when_the_subject_is_the_current_env_key() {
    let envs: Vec<String> = vocabulary()
        .environments
        .iter()
        .map(|e| e.name.clone())
        .collect();
    let header = "# @currentEnv=$APP_ENV\n";
    let text = match_schema(header, "match($APP_ENV, ");
    assert_eq!(labels(&text, after(&text, 6, "ENV, ")), envs);
    assert_eq!(
        complete(&text, after(&text, 6, "ENV, "))[0].insert,
        "development: "
    );

    let text = match_schema(header, "match(${APP_ENV}, production: a, st");
    let got = labels(&text, after(&text, 6, ", st"));
    assert!(!got.contains(&"production".to_string()), "{got:?}");
    assert!(got.contains(&"staging".to_string()), "{got:?}");

    let text = match_schema(header, "match($APP_ENV, test: x)");
    let items = complete(&text, after(&text, 6, ", te"));
    assert_eq!(
        items.iter().find(|c| c.label == "test").unwrap().insert,
        "test"
    );
}

#[test]
fn match_labels_are_not_environments_for_any_other_subject() {
    let header = "# @currentEnv=$APP_ENV\n";
    for (header, expr, needle) in [
        (header, "match($OTHER, ", "OTHER, "),
        ("", "match($APP_ENV, ", "ENV, "),
        (header, "match($APP_ENV, production: ", "production: "),
        (header, "match(", "match("),
    ] {
        let text = match_schema(header, expr);
        let row = text.lines().position(|l| l.starts_with("V=")).unwrap() as u32;
        let got = labels(&text, after(&text, row, needle));
        assert!(!got.contains(&"production".to_string()), "{expr}: {got:?}");
    }
}
