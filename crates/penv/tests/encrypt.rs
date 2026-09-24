//! Values encrypted at rest: off by default, on with `[local] encrypt = true`
//! or `penv encrypt`.

use std::process::{Command, Output, Stdio};

const KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const OTHER: &str = "00000000000000000000000000000000000000000000000000000000000000ff";
const SECRET: &str = "sk_live_ENCRYPT_0123456789abcdef";

struct Dir(std::path::PathBuf);

impl Dir {
    fn new(name: &str, files: &[(&str, &str)]) -> Dir {
        let dir = std::env::temp_dir().join(format!("penv-encrypt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (path, body) in files {
            let full = dir.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, body).unwrap();
        }
        Dir(dir)
    }
    fn penv(&self, key: &str, args: &[&str], stdin: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
        command
            .current_dir(&self.0)
            .env("PENV_LOCAL_KEY", key)
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        if let Some(text) = stdin {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        child.wait_with_output().unwrap()
    }
    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.0.join(path)).unwrap_or_default()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

const SCHEMA: &str =
    "# @type=string\nSTRIPE_SECRET_KEY=\n\n# @type=port @sensitive=false\nPORT=3000\n";
const ON: &str = "[local]\nencrypt = true\n";

#[test]
fn with_encrypt_on_set_encrypts_and_run_hands_the_command_the_value_not_the_key() {
    let d = Dir::new("set", &[(".env.schema", SCHEMA), (".penv/config.toml", ON)]);
    let set = d.penv(
        KEY,
        &["set", "STRIPE_SECRET_KEY"],
        Some(&format!("{SECRET}\n")),
    );
    assert_eq!(set.status.code(), Some(0), "{}", text(&set));
    let file = d.read(".env");
    assert!(file.contains("STRIPE_SECRET_KEY=enc:v1:"), "{file}");
    assert!(!file.contains(SECRET));

    let run = d.penv(
        KEY,
        &[
            "run",
            "--",
            "sh",
            "-c",
            "printf '%s|%s' \"${#STRIPE_SECRET_KEY}\" \"${PENV_LOCAL_KEY:-none}\"",
        ],
        None,
    );
    let out = String::from_utf8_lossy(&run.stdout).to_string();
    assert_eq!(out, format!("{}|none", SECRET.len()), "{}", text(&run));
}

#[test]
fn with_no_setting_or_encrypt_false_set_writes_plain_text() {
    for config in [None, Some("[local]\nencrypt = false\n")] {
        let mut files = vec![(".env.schema", SCHEMA)];
        if let Some(c) = config {
            files.push((".penv/config.toml", c));
        }
        let d = Dir::new(if config.is_some() { "off" } else { "default" }, &files);
        let set = d.penv(
            KEY,
            &["set", "STRIPE_SECRET_KEY"],
            Some(&format!("{SECRET}\n")),
        );
        assert_eq!(set.status.code(), Some(0), "{}", text(&set));
        assert_eq!(
            d.read(".env").trim(),
            format!("STRIPE_SECRET_KEY={SECRET}"),
            "{config:?}"
        );
    }
}

#[test]
fn a_value_encrypted_with_another_key_stops_the_run_and_names_the_key_not_the_value() {
    let d = Dir::new(
        "other",
        &[(".env.schema", SCHEMA), (".penv/config.toml", ON)],
    );
    d.penv(
        OTHER,
        &["set", "STRIPE_SECRET_KEY"],
        Some(&format!("{SECRET}\n")),
    );
    let run = d.penv(KEY, &["run", "--", "sh", "-c", "echo started"], None);
    assert_ne!(run.status.code(), Some(0));
    let t = text(&run);
    assert!(
        t.contains("decrypt_failed") && t.contains("STRIPE_SECRET_KEY"),
        "{t}"
    );
    assert!(!t.contains("started") && !t.contains(SECRET), "{t}");
}

#[test]
fn encrypt_and_decrypt_convert_the_files_switch_the_setting_and_an_agent_cannot_decrypt() {
    let d = Dir::new(
        "convert",
        &[
            (".env.schema", SCHEMA),
            (
                ".env",
                &format!("# keep me\nSTRIPE_SECRET_KEY={SECRET}\nPORT=3000\n"),
            ),
            (".env.local", "UNDECLARED=value_1234\n"),
        ],
    );
    let enc = d.penv(KEY, &["--format", "text", "encrypt"], None);
    assert_eq!(enc.status.code(), Some(0), "{}", text(&enc));
    let file = d.read(".env");
    assert!(
        file.starts_with("# keep me\nSTRIPE_SECRET_KEY=enc:v1:"),
        "{file}"
    );
    assert!(file.ends_with("\nPORT=3000\n"), "{file}");
    assert!(
        d.read(".env.local").starts_with("UNDECLARED=enc:v1:"),
        "an undeclared key counts as sensitive"
    );
    assert!(
        d.read(".penv/config.toml").contains("encrypt = true"),
        "encrypt turns the setting on"
    );
    let later = d.penv(KEY, &["set", "LATER_SECRET"], Some("later_value_1234\n"));
    assert_eq!(later.status.code(), Some(0), "{}", text(&later));
    assert!(
        d.read(".env").contains("LATER_SECRET=enc:v1:"),
        "and set follows it"
    );

    let refused = d.penv(KEY, &["--agent", "decrypt"], None);
    assert_ne!(refused.status.code(), Some(0));
    assert!(
        text(&refused).contains("agent_session"),
        "{}",
        text(&refused)
    );
    assert!(d.read(".env").contains("enc:v1:"), "nothing was written");

    let dec = d.penv(KEY, &["--format", "text", "decrypt"], None);
    assert_eq!(dec.status.code(), Some(0), "{}", text(&dec));
    assert_eq!(
        d.read(".env"),
        format!(
            "# keep me\nSTRIPE_SECRET_KEY={SECRET}\nPORT=3000\nLATER_SECRET=later_value_1234\n"
        )
    );
    assert_eq!(d.read(".env.local"), "UNDECLARED=value_1234\n");
    assert!(
        d.read(".penv/config.toml").contains("encrypt = false"),
        "decrypt turns it off"
    );
}

#[test]
fn encrypt_leaves_every_computed_value_as_written() {
    let d = Dir::new(
        "computed",
        &[
            (".env.schema", SCHEMA),
            (
                ".env",
                &format!(
                    "STRIPE_SECRET_KEY={SECRET}\nREPLICA=penv(production/DATABASE_URL)\nSESSION=random(32)\nMODE=if(eq($PORT, 3000), dev, prod)\nJOINED=concat(a, b)\nREF=${{STRIPE_SECRET_KEY}}\n"
                ),
            ),
        ],
    );
    let enc = d.penv(KEY, &["--format", "text", "encrypt"], None);
    assert_eq!(enc.status.code(), Some(0), "{}", text(&enc));
    let file = d.read(".env");
    assert!(file.starts_with("STRIPE_SECRET_KEY=enc:v1:"), "{file}");
    for line in [
        "REPLICA=penv(production/DATABASE_URL)",
        "SESSION=random(32)",
        "MODE=if(eq($PORT, 3000), dev, prod)",
        "JOINED=concat(a, b)",
        "REF=${STRIPE_SECRET_KEY}",
    ] {
        assert!(file.contains(line), "{line} was encrypted: {file}");
    }
}

#[test]
fn init_writes_every_setting_with_its_meaning_and_keeps_what_a_config_already_says() {
    let fresh = Dir::new("init", &[(".env", "PORT=3000\n")]);
    let init = fresh.penv(KEY, &["init"], None);
    assert_eq!(init.status.code(), Some(0), "{}", text(&init));
    let config = fresh.read(".penv/config.toml");
    for line in [
        "preload = true",
        "encrypt = false",
        "# true: they write enc:v1:",
        "# false: penv set, penv pull and random() write values in plain text",
        "prefixes = []",
        "version = 1",
    ] {
        assert!(config.contains(line), "{line} missing from:\n{config}");
    }

    let existing = Dir::new(
        "init-existing",
        &[
            (".env", "PORT=3000\n"),
            (
                ".penv/config.toml",
                "# mine\n[run]\npreload = false # on purpose\n",
            ),
        ],
    );
    existing.penv(KEY, &["init"], None);
    let config = existing.read(".penv/config.toml");
    assert!(
        config.contains("# mine") && config.contains("preload = false # on purpose"),
        "{config}"
    );
    assert!(
        config.contains("encrypt = false"),
        "a missing setting is added: {config}"
    );
    assert_eq!(config.matches("preload =").count(), 1, "{config}");
}
