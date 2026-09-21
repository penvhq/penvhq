use penv_dotenv::{WriteError, ensure_ignored, infer, read, write};
use penv_schema::{BaseType, render};

fn codes(warnings: &[penv_dotenv::Warning]) -> Vec<&str> {
    warnings.iter().map(|w| w.code).collect()
}

#[test]
fn reads_the_safe_subset() {
    let env = read("A_KEY=one\nB_KEY=two\n");
    assert_eq!(
        env.entries
            .iter()
            .map(|e| (e.key.as_str(), e.value.as_str()))
            .collect::<Vec<_>>(),
        [("A_KEY", "one"), ("B_KEY", "two")]
    );
    assert!(env.warnings.is_empty());
}

#[test]
fn tolerates_what_real_files_contain() {
    let source = "\u{feff}# a comment\r\n\r\nexport A_KEY = one\r\nB_KEY='two three'\r\nC_KEY=\"four\"\r\nD_KEY=five # trailing\r\n";
    let env = read(source);
    assert_eq!(env.get("A_KEY"), Some("one"));
    assert_eq!(env.get("B_KEY"), Some("two three"));
    assert_eq!(env.get("C_KEY"), Some("four"));
    assert_eq!(env.get("D_KEY"), Some("five"));
    let found = codes(&env.warnings);
    for expected in [
        "bom",
        "crlf",
        "export_prefix",
        "spaces_around_equals",
        "inline_comment",
    ] {
        assert!(found.contains(&expected), "missing {expected} in {found:?}");
    }
}

#[test]
fn warns_on_constructs_outside_the_subset() {
    let env =
        read("A_KEY=one\nA_KEY=two\nB_KEY=$A_KEY\nC_KEY=\"line one\nline two\"\nnot a pair\n");
    assert_eq!(env.get("A_KEY"), Some("two"), "the last value wins");
    assert_eq!(env.get("C_KEY"), Some("line one\nline two"));
    let found = codes(&env.warnings);
    for expected in [
        "duplicate_key",
        "interpolation",
        "multiline_value",
        "invalid_line",
    ] {
        assert!(found.contains(&expected), "missing {expected} in {found:?}");
    }
}

#[test]
fn writes_only_the_safe_subset() {
    let out = write(&[
        ("A_KEY", "one"),
        ("B_KEY", "two three"),
        ("C_KEY", ""),
        ("D_KEY", "has#hash"),
    ])
    .unwrap();
    assert_eq!(
        out,
        "A_KEY=one\nB_KEY=\"two three\"\nC_KEY=\nD_KEY=\"has#hash\"\n"
    );
}

#[test]
fn the_writer_refuses_what_the_subset_excludes() {
    assert_eq!(
        write(&[("A_KEY", "$OTHER")]),
        Err(WriteError::Interpolation {
            key: "A_KEY".into()
        })
    );
    assert_eq!(
        write(&[("A_KEY", "one"), ("A_KEY", "two")]),
        Err(WriteError::DuplicateKey {
            key: "A_KEY".into()
        })
    );
    assert_eq!(
        write(&[("a-key", "one")]),
        Err(WriteError::InvalidKey {
            key: "a-key".into()
        })
    );
    // A line break needs double quotes, and a backslash cannot be written inside
    // them without an escape no dialect agrees on.
    assert_eq!(
        write(&[("A_KEY", "one\ntwo\\three")]),
        Err(WriteError::Unquotable {
            key: "A_KEY".into()
        })
    );
    assert_eq!(
        write(&[("A_KEY", r"a \ and a '")]),
        Err(WriteError::Unquotable {
            key: "A_KEY".into()
        })
    );
}

#[test]
fn every_shape_a_real_value_takes_round_trips() {
    let pairs = [
        (
            "PEM_KEY",
            "-----BEGIN PRIVATE KEY-----\nZmFrZWtleQ==\n-----END PRIVATE KEY-----\n",
        ),
        ("WINDOWS_PATH", r"C:\Users\example\.penv"),
        ("TRAILING_SLASH", r"C:\Users\example\"),
        ("REGEX", r"\d+\.\d+"),
        ("QUOTED_HASH", "say \"hi\" #1"),
        ("LEADING_NBSP", "\u{a0}indented"),
        ("PLAIN", "one"),
        ("EMPTY", ""),
    ];
    let text = write(&pairs).unwrap();
    assert_eq!(
        text.lines().count(),
        pairs.len(),
        "a value spilled onto its own line: {text:?}"
    );
    assert!(text.contains(r"\n-----END"), "{text}");

    let env = read(&text);
    assert!(env.warnings.is_empty(), "{:?}", env.warnings);
    for (key, value) in pairs {
        assert_eq!(env.get(key), Some(value), "wrote {text:?}");
    }
}

#[test]
fn an_escape_outside_the_subset_is_named_by_its_column_and_never_its_character() {
    let env = read("A_KEY=\"a \\x b\"\n");
    assert_eq!(env.get("A_KEY"), Some("a x b"));
    let warning = env
        .warnings
        .iter()
        .find(|w| w.code == "escape_sequences")
        .expect("the unknown escape was not reported");
    assert_eq!(warning.line, 1);
    assert_eq!(warning.message, "the escape at column 10 is outside \\n");
    assert_eq!(
        env.warnings
            .iter()
            .filter(|w| w.code == "escape_sequences")
            .count(),
        1,
        "one warning per value"
    );

    let subset = read("B_KEY=\"line\\none\"\n");
    assert_eq!(subset.get("B_KEY"), Some("line\none"));
    assert!(subset.warnings.is_empty(), "{:?}", subset.warnings);

    // Node's util.parseEnv decodes \n and nothing else, so \r is a dialect too.
    let carriage = read("C_KEY=\"line\\rone\"\n");
    assert_eq!(carriage.get("C_KEY"), Some("line\rone"));
    assert_eq!(codes(&carriage.warnings), ["escape_sequences"]);
}

#[test]
fn an_escape_on_a_continuation_line_is_named_where_it_sits() {
    let env = read("A_KEY=\"line one\n\\x line two\"\n");
    assert_eq!(env.get("A_KEY"), Some("line one\nx line two"));
    let warning = env
        .warnings
        .iter()
        .find(|w| w.code == "escape_sequences")
        .expect("the unknown escape was not reported");
    assert_eq!(warning.line, 2, "the escape sits on the second line");
    assert_eq!(warning.message, "the escape at column 1 is outside \\n");
}

#[test]
fn a_crlf_value_is_written_and_read_back_as_the_lf_one_it_means() {
    let pem = "-----BEGIN PRIVATE KEY-----\r\nZmFrZWtleQ==\r\n-----END PRIVATE KEY-----";
    let text = write(&[("PEM_KEY", pem)]).unwrap();
    assert!(!text.contains('\r'), "{text:?}");
    assert!(text.contains(r"\nZmFrZWtleQ==\n"), "{text:?}");

    let env = read(&text);
    assert!(env.warnings.is_empty(), "{:?}", env.warnings);
    assert_eq!(
        env.get("PEM_KEY"),
        Some("-----BEGIN PRIVATE KEY-----\nZmFrZWtleQ==\n-----END PRIVATE KEY-----")
    );

    assert_eq!(
        write(&[("A_KEY", "one\rtwo"), ("B_KEY", "https://example.test\r")]).unwrap(),
        "A_KEY=\"one\\ntwo\"\nB_KEY=https://example.test\n",
        "a carriage return never blocks a pull"
    );
}

#[test]
fn what_the_writer_emits_reads_back_unchanged() {
    let pairs = [("A_KEY", "one two"), ("B_KEY", "has#hash"), ("C_KEY", "")];
    let text = write(&pairs).unwrap();
    let env = read(&text);
    assert!(env.warnings.is_empty(), "{:?}", env.warnings);
    for (key, value) in pairs {
        assert_eq!(env.get(key), Some(value));
    }
}

#[test]
fn infers_types_from_values() {
    let env = read(
        "APP_URL=https://app.example.test\nPORT=3000\nDEBUG=true\nWORKERS=4\nRATE=1.5\nOWNER_EMAIL=dev@example.test\nAPP_NAME=demo\n",
    );
    let schema = infer(&env);
    let ty = |name: &str| schema.get(name).unwrap().ty.base;
    assert_eq!(ty("APP_URL"), BaseType::Url);
    assert_eq!(ty("PORT"), BaseType::Port);
    assert_eq!(ty("DEBUG"), BaseType::Boolean);
    assert_eq!(ty("WORKERS"), BaseType::Number);
    assert_eq!(
        schema.get("WORKERS").unwrap().ty.constraint("isInt"),
        Some("true"),
        "a whole number is number(isInt=true)"
    );
    assert_eq!(ty("RATE"), BaseType::Number);
    assert_eq!(ty("OWNER_EMAIL"), BaseType::Email);
    assert_eq!(ty("APP_NAME"), BaseType::String);
    assert!(
        schema.keys.iter().all(|k| k.ty.base != BaseType::Enum),
        "enum is never inferred"
    );
}

#[test]
fn every_key_is_sensitive_unless_a_prefix_or_a_dull_value_says_otherwise() {
    let env = read(concat!(
        "STRIPE_SECRET_KEY=sk_test_0000000000
",
        "SESSION_JWT=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJmYWtlIn0.c2lnbmF0dXJlZmFrZQ
",
        "DATABASE_URL=postgres://someone:fakepassword@localhost:5432/app
",
        "APP_NAME=demo
",
        "NEXT_PUBLIC_ANALYTICS_ID=pk_public_fake
",
    ));
    let schema = infer(&env);
    let sensitive = |name: &str| schema.get(name).unwrap().sensitive;
    assert!(sensitive("STRIPE_SECRET_KEY"));
    assert!(sensitive("SESSION_JWT"));
    assert!(
        sensitive("DATABASE_URL"),
        "a connection string is never copied, however it is spelled"
    );
    assert!(!sensitive("APP_NAME"), "a lowercase word is not a secret");
    assert!(
        !sensitive("NEXT_PUBLIC_ANALYTICS_ID"),
        "a bundler prefix ships the value to the browser anyway"
    );
    assert_eq!(
        schema
            .get("NEXT_PUBLIC_ANALYTICS_ID")
            .unwrap()
            .sensitive_decorator,
        None,
        "the prefix rule needs no decorator"
    );
}

#[test]
fn a_prefixed_key_named_for_a_credential_is_public_but_its_value_stays_out() {
    let env = read(
        "NEXT_PUBLIC_SUPABASE_ANON_KEY=eyfake
",
    );
    let key = schema_key(&env, "NEXT_PUBLIC_SUPABASE_ANON_KEY");
    assert!(!key.sensitive, "the prefix says the browser reads it");
    assert_eq!(key.default, None, "the name says what it holds");
    assert_eq!(key.sensitive_decorator, None);
}

#[test]
fn the_credential_words_match_their_plurals_in_both_spellings() {
    let env = read(concat!(
        "SMTP_PASSES=hunter\n",
        "MY_PASSWD=hunter\n",
        "CRED=hunter\n",
        "DB_PASS=hunter\n",
        "TOKEN_TTL=300\n",
        "PORT=3000\n",
    ));
    for vetoed in ["SMTP_PASSES", "MY_PASSWD", "CRED", "DB_PASS", "TOKEN_TTL"] {
        let key = schema_key(&env, vetoed);
        assert_eq!(key.default, None, "{vetoed} carried its value out");
        assert!(key.sensitive, "{vetoed} is not sensitive");
    }
    let port = schema_key(&env, "PORT");
    assert_eq!(port.default.as_deref(), Some("3000"));
    assert!(!port.sensitive);
}

fn schema_key(env: &penv_dotenv::Dotenv, name: &str) -> penv_schema::Key {
    infer(env).get(name).expect("the key").clone()
}

#[test]
fn sensitive_values_never_reach_the_schema() {
    let env = read("STRIPE_SECRET_KEY=sk_test_0000000000\nAPP_NAME=demo\nPORT=3000\n");
    let schema = infer(&env);
    let secret = schema.get("STRIPE_SECRET_KEY").unwrap();
    assert_eq!(secret.default, None);
    assert!(secret.required);
    assert_eq!(
        schema.get("APP_NAME").unwrap().default.as_deref(),
        Some("demo")
    );
    assert!(!schema.get("APP_NAME").unwrap().required);

    let rendered = render(&schema);
    assert!(!rendered.contains("sk_test_0000000000"));
    assert!(rendered.contains("STRIPE_SECRET_KEY=\n"));
}

#[test]
fn an_inferred_schema_parses_back() {
    let env =
        read("APP_URL=https://app.example.test\nSTRIPE_SECRET_KEY=sk_test_0000000000\nPORT=3000\n");
    let schema = infer(&env);
    let reparsed = penv_schema::parse(&render(&schema)).expect("the draft parses");
    assert_eq!(schema, reparsed);
}

#[test]
fn the_gitignore_helper_is_idempotent() {
    let first = ensure_ignored("node_modules/\n");
    assert_eq!(first.added, [".env", ".env.*", "!.env.schema"]);
    assert!(first.content.contains("\n.env\n"));
    assert!(first.content.contains("\n.env.*\n"));
    assert!(
        first.content.contains("\n!.env.schema\n"),
        ".env.* would otherwise swallow the one committed penv file"
    );

    let second = ensure_ignored(&first.content);
    assert!(second.added.is_empty());
    assert_eq!(second.content, first.content);
    assert!(!second.changed());
}

#[test]
fn the_gitignore_helper_adds_only_what_is_missing() {
    let update = ensure_ignored("/.env\ndist/\n");
    assert_eq!(update.added, [".env.*", "!.env.schema"]);
}

#[test]
fn awkward_values_round_trip_through_the_writer() {
    let values = [
        "plain",
        "one two",
        "has#hash",
        r"back\slash",
        r"C:\Users\dev\app",
        r"back\slash and a space",
        "quote\"inside",
        "apostrophe'inside",
        "trailing space ",
        "=equals=",
        "",
    ];
    for value in values {
        let text = write(&[("A_KEY", value)]).unwrap_or_else(|e| panic!("{value:?}: {e}"));
        let env = read(&text);
        assert_eq!(env.get("A_KEY"), Some(value), "wrote {text:?}");
        assert!(env.warnings.is_empty(), "{value:?}: {:?}", env.warnings);
    }
}

#[test]
fn only_a_dull_value_is_copied_into_the_committed_schema() {
    let env = read(concat!(
        "SLACK_WEBHOOK_URL=https://hooks.slack.test/services/T00/B00/xoxbFAKETOKEN\n",
        "DATABASE_URL=postgres://app.example.test/db?password=hunter2\n",
        "SESSION_SEED=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
        "ADMIN_PW=hunter2\n",
        "NODE_ENV=development\n",
        "PORT=3000\n",
        "NEXT_PUBLIC_APP_URL=https://example.test/x?y=1\n",
        "DEBUG=true\n",
        "API_URL=http://localhost:3000\n",
    ));
    let schema = infer(&env);
    let default = |name: &str| schema.get(name).unwrap().default.clone();
    let sensitive = |name: &str| schema.get(name).unwrap().sensitive;
    let required = |name: &str| schema.get(name).unwrap().required;

    for kept in [
        "SLACK_WEBHOOK_URL",
        "DATABASE_URL",
        "SESSION_SEED",
        "ADMIN_PW",
    ] {
        assert_eq!(
            default(kept),
            None,
            "{kept} carried its value into the schema"
        );
        assert!(sensitive(kept), "{kept} is not sensitive");
        assert!(required(kept), "{kept} is not required");
    }
    for copied in [
        "NODE_ENV",
        "PORT",
        "NEXT_PUBLIC_APP_URL",
        "DEBUG",
        "API_URL",
    ] {
        assert!(default(copied).is_some(), "{copied} lost its value");
        assert!(!sensitive(copied), "{copied} is sensitive but was copied");
        assert!(
            !required(copied),
            "{copied} has a default, so it is optional"
        );
    }

    let rendered = render(&schema);
    for secret in ["xoxbFAKETOKEN", "hunter2", "0123456789abcdef"] {
        assert!(!rendered.contains(secret), "{secret} reached the schema");
    }
    assert!(rendered.contains("NEXT_PUBLIC_APP_URL=https://example.test/x?y=1"));
    assert_eq!(penv_schema::parse(&rendered).unwrap(), schema);
}

#[test]
fn a_lowercase_slug_is_dull_enough_for_the_schema_and_a_token_is_not() {
    let env = read(concat!(
        "NODE_ENV=development\n",
        "AWS_REGION=us-east-1\n",
        "MODEL=gpt-4o\n",
        "SERVICE_HOST=api.internal\n",
        "API_TOKEN=a3f9c2d4e5b6\n",
        "REQUEST_ID=7f3a91b2c4d5e6f708192a3b4c5d6e7f\n",
        "TENANT_ID=acme-0123456789abcdef0123456789abcd\n",
    ));
    let schema = infer(&env);
    let default = |name: &str| schema.get(name).unwrap().default.clone();
    for copied in ["NODE_ENV", "AWS_REGION", "MODEL", "SERVICE_HOST"] {
        assert!(default(copied).is_some(), "{copied} lost its value");
    }
    for kept in ["API_TOKEN", "REQUEST_ID", "TENANT_ID"] {
        assert_eq!(default(kept), None, "{kept} reached the schema");
    }
}

#[test]
fn a_key_that_says_what_it_holds_keeps_its_value_whatever_the_value_looks_like() {
    let env = read(concat!(
        "STRIPE_SECRET_KEY=sk_test_0000\n",
        "ADMIN_PASSWORD=hunter\n",
        "SESSION_SALT=pepper\n",
        "DB_PASS=letmein\n",
        "API_KEYS=one\n",
        "AUTH_HEADER=bearer\n",
        "NEXT_PUBLIC_SUPABASE_ANON_KEY=eyfake\n",
        "APP_MODE=maintenance\n",
    ));
    let schema = infer(&env);
    let default = |name: &str| schema.get(name).unwrap().default.clone();
    for kept in [
        "STRIPE_SECRET_KEY",
        "ADMIN_PASSWORD",
        "SESSION_SALT",
        "DB_PASS",
        "API_KEYS",
        "AUTH_HEADER",
        "NEXT_PUBLIC_SUPABASE_ANON_KEY",
    ] {
        assert_eq!(default(kept), None, "{kept} reached the schema");
    }
    assert!(
        schema.get("DB_PASS").unwrap().sensitive,
        "a copied value would have turned masking off"
    );
    assert_eq!(default("APP_MODE").as_deref(), Some("maintenance"));
}

#[test]
fn types_are_inferred_even_when_the_value_stays_out() {
    let env = read(concat!(
        "DATABASE_URL=postgres://app.example.test/db?password=hunter2\n",
        "SMTP_PORT=2525\n",
        "OWNER_EMAIL=dev@example.test\n",
        "RETRY_BUDGET=7\n",
        "SAMPLE_RATE=0.25\n",
    ));
    let schema = infer(&env);
    let ty = |name: &str| schema.get(name).unwrap().ty.clone();
    assert_eq!(ty("DATABASE_URL").base, BaseType::Url);
    assert_eq!(ty("SMTP_PORT").base, BaseType::Port);
    assert_eq!(ty("OWNER_EMAIL").base, BaseType::Email);
    assert_eq!(ty("RETRY_BUDGET").constraint("isInt"), Some("true"));
    assert_eq!(ty("SAMPLE_RATE").base, BaseType::Number);
    assert_eq!(
        schema.get("DATABASE_URL").unwrap().default,
        None,
        "the type came from the value, the value stayed out"
    );
}
