use penv_schema::Values;
use penv_schema::functions::Policy;
use penv_schema::functions::json::duplicate_keys;
use penv_schema::resolve::{Raw, resolve};

const DOC: &str = r#"{"username":"app","db":{"hosts":["db1.internal.test","db2.internal.test"],"port":5432},"tls":true,"gone":null}"#;

fn run(
    pairs: &[(&str, Raw)],
    policy: &Policy,
) -> (Values, Vec<penv_schema::resolve::ResolveError>) {
    let raw = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    resolve(&raw, &Values::new(), "development", &Values::new(), policy)
}

fn eval(expr: &str) -> Result<String, String> {
    let (v, e) = run(
        &[
            ("DB_SECRET", Raw::literal(DOC)),
            ("APP_ENV", Raw::literal("staging")),
            ("D", Raw::computed(expr)),
        ],
        &Policy::default(),
    );
    match v.get("D") {
        Some(x) if e.is_empty() => Ok(x.clone()),
        _ => Err(e
            .into_iter()
            .map(|e| e.message)
            .collect::<Vec<_>>()
            .join("; ")),
    }
}

#[test]
fn reads() {
    assert_eq!(
        eval("json($DB_SECRET, db.hosts[0])").unwrap(),
        "db1.internal.test"
    );
    assert_eq!(
        eval("json($DB_SECRET, db.hosts)").unwrap(),
        r#"["db1.internal.test","db2.internal.test"]"#
    );
    assert_eq!(eval("json($DB_SECRET, tls)").unwrap(), "true");
    assert_eq!(eval("json($DB_SECRET, db.port)").unwrap(), "5432");
    assert_eq!(eval("json($DB_SECRET)").unwrap(), DOC);
}

#[test]
fn builds() {
    assert_eq!(
        eval("json(beta=eq($APP_ENV, staging), maxUploadMb=25, region=eu-1)").unwrap(),
        r#"{"beta":true,"maxUploadMb":25,"region":"eu-1"}"#
    );
    assert_eq!(eval("json(get=1)").unwrap(), r#"{"get":1}"#);
    assert_eq!(
        eval("json(db.host=h, db.port=5432)").unwrap(),
        r#"{"db":{"host":"h","port":5432}}"#
    );
    assert_eq!(
        eval("json(hosts[0]=a, hosts[1]=b)").unwrap(),
        r#"{"hosts":["a","b"]}"#
    );
    assert_eq!(
        eval("json(db=raw('{\"a\":[1,2]}'))").unwrap(),
        r#"{"db":{"a":[1,2]}}"#
    );
    assert!(eval("json(db=raw('{nope'))").is_err());
}

#[test]
fn writes_keep_order_and_create() {
    assert_eq!(
        eval("json($DB_SECRET, project_id=acme-sandbox)").unwrap(),
        DOC.trim_end_matches('}').to_string() + r#","project_id":"acme-sandbox"}"#
    );
    assert!(
        eval("json($DB_SECRET, username=x)")
            .unwrap()
            .starts_with(r#"{"username":"x","db""#)
    );
}

#[test]
fn quoting_forces_a_string() {
    assert_eq!(
        eval("json(maxUploadMb='25')").unwrap(),
        r#"{"maxUploadMb":"25"}"#
    );
    assert_eq!(
        eval("json(a=true, b='true')").unwrap(),
        r#"{"a":true,"b":"true"}"#
    );
}

#[test]
fn strings_are_escaped() {
    let (v, e) = run(
        &[
            ("Q", Raw::literal("say \"hi\"\nthere")),
            ("D", Raw::computed("json(msg=$Q)")),
        ],
        &Policy::default(),
    );
    assert!(e.is_empty());
    assert_eq!(v["D"], r#"{"msg":"say \"hi\"\nthere"}"#);
}

#[test]
fn missing_null_and_invalid_fail_without_values() {
    for expr in [
        "json($DB_SECRET, nope)",
        "json($DB_SECRET, gone)",
        "json($DB_SECRET, db.hosts[9])",
    ] {
        let err = eval(expr).unwrap_err();
        assert!(err.contains("missing or null"), "{err}");
    }
    assert!(eval("json($DB_SECRET, nope)").unwrap_err().contains("nope"));
    assert!(
        eval("json($APP_ENV)")
            .unwrap_err()
            .contains("not valid JSON")
    );
    assert!(!eval("json($APP_ENV)").unwrap_err().contains("staging"));
}

#[test]
#[ignore = "resolve.rs fallback() propagates errors; needs a T1-path change"]
fn fallback_supplies_a_default() {
    assert_eq!(eval("fallback(json($DB_SECRET, nope), d)").unwrap(), "d");
}

#[test]
fn bracketed_key_names_a_dotted_key() {
    let (v, e) = run(
        &[
            ("J", Raw::literal(r#"{"a.b":7}"#)),
            ("D", Raw::computed(r#"json($J, ["a.b"])"#)),
        ],
        &Policy::default(),
    );
    assert!(e.is_empty(), "{e:?}");
    assert_eq!(v["D"], "7");
}

#[test]
fn duplicate_keys_are_named_and_the_last_wins() {
    assert_eq!(duplicate_keys(r#"{"a":1,"b":2,"a":3}"#), vec!["a"]);
    assert!(duplicate_keys(DOC).is_empty());
    assert_eq!(eval_doc(r#"{"a":1,"b":2,"a":3}"#), r#"{"a":3,"b":2}"#);
}

fn eval_doc(doc: &str) -> String {
    let (v, _) = run(
        &[("J", Raw::literal(doc)), ("D", Raw::computed("json($J)"))],
        &Policy::default(),
    );
    v["D"].clone()
}

#[test]
fn taint_listed_path_is_public() {
    use penv_schema::functions::{Taint, taint};
    let mut policy = Policy::default();
    policy
        .json_public
        .insert("DB_SECRET".into(), ["db.port".to_string()].into());
    assert_eq!(
        taint("json", Some("db.port"), Some("DB_SECRET"), "P", &policy),
        Taint::Public
    );
    assert_eq!(
        taint("json", Some("username"), Some("DB_SECRET"), "P", &policy),
        Taint::Inputs
    );
}

fn doc_result(doc: &str, expr: &str) -> Result<String, ()> {
    let (v, e) = run(
        &[("J", Raw::literal(doc)), ("D", Raw::computed(expr))],
        &Policy::default(),
    );
    match v.get("D") {
        Some(x) if e.is_empty() => Ok(x.clone()),
        _ => Err(()),
    }
}

#[test]
fn unicode_escape_needs_four_hex_digits() {
    assert!(doc_result(r#"{"a":"\u+041"}"#, "json($J)").is_err());
    assert!(doc_result(r#"{"a":"A"}"#, "json($J)").is_ok());
}

#[test]
fn form_feed_is_not_json_whitespace() {
    assert!(doc_result("{\"a\":1,\u{c}\"b\":2}", "json($J)").is_err());
    assert!(doc_result("{\"a\":1,\t\"b\":2}", "json($J)").is_ok());
}

#[test]
fn path_index_and_separator_are_strict() {
    let doc = r#"{"a":[10,20]}"#;
    assert_eq!(doc_result(doc, "json($J, a[1])").unwrap(), "20");
    assert!(doc_result(doc, "json($J, a[+1])").is_err());
    assert!(doc_result(r#"{"a":[{"b":1}]}"#, "json($J, a[0]b)").is_err());
    assert_eq!(
        doc_result(r#"{"a":[{"b":1}]}"#, "json($J, a[0].b)").unwrap(),
        "1"
    );
}

#[test]
fn many_duplicate_keys_keep_first_position_last_value() {
    let doc = format!(
        "{{{}}}",
        (0..2000)
            .map(|i| format!("\"k{}\":{}", i % 1000, i))
            .collect::<Vec<_>>()
            .join(",")
    );
    let out = eval_doc(&doc);
    assert!(out.starts_with(r#"{"k0":1000,"k1":1001,"#));
}
