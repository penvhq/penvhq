//! A sealed run end to end: the command holds a placeholder, an allowed HTTPS
//! host receives the value in the request head, the body and every other host
//! get the placeholder, and a value echoed back returns as the placeholder.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

const REAL: &str = "sk_test_REAL_0123456789abcdefghijklmnopqr";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("penv-sealed-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Read one request: its head lines and its body.
fn read_request(reader: &mut impl BufRead) -> (Vec<String>, String) {
    let mut head = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end().to_string();
        if line.is_empty() {
            break;
        }
        head.push(line);
    }
    let length = head
        .iter()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).unwrap();
    (head, String::from_utf8_lossy(&body).into_owned())
}

fn answer(stream: &mut impl Write) {
    let body = format!("{{\"echo\":\"{REAL}\"}}");
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    stream.flush().unwrap();
}

type Seen = Arc<Mutex<Vec<(Vec<String>, String)>>>;

/// An HTTPS server for `localhost` with its own authority; returns the port,
/// the authority's PEM and what it received.
fn https_upstream() -> (u16, String, Seen) {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&leaf_key, &Issuer::from_params(&ca_params, &ca_key))
        .unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone(), CertificateDer::from(ca.der().to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
        )
        .unwrap();
    let config = Arc::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    std::thread::spawn(move || {
        for tcp in listener.incoming().flatten() {
            let conn = rustls::ServerConnection::new(config.clone()).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            let mut reader = BufReader::new(&mut tls);
            let got = read_request(&mut reader);
            record.lock().unwrap().push(got);
            answer(&mut tls);
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    });
    (port, ca.pem(), seen)
}

/// A plain HTTP server on 127.0.0.1, a host no key allows.
fn http_upstream() -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    std::thread::spawn(move || {
        for mut tcp in listener.incoming().flatten() {
            let mut reader = BufReader::new(tcp.try_clone().unwrap());
            record.lock().unwrap().push(read_request(&mut reader));
            answer(&mut tcp);
        }
    });
    (port, seen)
}

fn has_curl() -> bool {
    Command::new("curl")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn a_sealed_command_holds_a_placeholder_and_the_allowed_host_gets_the_value_in_the_head_only() {
    if !has_curl() {
        return;
    }
    let (port, ca_pem, seen) = https_upstream();
    let (plain_port, plain_seen) = http_upstream();
    let dir = scratch("curl");
    std::fs::write(
        dir.join(".env.schema"),
        "# @type=string(startsWith=sk_test_, minLength=40) @hosts=localhost\nSTRIPE_SECRET_KEY=\n",
    )
    .unwrap();
    std::fs::write(dir.join(".env"), format!("STRIPE_SECRET_KEY={REAL}\n")).unwrap();
    let trust = dir.join("upstream-ca.pem");
    std::fs::write(&trust, &ca_pem).unwrap();
    let script = format!(
        "echo \"env=$STRIPE_SECRET_KEY\"; \
         curl -s --fail https://localhost:{port}/v1 -H \"Authorization: Bearer $STRIPE_SECRET_KEY\" -d \"k=$STRIPE_SECRET_KEY\"; echo; \
         curl -s http://127.0.0.1:{plain_port}/other -H \"Authorization: Bearer $STRIPE_SECRET_KEY\"; echo"
    );
    let out = Command::new(env!("CARGO_BIN_EXE_penv"))
        .current_dir(&dir)
        .env("SSL_CERT_FILE", &trust)
        .env_remove("PENV_ENV")
        .args(["run", "--sealed", "--", "sh", "-c", &script])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");

    let placeholder = stdout
        .lines()
        .find_map(|l| l.strip_prefix("env="))
        .expect("the command printed its variable")
        .to_string();
    assert!(
        placeholder.starts_with("sk_test_") && placeholder.len() >= 40,
        "{placeholder}"
    );
    assert_ne!(placeholder, REAL);
    assert!(
        !stdout.contains(REAL),
        "the command never sees the value: {stdout}"
    );
    assert!(
        stdout.contains(&format!("{{\"echo\":\"{placeholder}\"}}")),
        "an echoed value comes back as the placeholder: {stdout}"
    );

    let seen = seen.lock().unwrap();
    let (head, body) = &seen[0];
    assert!(
        head.iter()
            .any(|l| l == &format!("Authorization: Bearer {REAL}")),
        "{head:?}"
    );
    assert_eq!(
        body,
        &format!("k={placeholder}"),
        "the body keeps the placeholder"
    );

    let plain = plain_seen.lock().unwrap();
    assert!(
        plain[0]
            .0
            .iter()
            .any(|l| l == &format!("Authorization: Bearer {placeholder}")),
        "a host no key allows gets the placeholder: {:?}",
        plain[0].0
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_key_whose_type_leaves_no_room_is_refused_before_the_command_starts() {
    let dir = scratch("room");
    std::fs::write(
        dir.join(".env.schema"),
        "# @type=string(maxLength=10) @hosts=api.example.com\nK=\n",
    )
    .unwrap();
    std::fs::write(dir.join(".env"), "K=abcdefghij\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_penv"))
        .current_dir(&dir)
        .args(["run", "--sealed", "--", "sh", "-c", "echo started"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("started"));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("cannot_seal")
            || String::from_utf8_lossy(&out.stdout).contains("cannot_seal")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
