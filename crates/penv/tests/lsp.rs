//! `penv lsp` over real pipes, the way an editor drives it.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

const SECRET: &str = "sk_live_LSP_0123456789abcdef";

struct Client {
    child: Child,
    stdin: ChildStdin,
    from_server: Receiver<Value>,
    next_id: i64,
}

impl Client {
    fn start(dir: &Path) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_penv"))
            .arg("lsp")
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(n) = line.strip_prefix("Content-Length: ") {
                        length = n.parse().unwrap();
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    return;
                }
            }
        });
        Client {
            child,
            stdin,
            from_server: rx,
            next_id: 1,
        }
    }

    fn send_raw(&mut self, bytes: &[u8]) {
        self.stdin.write_all(bytes).unwrap();
        self.stdin.flush().unwrap();
    }

    fn send(&mut self, message: Value) {
        let body = serde_json::to_vec(&message).unwrap();
        self.send_raw(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        self.send_raw(&body);
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let message = self.recv();
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    fn recv(&self) -> Value {
        self.from_server
            .recv_timeout(Duration::from_secs(20))
            .expect("the server answered in time")
    }

    fn initialize(&mut self) -> Value {
        let answer = self.request(
            "initialize",
            json!({ "processId": null, "rootUri": null, "capabilities": {} }),
        );
        self.notify("initialized", json!({}));
        answer
    }

    fn open(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": "env-spec", "version": 1, "text": text } }),
        );
    }

    fn exit_code(mut self) -> i32 {
        self.notify("exit", Value::Null);
        drop(self.stdin);
        self.child.wait().unwrap().code().unwrap_or(-1)
    }
}

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("penv-lsp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".penv")).unwrap();
    dir
}

fn uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{}", text.replace(' ', "%20"))
    } else {
        format!("file:///{}", text.replace(' ', "%20"))
    }
}

fn at(uri: &str, line: u32, character: u32) -> Value {
    json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } })
}

const SCHEMA: &str = "\
# @defaultSensitive=true
# ---

# @type=port @sensitive=false
PORT=8080

# @type=string(startsWith=sk_) @hosts(api.stripe.com) @rotate=90d
STRIPE_SECRET_KEY=
";

#[test]
fn an_editor_session_from_initialize_to_exit() {
    let dir = workspace("session");
    let schema = dir.join(".env.schema");
    std::fs::write(&schema, SCHEMA).unwrap();
    let doc = uri(&schema);
    let mut client = Client::start(&dir);

    let init = client.initialize();
    let caps = &init["result"]["capabilities"];
    assert_eq!(caps["textDocumentSync"]["change"], 1);
    assert_eq!(caps["hoverProvider"], true);
    assert_eq!(init["result"]["serverInfo"]["name"], "penv");

    client.open(
        &doc,
        &SCHEMA.replace("@type=port", "@type=port @rotate=soon"),
    );
    let published = client.recv();
    assert_eq!(published["method"], "textDocument/publishDiagnostics");
    let found = published["params"]["diagnostics"].as_array().unwrap();
    assert_eq!(found.len(), 1, "{published}");
    assert_eq!(found[0]["severity"], 1);
    assert_eq!(
        found[0]["range"]["start"],
        json!({ "line": 3, "character": 13 })
    );
    assert!(
        found[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("@rotate takes")
    );

    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": doc, "version": 2 }, "contentChanges": [{ "text": SCHEMA }] }),
    );
    let published = client.recv();
    let found = published["params"]["diagnostics"].as_array().unwrap();
    assert!(
        found.is_empty(),
        "fixed text clears the problem: {published}"
    );

    let items = client.request("textDocument/completion", at(&doc, 6, 3));
    let labels: Vec<&str> = items["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["label"].as_str())
        .collect();
    assert!(
        labels.contains(&"@hosts") && labels.contains(&"@rotate"),
        "{labels:?}"
    );

    let tip = client.request("textDocument/hover", at(&doc, 7, 3));
    let md = tip["result"]["contents"]["value"].as_str().unwrap();
    assert!(md.contains("sealed to `api.stripe.com`"), "{md}");
    assert!(
        md.contains("no recorded write"),
        "local-mode rotation note: {md}"
    );

    let outline = client.request("textDocument/documentSymbol", at(&doc, 0, 0));
    assert_eq!(outline["result"].as_array().unwrap().len(), 2);

    let unknown = client.request("textDocument/formatting", at(&doc, 0, 0));
    assert_eq!(unknown["error"]["code"], -32601);

    client.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": doc } }),
    );
    let cleared = client.recv();
    assert_eq!(cleared["params"]["diagnostics"], json!([]));

    assert_eq!(
        client.request("shutdown", Value::Null)["result"],
        Value::Null
    );
    assert_eq!(client.exit_code(), 0);
}

#[test]
fn an_overdue_rotation_is_a_warning_read_from_the_committed_config() {
    let dir = workspace("rotate");
    let schema = dir.join(".env.schema");
    std::fs::write(&schema, SCHEMA).unwrap();
    std::fs::write(
        dir.join(".penv/config.toml"),
        "[rotation]\nSTRIPE_SECRET_KEY = \"2020-01-01\"\n",
    )
    .unwrap();
    let doc = uri(&schema);
    let mut client = Client::start(&dir);
    client.initialize();
    client.open(&doc, SCHEMA);
    let published = client.recv();
    let found = published["params"]["diagnostics"].as_array().unwrap();
    assert_eq!(found.len(), 1, "{published}");
    assert_eq!(found[0]["severity"], 2);
    assert_eq!(found[0]["code"], "rotate_overdue");
    assert_eq!(
        found[0]["message"],
        "rotate STRIPE_SECRET_KEY was due 2020-03-31; rotate it, then penv set STRIPE_SECRET_KEY"
    );
    assert_eq!(
        found[0]["range"]["start"],
        json!({ "line": 6, "character": 54 })
    );
    client.request("shutdown", Value::Null);
    assert_eq!(client.exit_code(), 0);
}

#[test]
fn value_files_are_never_read_or_answered() {
    let dir = workspace("values");
    std::fs::write(dir.join(".env.schema"), SCHEMA).unwrap();
    let env_file = dir.join(".env");
    std::fs::write(&env_file, format!("STRIPE_SECRET_KEY={SECRET}\n")).unwrap();
    let doc = uri(&env_file);
    let mut client = Client::start(&dir);
    client.initialize();
    client.open(&doc, &format!("STRIPE_SECRET_KEY={SECRET}\n"));
    // No diagnostics are published for it: the next message is this answer.
    let tip = client.request("textDocument/hover", at(&doc, 0, 3));
    assert_eq!(tip["result"], Value::Null);
    let items = client.request("textDocument/completion", at(&doc, 0, 20));
    assert_eq!(items["result"]["items"], json!([]));
    assert!(!items.to_string().contains(SECRET));
    client.request("shutdown", Value::Null);
    assert_eq!(client.exit_code(), 0);
}

#[test]
fn the_protocols_order_and_framing_rules_hold() {
    let dir = workspace("order");
    let mut client = Client::start(&dir);
    let early = client.request("textDocument/hover", json!({}));
    assert_eq!(early["error"]["code"], -32002);

    // A frame that is not JSON is answered with a parse error, and the stream recovers.
    client.send_raw(b"Content-Length: 5\r\n\r\nnope!");
    assert_eq!(client.recv()["error"]["code"], -32700);
    client.initialize();
    client.request("shutdown", Value::Null);
    let after = client.request("textDocument/hover", json!({}));
    assert_eq!(after["error"]["code"], -32600);
    assert_eq!(client.exit_code(), 0);
}

#[test]
fn exit_without_shutdown_and_a_closed_pipe_both_exit_one() {
    let dir = workspace("exit");
    let mut client = Client::start(&dir);
    client.initialize();
    assert_eq!(client.exit_code(), 1);

    let mut client = Client::start(&dir);
    client.initialize();
    drop(client.stdin);
    assert_eq!(client.child.wait().unwrap().code(), Some(1));
}

#[test]
fn the_editor_reports_what_check_reports_beyond_the_parser() {
    let dir = workspace("layer");
    let schema = dir.join(".env.schema");
    let doc = uri(&schema);
    let missing = "# @import(../nowhere/.env.schema)\n# ---\n\nKEY=x\n";
    std::fs::write(&schema, missing).unwrap();
    let mut client = Client::start(&dir);
    client.initialize();

    client.open(&doc, missing);
    let found = client.recv()["params"]["diagnostics"].clone();
    assert_eq!(found.as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found[0]["code"], "invalid_import");
    assert_eq!(found[0]["range"]["start"]["line"], 0);

    // A prefix .penv/config.toml marks public cannot also be @sensitive.
    std::fs::write(
        dir.join(".penv/config.toml"),
        "[schema]\nversion = 1\n\n[public]\nprefixes = [\"SHARED_\"]\n",
    )
    .unwrap();
    let text = "# @defaultRequired=false\n# ---\n\n# @sensitive\nSHARED_KEY=\n";
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": doc, "version": 2 }, "contentChanges": [{ "text": text }] }),
    );
    let found = client.recv()["params"]["diagnostics"].clone();
    let codes: Vec<&str> = found
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["code"].as_str())
        .collect();
    assert_eq!(codes, ["sensitive_public_key"], "{found}");

    // A config file that is not TOML is named as the file at fault.
    std::fs::write(dir.join(".penv/config.toml"), "[schema\n").unwrap();
    client.notify(
        "textDocument/didSave",
        json!({ "textDocument": { "uri": doc } }),
    );
    let found = client.recv()["params"]["diagnostics"].clone();
    assert_eq!(found[0]["code"], "invalid_config", "{found}");
    assert!(
        found[0]["message"]
            .as_str()
            .unwrap()
            .contains("config.toml"),
        "{found}"
    );

    client.request("shutdown", Value::Null);
    assert_eq!(client.exit_code(), 0);
}
