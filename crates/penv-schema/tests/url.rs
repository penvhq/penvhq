use std::collections::{BTreeMap, BTreeSet};

use penv_schema::functions::Policy;
use penv_schema::resolve::{Raw, resolve, resolve_full, tainted};
type Values = BTreeMap<String, String>;

const URL: &str = "postgres://app:fakepass@db.internal.test:5432/orders?sslmode=require";

fn run(url: &str, expr: &str) -> Result<String, String> {
    let raw = BTreeMap::from([
        ("U".to_string(), Raw::literal(url)),
        ("PW".to_string(), Raw::literal("fake pw/1")),
        ("R".to_string(), Raw::computed(expr)),
    ]);
    let (values, errors) = resolve(
        &raw,
        &Values::new(),
        "dev",
        &Values::new(),
        &Policy::default(),
    );
    match values.get("R") {
        Some(v) if errors.is_empty() => Ok(v.clone()),
        _ => Err(format!("{errors:?}")),
    }
}

fn read(url: &str, part: &str) -> Result<String, String> {
    run(url, &format!("url($U, {part})"))
}

#[test]
fn every_part_of_the_example_reads() {
    for (part, want) in [
        ("scheme", "postgres"),
        ("host", "db.internal.test"),
        ("port", "5432"),
        ("origin", "postgres://db.internal.test:5432"),
        ("path", "/orders"),
        ("user", "app"),
        ("password", "fakepass"),
        ("query.sslmode", "require"),
    ] {
        assert_eq!(read(URL, part).as_deref(), Ok(want), "{part}");
    }
}

#[test]
fn missing_parts_are_empty_and_fallback_supplies_a_default() {
    let bare = "postgres://db.internal.test";
    for part in ["port", "path", "user", "password", "query.sslmode"] {
        assert_eq!(read(bare, part).as_deref(), Ok(""), "{part}");
    }
    assert_eq!(
        run(bare, "fallback(url($U, port), 5432)").as_deref(),
        Ok("5432")
    );
}

#[test]
fn user_password_and_query_values_are_decoded() {
    let u = "x://a%40b:p%3Aw@h.test/?k=a%20b";
    assert_eq!(read(u, "user").as_deref(), Ok("a@b"));
    assert_eq!(read(u, "password").as_deref(), Ok("p:w"));
    assert_eq!(read(u, "query.k").as_deref(), Ok("a b"));
}

#[test]
fn several_hosts_fail_host_and_port_only() {
    let multi = "mongodb://u:fakepass@a.test:1,b.test:2/db";
    for part in ["host", "port"] {
        let err = read(multi, part).unwrap_err();
        assert!(
            err.contains("several hosts") && !err.contains("fakepass"),
            "{err}"
        );
    }
    assert_eq!(read(multi, "scheme").as_deref(), Ok("mongodb"));
}

#[test]
fn a_non_url_fails_every_part() {
    for part in [
        "scheme", "host", "port", "origin", "path", "user", "password", "query.a",
    ] {
        let err = read("not a url", part).unwrap_err();
        assert!(
            err.contains("not a URL") && !err.contains("not a url"),
            "{part}: {err}"
        );
    }
}

#[test]
fn host_and_port_write_and_round_trip() {
    let out = run(URL, "url($U, host=replica.internal.test, port=5433)").unwrap();
    assert_eq!(
        out,
        "postgres://app:fakepass@replica.internal.test:5433/orders?sslmode=require"
    );
    let raw = BTreeMap::from([
        ("A".to_string(), Raw::literal(URL)),
        (
            "B".to_string(),
            Raw::computed("url($A, host=replica.internal.test, port=5433)"),
        ),
        ("H".to_string(), Raw::computed("url($B, host)")),
        ("P".to_string(), Raw::computed("url($B, port)")),
    ]);
    let (v, e) = resolve(
        &raw,
        &Values::new(),
        "dev",
        &Values::new(),
        &Policy::default(),
    );
    assert!(e.is_empty(), "{e:?}");
    assert_eq!(
        (v["H"].as_str(), v["P"].as_str()),
        ("replica.internal.test", "5433")
    );
}

#[test]
fn password_writes_from_a_key_and_is_encoded() {
    let out = run(URL, "url($U, password=$PW, query.application_name=a b)").unwrap();
    assert_eq!(
        out,
        "postgres://app:fake%20pw%2F1@db.internal.test:5432/orders?sslmode=require&application_name=a%20b"
    );
    assert_eq!(run(&out, "url($U, password)").as_deref(), Ok("fake pw/1"));
}

#[test]
fn a_literal_password_fails_the_argument_check() {
    use penv_schema::functions::Arg;
    use penv_schema::functions::url::check_password;
    use penv_schema::resolve::Expr;
    let literal = [Arg::Named("password".into(), Expr::Text("x".into()))];
    assert!(
        check_password(&literal)
            .unwrap_err()
            .contains("never a literal")
    );
    let key = [Arg::Named("password".into(), Expr::Ref("PW".into()))];
    assert!(check_password(&key).is_ok());
}

#[test]
fn reading_and_writing_in_one_call_fails() {
    let err = run(URL, "url($U, host, port=1)").unwrap_err();
    assert!(err.contains("never both"), "{err}");
}

fn sensitive_set(
    policy: &Policy,
    exprs: &[(&str, &str)],
    source_sensitive: bool,
) -> BTreeSet<String> {
    let mut raw = BTreeMap::from([("U".to_string(), Raw::literal(URL))]);
    for (k, e) in exprs {
        raw.insert((*k).to_string(), Raw::computed(*e));
    }
    let out = resolve_full(&raw, &Values::new(), "dev", &Values::new(), policy);
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    tainted(&out.deps, |k| source_sensitive && k == "U")
}

#[test]
fn taint_follows_the_public_list() {
    let exprs = [
        ("H", "url($U, host)"),
        ("N", "url($U, user)"),
        ("P", "url($U, password)"),
        ("W", "url($U, host=x.test)"),
    ];
    let hot = sensitive_set(&Policy::default(), &exprs, true);
    assert_eq!(hot, BTreeSet::from(["N".into(), "P".into(), "W".into()]));

    let cold = sensitive_set(&Policy::default(), &exprs, false);
    assert_eq!(cold, BTreeSet::from(["P".to_string()]));

    let mut policy = Policy::default();
    policy.url_public.insert("user".into());
    let open = sensitive_set(&policy, &exprs, true);
    assert_eq!(open, BTreeSet::from(["P".into(), "W".into()]));
}
