//! `penv check` reads the code: variables read and not declared, and declared
//! keys nothing mentions.

use std::process::Command;

fn workspace(files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("penv-usage-{}-{}", std::process::id(), files.len()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, body) in files {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, body).unwrap();
    }
    dir
}

fn check(dir: &std::path::Path, extra: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .current_dir(dir)
        .args(["--format", "text", "check"])
        .args(extra)
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
fn check_names_undeclared_reads_and_unused_keys_and_strict_fails_on_the_first() {
    let dir = workspace(&[
        (
            ".env.schema",
            "# @type=string @sensitive=false\nAPI_KEY=abcdef\n\n# @type=string @sensitive=false\nSTALE=x\n\n# @type=url @sensitive=false\nDATABASE_URL=https://db.example.com\n",
        ),
        (
            "src/app.ts",
            "const k = process.env.API_KEY;\nconst r = process.env[\"REDIS_URL\"];\nif (process.env.NODE_ENV === \"x\") {}\n",
        ),
        (
            "schema.prisma",
            "datasource db {\n  url = env(\"DATABASE_URL\")\n}\n",
        ),
        (
            "src/env.ts",
            "// generated\nexport const STALE = process.env.STALE;\n",
        ),
        (
            ".penv/config.toml",
            "[targets.ts]\noutput = \"src/env.ts\"\n",
        ),
    ]);
    let (code, text) = check(&dir, &[]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("REDIS_URL is read in src/app.ts:2 and not declared"),
        "{text}"
    );
    assert!(
        !text.contains("NODE_ENV is read"),
        "a platform variable is not reported: {text}"
    );
    assert!(
        text.contains("not mentioned in any source file: STALE"),
        "the generated file does not count: {text}"
    );
    assert!(
        !text.contains("DATABASE_URL,") && !text.contains(": DATABASE_URL"),
        "prisma's env() counts: {text}"
    );

    let (code, text) = check(&dir, &["--strict"]);
    assert_eq!(code, 3, "{text}");
    assert!(text.contains("fail REDIS_URL"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}
