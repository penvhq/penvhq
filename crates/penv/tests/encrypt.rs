//! Values encrypted at rest: on by default, off with `[local] encrypt = false`.

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

#[test]
fn set_encrypts_a_sensitive_value_and_run_hands_the_command_the_value_not_the_key() {
    let d = Dir::new("set", &[(".env.schema", SCHEMA)]);
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
fn encrypt_false_in_config_writes_plain_text() {
    let d = Dir::new(
        "off",
        &[
            (".env.schema", SCHEMA),
            (".penv/config.toml", "[local]\nencrypt = false\n"),
        ],
    );
    let set = d.penv(
        KEY,
        &["set", "STRIPE_SECRET_KEY"],
        Some(&format!("{SECRET}\n")),
    );
    assert_eq!(set.status.code(), Some(0), "{}", text(&set));
    assert_eq!(d.read(".env").trim(), format!("STRIPE_SECRET_KEY={SECRET}"));
}

#[test]
fn a_value_encrypted_with_another_key_stops_the_run_and_names_the_key_not_the_value() {
    let d = Dir::new("other", &[(".env.schema", SCHEMA)]);
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
fn encrypt_and_decrypt_convert_the_files_sensitive_keys_only_and_an_agent_cannot_decrypt() {
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
        format!("# keep me\nSTRIPE_SECRET_KEY={SECRET}\nPORT=3000\n")
    );
    assert_eq!(d.read(".env.local"), "UNDECLARED=value_1234\n");
}

#[test]
fn init_writes_every_setting_with_its_meaning_and_keeps_what_a_config_already_says() {
    let fresh = Dir::new("init", &[(".env", "PORT=3000\n")]);
    let init = fresh.penv(KEY, &["init"], None);
    assert_eq!(init.status.code(), Some(0), "{}", text(&init));
    let config = fresh.read(".penv/config.toml");
    for line in [
        "preload = true",
        "encrypt = true",
        "# false: penv writes values in plain text",
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
        config.contains("encrypt = true"),
        "a missing setting is added: {config}"
    );
    assert_eq!(config.matches("preload =").count(), 1, "{config}");
}
