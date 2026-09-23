//! One fixture schema through every built-in target, byte for byte.

use penv_targets::{Roots, Source, Target, Tree, load, render};

const FIXTURE: &str = include_str!("fixture.env.schema");
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
    let schema = penv_schema::parse(FIXTURE).expect("the fixture parses");
    render(&built_in(name), &view(&schema), VERSION).expect("the fixture renders")
}

/// Point at the first line that differs; the whole file is too long to read in a
/// test failure.
fn assert_same(name: &str, expected: &str, actual: &str) {
    if expected == actual {
        return;
    }
    if std::env::var_os("PENV_BLESS").is_some() {
        std::fs::write(format!("tests/snapshots/{name}"), actual).unwrap();
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

#[test]
fn the_ts_target_renders_its_snapshot() {
    assert_same("ts.env.ts", TS, &rendered("ts"));
}

#[test]
fn the_py_target_renders_its_snapshot() {
    assert_same("py.penv_env.py", PY, &rendered("py"));
}

#[test]
fn the_go_rust_php_java_and_csharp_targets_render_their_snapshots() {
    for (name, file, snapshot) in [
        ("go", "go.env.go", GO),
        ("rust", "rust.env.rs", RUST),
        ("php", "php.Env.php", PHP),
        ("java", "java.Env.java", JAVA),
        ("csharp", "csharp.Env.g.cs", CSHARP),
    ] {
        assert_same(file, snapshot, &rendered(name));
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

/// The schema JSON as `penv gen` hands it over, with each key marked public or not.
fn view(schema: &penv_schema::Schema) -> serde_json::Value {
    let mut json = schema.to_json();
    for key in json["keys"].as_array_mut().expect("keys") {
        let public = schema.is_public(key["name"].as_str().unwrap_or_default());
        key["public"] = serde_json::Value::Bool(public);
    }
    json
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
    let out = render(&target, &schema.to_json(), VERSION).expect("the fixture renders");
    assert!(out.contains("from pydantic import HttpUrl, SecretStr"));
    assert!(out.contains("self.STRIPE_SECRET_KEY: SecretStr = SecretStr("));
    assert!(
        !PY.contains("pydantic"),
        "the default is the standard library"
    );
    assert!(
        PY.contains("self.STRIPE_SECRET_KEY: str = _require("),
        "{PY}"
    );
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
