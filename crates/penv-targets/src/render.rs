use minijinja::value::Value as Jinja;
use minijinja::{Environment, context};
use serde_json::{Value, json};

use crate::error::Error;
use crate::folder::describe;
use crate::target::{INT_TYPE, Target};

/// A template sees the schema JSON, `penv.version`, the target's `[options]`
/// table, and one computed field per key: `lang_type`. Casting is the
/// template's own business.
pub fn render(target: &Target, schema: &Value, penv_version: &str) -> Result<String, Error> {
    let env = environment();
    let mut context = schema.clone();
    let keys = context
        .get_mut("keys")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| Error::Render {
            target: target.name.clone(),
            message: "the schema JSON has no keys array".into(),
        })?;

    for key in keys.iter_mut() {
        let base = mapped_name(target, &key["type"]).to_string();
        let lang = lang_type(&env, target, &base, &key["type"]["members"])?;
        key["lang_type"] = Value::String(lang);
    }
    context["penv"] = json!({ "version": penv_version });
    context["options"] = serde_json::to_value(&target.options).unwrap_or_else(|_| json!({}));

    env.render_str(&target.template, Jinja::from_serialize(&context))
        .map_err(|e| Error::Render {
            target: target.name.clone(),
            message: describe(&e),
        })
}

/// The `[types]` entry a key maps to: `integer` when a number is constrained to
/// whole values and the target carries that entry, the base type otherwise.
fn mapped_name<'a>(target: &Target, ty: &'a Value) -> &'a str {
    let base = ty["name"].as_str().unwrap_or("string");
    if base == "number" && is_int(ty) && target.types.contains_key(INT_TYPE) {
        return INT_TYPE;
    }
    base
}

fn is_int(ty: &Value) -> bool {
    let constraint = &ty["constraints"]["isInt"];
    constraint == &Value::Bool(true) || constraint.as_str() == Some("true")
}

fn lang_type(
    env: &Environment<'_>,
    target: &Target,
    base: &str,
    members: &Value,
) -> Result<String, Error> {
    let mapped = target.types.get(base).ok_or_else(|| Error::NoTypeFor {
        target: target.name.clone(),
        base: base.to_string(),
    })?;
    if base != "enum" {
        return Ok(mapped.clone());
    }
    let expression = env.compile_expression(mapped).map_err(|e| Error::Render {
        target: target.name.clone(),
        message: format!("the enum type expression is not valid: {}", describe(&e)),
    })?;
    expression
        .eval(context! { values => Jinja::from_serialize(members) })
        .map(|value| value.to_string())
        .map_err(|e| Error::Render {
            target: target.name.clone(),
            message: format!("the enum type expression failed: {}", describe(&e)),
        })
}

/// The shared environment plus the case, quoting and comment filters only a
/// language target needs.
fn environment() -> Environment<'static> {
    let mut env = crate::folder::environment();
    env.add_filter("pascal", pascal);
    env.add_filter("camel", camel);
    env.add_filter("snake", snake);
    env.add_filter("quote_rust", quote_rust);
    env.add_filter("quote_php", quote_php);
    env.add_filter("comment", comment);
    env.add_filter("java_comment", java_comment);
    env
}

/// A Rust string literal: `Debug` writes only escapes Rust reads back.
fn quote_rust(value: &str) -> String {
    format!("{value:?}")
}

/// A single-quoted PHP literal, where nothing but `\\` and `'` is special, so
/// a `$` stays a dollar sign.
fn quote_php(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Text inside a `/* */` comment that cannot end it early.
fn comment(value: &str) -> String {
    value.replace("*/", "*\\/")
}

/// A block comment Java can compile: javac reads a `\u` escape anywhere in
/// the source, so a lone backslash before `u` gets a second one.
fn java_comment(value: &str) -> String {
    let mut out = String::new();
    let mut run = 0;
    for c in comment(value).chars() {
        if c == 'u' && run % 2 == 1 {
            out.push('\\');
        }
        run = if c == '\\' { run + 1 } else { 0 };
        out.push(c);
    }
    out
}

/// Split a name on separators and on lower-to-upper humps, so `NEXT_PUBLIC_URL`
/// and `nextPublicUrl` produce the same words.
fn words(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut previous: Option<char> = None;
    for c in value.chars() {
        if !c.is_ascii_alphanumeric() {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            previous = None;
            continue;
        }
        let hump = previous.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
            && c.is_ascii_uppercase();
        if hump && !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
        word.push(c);
        previous = Some(c);
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

fn pascal(value: &str) -> String {
    words(value)
        .iter()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect()
}

fn camel(value: &str) -> String {
    let pascal = pascal(value);
    let mut chars = pascal.chars();
    match chars.next() {
        Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn snake(value: &str) -> String {
    words(value)
        .iter()
        .map(|word| word.to_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder::Source;
    use crate::target::parse;
    use std::collections::BTreeMap;

    fn target(template: &str, enum_expression: &str) -> Target {
        let types: BTreeMap<String, String> = [
            ("string", "string"),
            ("number", "number"),
            ("integer", "int"),
            ("boolean", "boolean"),
            ("url", "string"),
            ("email", "string"),
            ("port", "number"),
            ("enum", enum_expression),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        Target {
            name: "fake".into(),
            output: "out".into(),
            output_source: Source::BuiltIn,
            detect: vec![],
            types,
            options: toml::Table::new(),
            knobs: Vec::new(),
            suggest: Vec::new(),
            layout: Vec::new(),
            check: None,
            import: None,
            paths_from: None,
            template: template.into(),
            source: Source::BuiltIn,
            dir: "built in".into(),
        }
    }

    fn schema(keys: Value) -> Value {
        json!({
            "schemaVersion": 1,
            "org": null,
            "project": null,
            "defaultSensitive": true,
            "defaultRequired": true,
            "keys": keys,
        })
    }

    fn key(name: &str, base: &str, members: Value) -> Value {
        json!({
            "name": name,
            "description": null,
            "type": { "name": base, "raw": base, "members": members, "constraints": {} },
            "required": true,
            "sensitive": true,
            "default": null,
            "example": null,
            "docs": null,
            "since": null,
            "deprecated": null,
            "rotate": null,
            "dynamic": null,
        })
    }

    fn rendered(template: &str, keys: Value) -> String {
        render(
            &target(template, "values | join(' | ')"),
            &schema(keys),
            "9.9.9",
        )
        .unwrap()
    }

    #[test]
    fn a_template_sees_the_version_and_the_schema() {
        let out = rendered(
            "{{ penv.version }} {{ schemaVersion }} {{ keys[0].name }}",
            json!([key("PORT", "port", json!([]))]),
        );
        assert_eq!(out, "9.9.9 1 PORT");
    }

    #[test]
    fn every_key_carries_the_language_type_the_map_names() {
        let out = rendered(
            "{% for key in keys %}{{ key.lang_type }};{% endfor %}",
            json!([
                key("PORT", "port", json!([])),
                key("HOST", "string", json!([])),
            ]),
        );
        assert_eq!(out, "number;string;");
    }

    #[test]
    fn the_enum_entry_is_an_expression_over_values() {
        let target = target(
            "{{ keys[0].lang_type }}",
            "values | map('quote') | join(' | ')",
        );
        let out = render(
            &target,
            &schema(json!([key(
                "NODE_ENV",
                "enum",
                json!(["development", "production"])
            )])),
            "9.9.9",
        )
        .unwrap();
        assert_eq!(out, "\"development\" | \"production\"");
    }

    #[test]
    fn a_broken_enum_expression_names_the_target() {
        let target = target("{{ keys[0].lang_type }}", "values | nosuchfilter");
        let error = render(
            &target,
            &schema(json!([key("NODE_ENV", "enum", json!(["a"]))])),
            "9.9.9",
        )
        .unwrap_err();
        assert!(error.to_string().contains("target fake"));
    }

    #[test]
    fn a_typo_in_a_template_fails_instead_of_rendering_a_blank() {
        let error = render(
            &target("{{ keys[0].nmae }}", "values | join('|')"),
            &schema(json!([key("PORT", "port", json!([]))])),
            "9.9.9",
        )
        .unwrap_err();
        assert!(matches!(error, Error::Render { .. }));
    }

    #[test]
    fn the_case_filters_agree_on_the_words() {
        let out = rendered(
            "{{ keys[0].name | pascal }} {{ keys[0].name | camel }} {{ keys[0].name | snake }}",
            json!([key("NEXT_PUBLIC_APP_URL", "string", json!([]))]),
        );
        assert_eq!(out, "NextPublicAppUrl nextPublicAppUrl next_public_app_url");
    }

    #[test]
    fn the_case_filters_split_humps_too() {
        let out = rendered(
            "{{ keys[0].name | snake }} {{ keys[0].name | pascal }}",
            json!([key("nextPublicAppUrl", "string", json!([]))]),
        );
        assert_eq!(out, "next_public_app_url NextPublicAppUrl");
    }

    #[test]
    fn quote_escapes_what_a_string_literal_cannot_hold() {
        let out = rendered(
            "{{ keys[0].name | quote }}",
            json!([key("A\"B", "string", json!([]))]),
        );
        assert_eq!(out, "\"A\\\"B\"");
    }

    #[test]
    fn each_language_quotes_what_its_own_literals_read_back() {
        let tricky = "Hello $name \\ 'q' \"d\" \u{1f}\u{8}\u{c}\n";
        let out = rendered(
            "{{ keys[0].name | quote_rust }}|{{ keys[0].name | quote_php }}",
            json!([key(tricky, "string", json!([]))]),
        );
        let (rust, php) = out.split_once('|').unwrap();
        assert_eq!(rust, format!("{tricky:?}"));
        assert!(!rust.contains("\\u001f") && !rust.contains("\\b"), "{rust}");
        assert_eq!(
            php, "'Hello $name \\\\ \\'q\\' \"d\" \u{1f}\u{8}\u{c}\n'",
            "single quotes, so $name is not interpolated"
        );
    }

    #[test]
    fn a_description_cannot_close_the_comment_it_sits_in() {
        let out = rendered(
            "/* {{ keys[0].name | comment }} */ /* {{ keys[0].name | java_comment }} */",
            json!([key(
                "ends */ here, C:\\users \\\\u0041",
                "string",
                json!([])
            )]),
        );
        assert_eq!(
            out,
            "/* ends *\\/ here, C:\\users \\\\u0041 */ /* ends *\\/ here, C:\\\\users \\\\u0041 */"
        );
    }

    #[test]
    fn quote_escapes_the_separators_a_csharp_literal_ends_a_line_at() {
        let out = rendered(
            "{{ keys[0].name | quote }}",
            json!([key("a\u{2028}b\u{85}", "string", json!([]))]),
        );
        assert_eq!(out, "\"a\\u2028b\\u0085\"");
    }

    #[test]
    fn json_writes_a_list_a_template_can_paste() {
        let out = rendered(
            "{{ keys[0].type.members | json }}",
            json!([key("NODE_ENV", "enum", json!(["a", "b"]))]),
        );
        assert_eq!(out, "[\"a\",\"b\"]");
    }

    #[test]
    fn the_options_table_reaches_the_template_whatever_it_holds() {
        let mut target = target(
            "{{ options.key_case }} {{ options.width }}",
            "values | join('|')",
        );
        target.options = toml::from_str("key_case = \"camel\"\nwidth = 80\n").unwrap();
        let out = render(
            &target,
            &schema(json!([key("PORT", "port", json!([]))])),
            "9.9.9",
        )
        .unwrap();
        assert_eq!(out, "camel 80");
    }

    #[test]
    fn a_target_with_no_options_still_lets_a_template_ask() {
        let out = rendered(
            "{% if options.key_case is defined %}set{% else %}absent{% endif %}",
            json!([key("PORT", "port", json!([]))]),
        );
        assert_eq!(out, "absent");
    }

    #[test]
    fn a_target_missing_a_type_the_schema_uses_says_which_one() {
        let mut target = target("{{ keys[0].lang_type }}", "values | join('|')");
        target.types.remove("port");
        let error = render(
            &target,
            &schema(json!([key("PORT", "port", json!([]))])),
            "9.9.9",
        )
        .unwrap_err();
        assert_eq!(
            error,
            Error::NoTypeFor {
                target: "fake".into(),
                base: "port".into()
            }
        );
    }

    #[test]
    fn a_built_in_folder_is_a_folder_like_any_other() {
        for built_in in crate::BUILT_IN {
            parse(
                built_in.name,
                &toml::from_str(built_in.file("target.toml").unwrap()).unwrap(),
                built_in.file("env.tmpl").unwrap(),
                Source::BuiltIn,
                Source::BuiltIn,
                "built in",
            )
            .unwrap();
        }
    }

    #[test]
    fn a_whole_number_takes_the_integer_entry_and_a_plain_number_does_not() {
        let mut whole = key("MAX_RETRIES", "number", json!([]));
        whole["type"]["constraints"] = json!({ "isInt": "true" });
        let out = rendered(
            "{% for key in keys %}{{ key.lang_type }};{% endfor %}",
            json!([whole, key("CACHE_TTL", "number", json!([]))]),
        );
        assert_eq!(out, "int;number;");
    }

    #[test]
    fn a_target_with_no_integer_entry_falls_back_to_number() {
        let mut target = target("{{ keys[0].lang_type }}", "values | join('|')");
        target.types.remove("integer");
        let mut whole = key("MAX_RETRIES", "number", json!([]));
        whole["type"]["constraints"] = json!({ "isInt": "true" });
        let out = render(&target, &schema(json!([whole])), "9.9.9").unwrap();
        assert_eq!(out, "number");
    }
}
