//! `penv why` says where a value comes from and never prints it.

use std::process::Command;

const SECRET: &str = "sk_live_WHY_0123456789abcdef";

fn workspace() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("penv-why-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(".env.schema"),
        "# @type=string(startsWith=sk_) @hosts=api.stripe.com\nSTRIPE_SECRET_KEY=\n\n# @type=string @sensitive=false\nDERIVED=${STRIPE_SECRET_KEY}-suffix\n\n# @type=url\nNEXT_PUBLIC_API=https://api.example.com\n",
    )
    .unwrap();
    std::fs::write(
        dir.join(".env"),
        "STRIPE_SECRET_KEY=sk_live_older_value_000000\n",
    )
    .unwrap();
    std::fs::write(
        dir.join(".env.local"),
        format!("STRIPE_SECRET_KEY={SECRET}\nLOCAL_ONLY=\"${{STRIPE_SECRET_KEY}}x\"\n"),
    )
    .unwrap();
    dir
}

fn why(dir: &std::path::Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn why_names_the_winning_file_the_overridden_one_and_what_penv_does_never_the_value() {
    let dir = workspace();
    let (code, text) = why(&dir, &["--format", "text", "why", "STRIPE_SECRET_KEY"]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("from      .env.local"), "{text}");
    assert!(text.contains("overrides .env"), "{text}");
    assert!(text.contains("only to api.stripe.com"), "{text}");
    for args in [
        &["why", "STRIPE_SECRET_KEY"][..],
        &["why", "DERIVED"],
        &["why", "LOCAL_ONLY"],
        &["--format", "text", "why", "DERIVED"],
        &["--format", "text", "why", "LOCAL_ONLY"],
    ] {
        let (_, text) = why(&dir, args);
        assert!(
            !text.contains(SECRET) && !text.contains("older_value") && !text.contains("-suffix"),
            "{args:?}: {text}"
        );
    }
    let (_, text) = why(&dir, &["why", "DERIVED"]);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["builtFrom"], serde_json::json!(["STRIPE_SECRET_KEY"]));
    assert_eq!(json["tainted"], true);
    let (_, text) = why(&dir, &["why", "NEXT_PUBLIC_API"]);
    assert!(text.contains("\"public\": true"), "{text}");
    let (code, _) = why(&dir, &["why", "NOPE"]);
    assert_eq!(code, 3);
    let _ = std::fs::remove_dir_all(&dir);
}
