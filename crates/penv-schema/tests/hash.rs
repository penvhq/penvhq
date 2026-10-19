use std::collections::BTreeSet;

use penv_schema::functions::{Call, Policy, Taint, hash, taint};

fn call<'a>(subject: &str, options: &[(&'a str, &str)]) -> Call<'a> {
    Call {
        name: "hash",
        subject: Some(subject.into()),
        address: None,
        options: options.iter().map(|(k, v)| (*k, v.to_string())).collect(),
        raw: BTreeSet::new(),
        quoted: BTreeSet::new(),
    }
}

fn run(subject: &str, options: &[(&str, &str)]) -> Result<String, String> {
    hash::eval(&call(subject, options))
}

const RFC_KEY: &str = "\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}\u{b}";

#[test]
fn fingerprint_of_the_fake_value() {
    let out = run("fake_api_key_123", &[("algo", "sha256"), ("length", "8")]);
    assert_eq!(out.unwrap(), "604d6582");
}

#[test]
fn sha256_of_abc_in_each_encoding() {
    let hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    assert_eq!(run("abc", &[("algo", "sha256")]).unwrap(), hex);
    let b64 = run("abc", &[("algo", "sha256"), ("encoding", "base64")]);
    assert_eq!(b64.unwrap(), "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=");
    let url = run("abc", &[("algo", "sha256"), ("encoding", "base64url")]);
    assert_eq!(url.unwrap(), "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0");
}

#[test]
fn sha512_of_abc_pads_base64_and_not_base64url() {
    let b64 = run("abc", &[("algo", "sha512"), ("encoding", "base64")]).unwrap();
    assert_eq!(
        b64,
        "3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw=="
    );
    let url = run("abc", &[("algo", "sha512"), ("encoding", "base64url")]).unwrap();
    assert_eq!(
        url,
        b64.trim_end_matches('=')
            .replace('+', "-")
            .replace('/', "_")
    );
}

#[test]
fn rfc4231_case_1() {
    let sha256 = run("Hi There", &[("algo", "hmac-sha256"), ("key", RFC_KEY)]);
    assert_eq!(
        sha256.unwrap(),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    let sha512 = run("Hi There", &[("algo", "hmac-sha512"), ("key", RFC_KEY)]);
    assert_eq!(
        sha512.unwrap(),
        "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
    );
}

#[test]
fn hmac_needs_a_key_and_others_refuse_one() {
    let err = run("x", &[("algo", "hmac-sha256")]).unwrap_err();
    assert!(err.contains("needs key="), "{err}");
    let err = run("x", &[("algo", "sha256"), ("key", "k")]).unwrap_err();
    assert!(err.contains("hmac-* only"), "{err}");
}

#[test]
fn unknown_algorithms_and_encodings_fail() {
    for algo in ["md5", "sha1"] {
        assert!(run("x", &[("algo", algo)]).is_err(), "{algo}");
    }
    assert!(run("x", &[("algo", "sha256"), ("encoding", "rot13")]).is_err());
    assert!(run("x", &[]).is_err());
}

#[test]
fn length_bounds() {
    assert!(run("x", &[("algo", "sha256"), ("length", "0")]).is_err());
    assert!(run("x", &[("algo", "sha256"), ("length", "abc")]).is_err());
    assert!(run("x", &[("algo", "sha256"), ("length", "+8")]).is_err());
    let all = run("x", &[("algo", "sha256")]).unwrap();
    assert_eq!(
        run("x", &[("algo", "sha256"), ("length", "999")]).unwrap(),
        all
    );
}

#[test]
fn taint_follows_inputs_unless_listed() {
    let mut policy = Policy::default();
    assert_eq!(taint("hash", None, Some("K"), "FP", &policy), Taint::Inputs);
    policy.hash_public.insert("FP".into());
    assert_eq!(taint("hash", None, Some("K"), "FP", &policy), Taint::Public);
}
