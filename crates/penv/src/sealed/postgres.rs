//! A sealed Postgres URL. The command connects to a loopback port with the
//! placeholder as its password; penv checks the placeholder, opens the real
//! connection (TLS as the URL's sslmode says) with the real password, and then
//! passes bytes both ways unread.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rustls::ClientConfig;

use super::scram::{Scram, md5_password};
use super::upstream::{self, Upstream};

const SSL_REQUEST: i32 = 80877103;
const GSS_REQUEST: i32 = 80877104;
const CANCEL_REQUEST: i32 = 80877102;
const PROTOCOL_3: i32 = 196608;

/// Where the real database is and how to reach it.
#[derive(Clone)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub password: String,
    pub placeholder: String,
    pub tls: Tls,
}

#[derive(Clone)]
pub enum Tls {
    /// `sslmode=disable`.
    Off,
    /// `allow` or `prefer` (libpq's default): TLS when the server has it.
    Prefer(Arc<ClientConfig>),
    /// `require`: TLS or nothing, the certificate unchecked, as libpq does.
    Require(Arc<ClientConfig>),
    /// `verify-ca` and `verify-full`: TLS with the certificate checked.
    Verify(Arc<ClientConfig>),
}

pub fn start(target: Target) -> io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let target = Arc::new(target);
    thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let target = target.clone();
            thread::spawn(move || {
                let _ = serve(client, &target);
            });
        }
    });
    Ok(port)
}

fn read_i32(r: &mut impl Read) -> io::Result<i32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_be_bytes(b))
}

/// One typed message: its type byte and payload.
fn read_message(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut kind = [0u8; 1];
    r.read_exact(&mut kind)?;
    let len = read_i32(r)?;
    if !(4..=1 << 24).contains(&len) {
        return Err(bad("a message length out of range"));
    }
    let mut payload = vec![0u8; len as usize - 4];
    r.read_exact(&mut payload)?;
    Ok((kind[0], payload))
}

fn message(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    out.extend_from_slice(&((payload.len() + 4) as i32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn error_response(code: &str, text: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    for (field, value) in [(b'S', "FATAL"), (b'V', "FATAL"), (b'C', code), (b'M', text)] {
        payload.push(field);
        payload.extend_from_slice(value.as_bytes());
        payload.push(0);
    }
    payload.push(0);
    message(b'E', &payload)
}

fn bad(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("the sealed Postgres proxy refused {what}"),
    )
}

/// Equal in time whatever the bytes, so the check says nothing about how close a guess was.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn serve(mut client: TcpStream, target: &Target) -> io::Result<()> {
    client.set_read_timeout(Some(Duration::from_secs(60)))?;
    // Startup: refuse TLS and GSS on the loopback leg, then read the parameters.
    let params = loop {
        let len = read_i32(&mut client)?;
        if !(8..=10_000).contains(&len) {
            return Err(bad("a startup length out of range"));
        }
        let code = read_i32(&mut client)?;
        let mut rest = vec![0u8; len as usize - 8];
        client.read_exact(&mut rest)?;
        match code {
            SSL_REQUEST | GSS_REQUEST => client.write_all(b"N")?,
            CANCEL_REQUEST => return Ok(()),
            PROTOCOL_3 => break rest,
            _ => return Err(bad("a protocol version other than 3")),
        }
    };
    let user = parameter(&params, "user").unwrap_or_default();

    // The command proves it holds the placeholder.
    client.write_all(&message(b'R', &3i32.to_be_bytes()))?;
    let (kind, payload) = read_message(&mut client)?;
    let given = payload.strip_suffix(&[0]).unwrap_or(&payload);
    if kind != b'p' || !same(given, target.placeholder.as_bytes()) {
        client.write_all(&error_response(
            "28P01",
            "password authentication failed (penv sealed proxy)",
        ))?;
        return Ok(());
    }

    let mut up = match connect(target) {
        Ok(up) => up,
        Err(e) => {
            client.write_all(&error_response(
                "08001",
                &format!("penv could not reach the database: {e}"),
            ))?;
            return Ok(());
        }
    };
    let mut startup = Vec::new();
    startup.extend_from_slice(&((params.len() + 8) as i32).to_be_bytes());
    startup.extend_from_slice(&PROTOCOL_3.to_be_bytes());
    startup.extend_from_slice(&params);
    up.write_all(&startup)?;
    up.flush()?;

    let mut scram: Option<Scram> = None;
    loop {
        let (kind, payload) = read_message(&mut up)?;
        match kind {
            b'E' => {
                client.write_all(&message(b'E', &payload))?;
                return Ok(());
            }
            b'R' if payload.len() >= 4 => {
                let code = i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                let body = &payload[4..];
                match code {
                    0 => {
                        client.write_all(&message(b'R', &payload))?;
                        break;
                    }
                    3 => send_password(&mut up, target.password.as_bytes())?,
                    5 if body.len() >= 4 => {
                        let salt = [body[0], body[1], body[2], body[3]];
                        send_password(
                            &mut up,
                            md5_password(&target.password, &user, &salt).as_bytes(),
                        )?;
                    }
                    10 => {
                        let mechanisms: Vec<&[u8]> = body.split(|b| *b == 0).collect();
                        if !mechanisms.contains(&&b"SCRAM-SHA-256"[..]) {
                            client.write_all(&error_response(
                                "28000",
                                "the database offered no SASL mechanism penv speaks",
                            ))?;
                            return Ok(());
                        }
                        let nonce = penv_cloud::b64::encode(
                            &penv_cloud::random_bytes(18).map_err(io::Error::other)?,
                        );
                        let s = Scram::new(&target.password, &nonce);
                        let first = s.first();
                        let mut out = b"SCRAM-SHA-256\0".to_vec();
                        out.extend_from_slice(&(first.len() as i32).to_be_bytes());
                        out.extend_from_slice(first.as_bytes());
                        up.write_all(&message(b'p', &out))?;
                        up.flush()?;
                        scram = Some(s);
                    }
                    11 => {
                        let s = scram
                            .as_mut()
                            .ok_or_else(|| bad("SASL continue without a start"))?;
                        let reply = s.respond(&String::from_utf8_lossy(body)).map_err(bad)?;
                        up.write_all(&message(b'p', reply.as_bytes()))?;
                        up.flush()?;
                    }
                    12 => {
                        let s = scram
                            .as_ref()
                            .ok_or_else(|| bad("SASL final without a start"))?;
                        if !s.verify(&String::from_utf8_lossy(body)) {
                            client.write_all(&error_response(
                                "28000",
                                "the database could not prove it knows the password",
                            ))?;
                            return Ok(());
                        }
                    }
                    _ => {
                        client.write_all(&error_response(
                            "28000",
                            "the database asked for an authentication method penv does not speak",
                        ))?;
                        return Ok(());
                    }
                }
            }
            _ => return Err(bad("a message before authentication finished")),
        }
    }
    client.set_read_timeout(None)?;
    upstream::pipe(client, up)
}

fn send_password(up: &mut Upstream, password: &[u8]) -> io::Result<()> {
    let mut payload = password.to_vec();
    payload.push(0);
    up.write_all(&message(b'p', &payload))?;
    up.flush()
}

fn parameter(params: &[u8], name: &str) -> Option<String> {
    let mut parts = params.split(|b| *b == 0);
    while let Some(key) = parts.next() {
        let value = parts.next()?;
        if key == name.as_bytes() {
            return Some(String::from_utf8_lossy(value).into_owned());
        }
    }
    None
}

fn connect(target: &Target) -> io::Result<Upstream> {
    let mut tcp = upstream::tcp(&target.host, target.port)?;
    let config = match &target.tls {
        Tls::Off => return Ok(Upstream::Plain(tcp)),
        Tls::Prefer(c) | Tls::Require(c) | Tls::Verify(c) => c.clone(),
    };
    let mut request = Vec::new();
    request.extend_from_slice(&8i32.to_be_bytes());
    request.extend_from_slice(&SSL_REQUEST.to_be_bytes());
    tcp.write_all(&request)?;
    let mut answer = [0u8; 1];
    tcp.read_exact(&mut answer)?;
    match (answer[0], &target.tls) {
        (b'S', _) => upstream::tls(tcp, &target.host, config),
        (b'N', Tls::Prefer(_)) => Ok(Upstream::Plain(tcp)),
        _ => Err(bad(
            "a database that will not use TLS, though the URL's sslmode requires it",
        )),
    }
}
