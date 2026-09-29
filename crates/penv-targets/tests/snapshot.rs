//! One fixture schema through every built-in target, byte for byte.

use penv_targets::{BUILT_IN, Roots, Source, Target, Tree, load, render};

const FIXTURE: &str = include_str!("fixture.env.schema");
/// Only string-shaped keys, so nothing parses, and no keys at all.
const STRINGS: &str = include_str!("strings.env.schema");
const EMPTY: &str = include_str!("empty.env.schema");
/// Names each language reserves or its loader already uses, and a default and
/// a description holding the characters literals and comments treat specially.
const RESERVED: &str = include_str!("reserved.env.schema");

const TS: &str = include_str!("snapshots/ts.env.ts");
const PY: &str = include_str!("snapshots/py.penv_env.py");
const GO: &str = include_str!("snapshots/go.env.go");
const RUST: &str = include_str!("snapshots/rust.env.rs");
const PHP: &str = include_str!("snapshots/php.Env.php");
const JAVA: &str = include_str!("snapshots/java.Env.java");
const CSHARP: &str = include_str!("snapshots/csharp.Env.g.cs");

/// Pinned so a version bump is not a snapshot change.
const VERSION: &str = "1.0.0-test";

struct Empty;

impl Tree for Empty {
    fn read(&self, _path: &str) -> Option<String> {
        None
    }
    fn dirs(&self, _path: &str) -> Vec<String> {
        Vec::new()
    }
}

fn built_in(name: &str) -> Target {
    let target = load(&Empty, &Roots::new("/repo", None), name).unwrap();
    assert_eq!(target.source, Source::BuiltIn);
    target
}

fn rendered(name: &str) -> String {
    rendered_from(FIXTURE, name)
}

fn rendered_from(fixture: &str, name: &str) -> String {
    let schema = penv_schema::parse(fixture).expect("the fixture parses");
    render(&built_in(name), &view(&schema), VERSION).expect("the fixture renders")
}

fn snapshot(path: &str) -> String {
    std::fs::read_to_string(format!("tests/snapshots/{path}")).unwrap_or_default()
}

/// Point at the first line that differs; the whole file is too long to read in a
/// test failure.
fn assert_same(name: &str, expected: &str, actual: &str) {
    if expected == actual {
        return;
    }
    if std::env::var_os("PENV_BLESS").is_some() {
        let path = format!("tests/snapshots/{name}");
        if let Some(parent) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, actual).unwrap();
        panic!("{name} was rewritten from PENV_BLESS; read the diff and run again");
    }
    let mut hint = String::new();
    for (line, (want, got)) in expected.lines().zip(actual.lines()).enumerate() {
        if want != got {
            hint = format!("line {}:\n-{want}\n+{got}", line + 1);
            break;
        }
    }
    if hint.is_empty() {
        hint = format!(
            "same prefix, {} expected line(s) against {} rendered",
            expected.lines().count(),
            actual.lines().count()
        );
    }
    panic!(
        "tests/snapshots/{name} no longer matches the fixture render.\n{hint}\nRe-run with PENV_BLESS=1 once the change is deliberate."
    );
}

/// Every built-in target and the file its snapshot is kept under: its name, then
/// the file name of its output.
fn files() -> Vec<(&'static str, String)> {
    BUILT_IN
        .iter()
        .map(|b| {
            let output = built_in(b.name).output;
            let file = output.rsplit('/').next().unwrap_or(&output).to_string();
            (b.name, format!("{}.{file}", b.name))
        })
        .collect()
}

#[test]
fn every_built_in_target_renders_its_snapshot() {
    for (name, file) in files() {
        assert_same(&file, &snapshot(&file), &rendered(name));
    }
}

#[test]
fn every_new_target_hides_secrets_behind_a_redacting_type() {
    for snapshot in [GO, RUST, PHP, JAVA, CSHARP] {
        assert!(snapshot.contains("[redacted]"));
        assert!(snapshot.contains("Secret"));
        // A problem message names the key and what it should be, never a value.
        assert!(snapshot.contains("is not set; run penv check"));
    }
}

/// The schema JSON exactly as `penv gen` hands it over.
fn view(schema: &penv_schema::Schema) -> serde_json::Value {
    penv_targets::view(schema, "development")
}

#[test]
fn a_schema_of_only_strings_or_of_nothing_still_renders_through_every_target() {
    for (dir, fixture) in [
        ("strings", STRINGS),
        ("empty", EMPTY),
        ("reserved", RESERVED),
    ] {
        for (name, file) in files() {
            let path = format!("{dir}/{file}");
            assert_same(&path, &snapshot(&path), &rendered_from(fixture, name));
        }
    }
}

#[test]
fn a_key_named_like_a_keyword_or_a_loader_local_is_escaped_per_language() {
    let rust = snapshot("reserved/rust.env.rs");
    for want in [
        "pub r#type: Option<",
        "pub r#match:",
        "pub self_:",
        "let r#type = Self::raw(",
    ] {
        assert!(rust.contains(want), "{want}");
    }
    assert!(rust.contains("__penv_problems"));
    let php = snapshot("reserved/php.Env.php");
    assert!(
        php.contains("$this_,") && php.contains("$__penvRaw("),
        "{php}"
    );
    let java = snapshot("reserved/java.Env.java");
    for want in [
        "Secret class_,",
        "Secret default_,",
        "Secret load_,",
        "Secret toString_,",
        "__penvProblems",
    ] {
        assert!(java.contains(want), "{want}");
    }
    let csharp = snapshot("reserved/csharp.Env.g.cs");
    for want in [
        "Secret? Load_,",
        "Secret? ToString_,",
        "var @class =",
        "            @default,",
        "__PenvRaw(",
    ] {
        assert!(csharp.contains(want), "{want}");
    }
    assert!(snapshot("reserved/py.penv_env.py").contains("self.None_:"));
}

#[test]
fn a_default_and_a_description_keep_their_characters_in_every_language() {
    assert!(snapshot("reserved/php.Env.php").contains("'Hi $name, from C:\\\\users'"));
    assert!(snapshot("reserved/rust.env.rs").contains("\"Hi $name, from C:\\\\users\""));
    assert!(
        snapshot("reserved/ts.env.ts")
            .contains("/** Kept in C:\\users\\penv, never in *\\/tmp. */")
    );
    assert!(
        snapshot("reserved/java.Env.java")
            .contains("/* Kept in C:\\\\users\\penv, never in *\\/tmp. */")
    );
}

#[test]
fn a_public_key_computed_from_a_secret_is_read_and_masked_like_one() {
    for runtime in ["node", "vite"] {
        let mut target = built_in("ts");
        target
            .options
            .insert("runtime".into(), toml::Value::String(runtime.into()));
        let schema = penv_schema::parse(FIXTURE).expect("the fixture parses");
        let out = render(&target, &view(&schema), VERSION).unwrap();
        assert!(
            !out.contains("env.NEXT_PUBLIC_CHECKOUT_TOKEN"),
            "{runtime} inlines it"
        );
        let mask = &out[out.find("values = [").unwrap()..out.find("].filter(").unwrap()];
        assert!(
            mask.contains("read(\"NEXT_PUBLIC_CHECKOUT_TOKEN\")"),
            "{mask}"
        );
    }
    assert!(
        !TS.contains("${STRIPE_SECRET_KEY}") && !PY.contains("${STRIPE_SECRET_KEY}"),
        "a computed default is never a literal fallback"
    );
}

#[test]
fn the_ts_target_reads_an_empty_value_as_unset_and_every_boolean_word() {
    assert!(
        TS.contains("Number(given(read(\"PORT\"), \"3000\"))"),
        "{TS}"
    );
    assert!(TS.contains("flag(given(read(\"FEATURE_BILLING\"), \"false\"), \"FEATURE_BILLING\")"));
    assert!(TS.contains("[\"1\", \"true\", \"yes\", \"on\"]"));
    assert!(!TS.contains("=== \"true\""));
    let schema = penv_schema::parse(
        "# @type=number(isInt=false) @sensitive=false\nRATIO=\n\n# @type=number(isInt=true) @sensitive=false\nCOUNT=\n",
    )
    .unwrap();
    let out = render(&built_in("ts"), &view(&schema), VERSION).unwrap();
    assert!(out.contains("RATIO must be a number"), "{out}");
    assert!(out.contains("COUNT must be a whole number"), "{out}");
}

/// `key_case` is the folder's own option, so a repo-local ts target can flip it.
#[test]
fn the_ts_target_renames_its_properties_when_key_case_is_camel() {
    let mut target = built_in("ts");
    target
        .options
        .insert("key_case".into(), toml::Value::String("camel".into()));
    let schema = penv_schema::parse(FIXTURE).expect("the fixture parses");
    let out = render(&target, &view(&schema), VERSION).expect("the fixture renders");
    assert!(out.contains("  get nextPublicAppUrl() {"), "{out}");
    assert!(out.contains("  get databaseUrl() {"), "{out}");
    assert!(
        out.contains("seen[\"DATABASE_URL\"]"),
        "the schema reads the environment, whatever the properties are called"
    );
    assert!(out.contains("path: [\"DATABASE_URL\"]"));
    assert!(out.contains("message: \"DATABASE_URL is required\""));
}

/// `runtime` is the folder's own option too, and the accessor is the only line
/// that changes with it.
#[test]
fn the_ts_target_reads_through_the_runtime_the_options_name() {
    let reads = |runtime: &str| {
        let mut target = built_in("ts");
        target
            .options
            .insert("runtime".into(), toml::Value::String(runtime.into()));
        let schema = penv_schema::parse(FIXTURE).expect("the fixture parses");
        render(&target, &view(&schema), VERSION).expect("the fixture renders")
    };
    // A public key is spelled out, the only form a bundler inlines.
    assert!(reads("vite").contains("import.meta.env.NEXT_PUBLIC_APP_URL"));
    assert!(reads("node").contains("process.env.NEXT_PUBLIC_APP_URL"));
    assert!(reads("deno").contains("Deno.env.get(\"NEXT_PUBLIC_APP_URL\")"));
    assert!(reads("workers").contains("from \"cloudflare:workers\""));
    // A secret never is, in any runtime, so no bundler can inline it.
    for runtime in ["node", "vite", "deno", "workers"] {
        let out = reads(runtime);
        assert!(out.contains("read(\"DATABASE_URL\")"), "{runtime}: {out}");
        for literal in [
            "process.env.DATABASE_URL",
            "import.meta.env.DATABASE_URL",
            "Deno.env.get(\"DATABASE_URL\")",
        ] {
            assert!(!out.contains(literal), "{runtime} wrote {literal}");
        }
    }
}

/// Stdlib only unless the folder asks for pydantic, so an import nobody has
/// never lands in a generated file.
#[test]
fn the_py_target_takes_pydantic_types_only_when_its_options_ask_for_them() {
    let mut target = built_in("py");
    target
        .options
        .insert("pydantic".into(), toml::Value::Boolean(true));
    let schema = penv_schema::parse(FIXTURE).expect("the fixture parses");
    let out = render(&target, &view(&schema), VERSION).expect("the fixture renders");
    assert!(out.contains("from pydantic import HttpUrl, SecretStr"));
    assert!(out.contains("self.STRIPE_SECRET_KEY: SecretStr = _secret("));
    assert!(
        !PY.contains("pydantic"),
        "the default is the standard library"
    );
    assert!(PY.contains("self.STRIPE_SECRET_KEY: str = _raw("), "{PY}");
}

#[test]
fn a_constraint_never_reaches_the_generated_file() {
    for snapshot in [TS, PY, GO, RUST, PHP, JAVA, CSHARP] {
        assert!(
            !snapshot.contains("sk_"),
            "startsWith=sk_ leaked into the output"
        );
    }
}

#[test]
fn the_ts_target_declares_inlined_only_when_a_public_key_reads_through_it() {
    let schema = penv_schema::parse(
        "# @type=string\nSTRIPE_SECRET_KEY=\n\n# @type=port @sensitive=false\nPORT=3000\n",
    )
    .unwrap();
    let out = render(&built_in("ts"), &view(&schema), VERSION).unwrap();
    assert!(
        !out.contains("const inlined"),
        "unused under noUnusedLocals"
    );
    assert!(
        TS.contains("const inlined"),
        "the fixture's public keys still read through it"
    );
    assert!(TS.contains("static override json"), "noImplicitOverride");
}
