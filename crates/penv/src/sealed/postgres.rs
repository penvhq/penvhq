//! A sealed Postgres URL. The command connects to a loopback port with the
//! placeholder as its password; penv checks the placeholder, opens the real
//! connection (TLS as the URL's sslmode says) with the real password, and then
//! passes bytes both ways unread.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use rustls::ClientConfig;

use super::read_len;
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
    /// The URL's user: the role the real password belongs to. Empty leaves the
    /// command's own (libpq's default, the login name).
    pub user: String,
    pub password: String,
    pub placeholder: String,
    pub tls: Tls,
    /// The ways penv may answer the server, from the URL's `require_auth`.
    pub auth: Auth,
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

/// Authentication methods the server may ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Auth {
    pub password: bool,
    pub md5: bool,
    pub scram: bool,
    /// Logging in without being asked for anything.
    pub none: bool,
}

impl Auth {
    pub const ANY: Auth = Auth {
        password: true,
        md5: true,
        scram: true,
        none: true,
    };
}

/// libpq's `require_auth`: a list of methods, or of `!method`s to rule out.
pub fn require_auth(text: &str) -> Result<Auth, String> {
    let methods: Vec<&str> = text.split(',').map(str::trim).collect();
    let negated = methods.iter().all(|m| m.starts_with('!'));
    if !negated && methods.iter().any(|m| m.starts_with('!')) {
        return Err(format!("require_auth={text} mixes methods with !methods"));
    }
    let mut auth = if negated {
        Auth::ANY
    } else {
        Auth {
            password: false,
            md5: false,
            scram: false,
            none: false,
        }
    };
    for method in methods {
        let slot = match method.strip_prefix('!').unwrap_or(method) {
            "password" => &mut auth.password,
            "md5" => &mut auth.md5,
            "scram-sha-256" => &mut auth.scram,
            "none" => &mut auth.none,
            // Methods penv never answers with; allowing them allows nothing.
            "gss" | "sspi" | "oauth" => continue,
            other => {
                return Err(format!(
                    "require_auth names {other}, which libpq does not know"
                ));
            }
        };
        *slot = !negated;
    }
    Ok(auth)
}

pub fn start(target: Target) -> io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let target = Arc::new(target);
    super::serve_each(listener, move |client| {
        let _ = serve(client, &target);
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
    let payload = read_len(r, len as usize - 4)?;
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

/// What the command opened its connection with.
#[derive(Debug, PartialEq, Eq)]
enum Startup {
    /// A login: the startup parameters.
    Params(Vec<u8>),
    /// A cancel request for a running query: the whole 16-byte packet.
    Cancel(Vec<u8>),
}

/// Refuse TLS and GSS on the loopback leg, then read the startup packet.
fn startup(client: &mut (impl Read + Write)) -> io::Result<Startup> {
    loop {
        let len = read_i32(client)?;
        if !(8..=10_000).contains(&len) {
            return Err(bad("a startup length out of range"));
        }
        let code = read_i32(client)?;
        let rest = read_len(client, len as usize - 8)?;
        match code {
            SSL_REQUEST | GSS_REQUEST => client.write_all(b"N")?,
            CANCEL_REQUEST if len == 16 => {
                let mut packet = len.to_be_bytes().to_vec();
                packet.extend_from_slice(&code.to_be_bytes());
                packet.extend_from_slice(&rest);
                return Ok(Startup::Cancel(packet));
            }
            CANCEL_REQUEST => return Err(bad("a cancel request of the wrong length")),
            PROTOCOL_3 => return Ok(Startup::Params(rest)),
            _ => return Err(bad("a protocol version other than 3")),
        }
    }
}

/// Startup parameters as name/value pairs, up to the empty name that ends them.
fn pairs(params: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    let mut parts = params.split(|b| *b == 0);
    while let (Some(key), Some(value)) = (parts.next(), parts.next()) {
        if key.is_empty() {
            break;
        }
        out.push((key, value));
    }
    out
}

/// The parameters with `user` pinned to the URL's, so the real password is
/// only ever spent on its own role. Postgres takes the last `user` it is
/// sent, so every one given must match and one is sent.
fn pin_user(params: &[u8], user: &str) -> Result<Vec<u8>, String> {
    if user.is_empty() {
        return Ok(params.to_vec());
    }
    let pairs = pairs(params);
    if let Some((_, given)) = pairs
        .iter()
        .find(|(k, v)| *k == b"user" && *v != user.as_bytes())
    {
        return Err(format!(
            "penv's sealed URL logs in as {user}, not {} (penv sealed proxy)",
            String::from_utf8_lossy(given)
        ));
    }
    let mut out = format!("user\0{user}\0").into_bytes();
    for (key, value) in pairs.into_iter().filter(|(k, _)| *k != b"user") {
        out.extend_from_slice(key);
        out.push(0);
        out.extend_from_slice(value);
        out.push(0);
    }
    out.push(0);
    Ok(out)
}

fn serve(mut client: TcpStream, target: &Target) -> io::Result<()> {
    client.set_read_timeout(Some(Duration::from_secs(60)))?;
    let params = match startup(&mut client)? {
        Startup::Params(params) => params,
        Startup::Cancel(packet) => {
            // A connection of its own that carries only the cancel packet, as
            // libpq sends it; the server checks the key it holds.
            let mut up = connect(target)?;
            up.write_all(&packet)?;
            return up.flush();
        }
    };
    let params = match pin_user(&params, &target.user) {
        Ok(params) => params,
        Err(text) => {
            client.write_all(&error_response("28000", &text))?;
            return Ok(());
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

    let nonce = penv_cloud::b64::encode(&penv_cloud::random_bytes(18).map_err(io::Error::other)?);
    match login(&mut up, target, &user, &nonce)? {
        Login::Refused(error) => {
            client.write_all(&error)?;
            return Ok(());
        }
        Login::Ok(payload) => client.write_all(&message(b'R', &payload))?,
    }
    client.set_read_timeout(None)?;
    upstream::pipe(client, up)
}

/// How logging in to the real server ended.
#[derive(Debug, PartialEq, Eq)]
enum Login {
    /// AuthenticationOk's payload, to pass on to the command.
    Ok(Vec<u8>),
    /// An ErrorResponse to send the command instead.
    Refused(Vec<u8>),
}

/// Answer the server's authentication requests with the real password, in the
/// ways `target.auth` allows. Once SCRAM has begun, only its own steps may
/// follow, and success counts only after the server has proved it knows the
/// password.
fn login(
    up: &mut (impl Read + Write),
    target: &Target,
    user: &str,
    nonce: &str,
) -> io::Result<Login> {
    let refuse = |code: &str, text: &str| Ok(Login::Refused(error_response(code, text)));
    let not_allowed = |what: &str| {
        refuse(
            "28000",
            &format!(
                "the database asked for {what}, which the URL's require_auth does not allow (penv sealed proxy)"
            ),
        )
    };
    let mut scram: Option<Scram> = None;
    let mut verified = false;
    let mut asked = false;
    loop {
        let (kind, payload) = read_message(up)?;
        match kind {
            b'E' => return Ok(Login::Refused(message(b'E', &payload))),
            b'R' if payload.len() >= 4 => {
                let code = i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                let body = &payload[4..];
                if scram.is_some() && !matches!(code, 0 | 11 | 12) {
                    return refuse(
                        "28000",
                        "the database left SCRAM for another method midway (penv sealed proxy)",
                    );
                }
                match code {
                    0 => {
                        if scram.is_some() && !verified {
                            return refuse(
                                "28000",
                                "the database ended SCRAM without proving it knows the password (penv sealed proxy)",
                            );
                        }
                        if !asked && !target.auth.none {
                            return not_allowed("no authentication");
                        }
                        return Ok(Login::Ok(payload));
                    }
                    3 if !target.auth.password => return not_allowed("a cleartext password"),
                    5 if !target.auth.md5 => return not_allowed("an MD5 password"),
                    10 if !target.auth.scram => return not_allowed("SASL"),
                    3 => {
                        asked = true;
                        send_password(up, target.password.as_bytes())?;
                    }
                    5 if body.len() >= 4 => {
                        asked = true;
                        let salt = [body[0], body[1], body[2], body[3]];
                        send_password(up, md5_password(&target.password, user, &salt).as_bytes())?;
                    }
                    10 => {
                        let mechanisms: Vec<&[u8]> = body.split(|b| *b == 0).collect();
                        if !mechanisms.contains(&&b"SCRAM-SHA-256"[..]) {
                            return refuse(
                                "28000",
                                "the database offered no SASL mechanism penv speaks",
                            );
                        }
                        asked = true;
                        let s = Scram::new(&target.password, nonce);
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
                            return refuse(
                                "28000",
                                "the database could not prove it knows the password",
                            );
                        }
                        verified = true;
                    }
                    _ => {
                        return refuse(
                            "28000",
                            "the database asked for an authentication method penv does not speak",
                        );
                    }
                }
            }
            _ => return Err(bad("a message before authentication finished")),
        }
    }
}

fn send_password(up: &mut impl Write, password: &[u8]) -> io::Result<()> {
    let mut payload = password.to_vec();
    payload.push(0);
    up.write_all(&message(b'p', &payload))?;
    up.flush()
}

fn parameter(params: &[u8], name: &str) -> Option<String> {
    pairs(params)
        .into_iter()
        .find(|(k, _)| *k == name.as_bytes())
        .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A server that says what it was given to say, and keeps what it is sent.
    struct Script {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Script {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn script(messages: &[Vec<u8>]) -> Script {
        Script {
            input: Cursor::new(messages.concat()),
            output: Vec::new(),
        }
    }

    fn auth(code: i32, body: &[u8]) -> Vec<u8> {
        let mut payload = code.to_be_bytes().to_vec();
        payload.extend_from_slice(body);
        message(b'R', &payload)
    }

    fn target(auth: Auth) -> Target {
        Target {
            host: "db.test".into(),
            port: 5432,
            user: "app".into(),
            password: "fake-db-password".into(),
            placeholder: "penvphFAKE".into(),
            tls: Tls::Off,
            auth,
        }
    }

    fn refused(login: Login) -> bool {
        matches!(login, Login::Refused(_))
    }

    #[test]
    fn success_after_scram_began_needs_the_servers_proof() {
        let mut up = script(&[auth(10, b"SCRAM-SHA-256\0\0"), auth(0, b"")]);
        let result = login(&mut up, &target(Auth::ANY), "app", "nonce").unwrap();
        assert!(refused(result));

        let mut up = script(&[auth(10, b"SCRAM-SHA-256\0\0"), auth(3, b"")]);
        let result = login(&mut up, &target(Auth::ANY), "app", "nonce").unwrap();
        assert!(refused(result), "no downgrade to cleartext midway");
        assert!(!up.output.windows(16).any(|w| w == b"fake-db-password"));
    }

    #[test]
    fn require_auth_limits_what_penv_answers() {
        let scram_only = require_auth("scram-sha-256").unwrap();
        for request in [auth(3, b""), auth(5, b"salt"), auth(0, b"")] {
            let mut up = script(std::slice::from_ref(&request));
            let result = login(&mut up, &target(scram_only), "app", "nonce").unwrap();
            assert!(refused(result));
            assert!(
                up.output.is_empty(),
                "nothing is sent for a method ruled out"
            );
        }
        let mut up = script(&[auth(3, b""), auth(0, b"")]);
        let result = login(&mut up, &target(Auth::ANY), "app", "nonce").unwrap();
        assert_eq!(result, Login::Ok(0i32.to_be_bytes().to_vec()));
    }

    #[test]
    fn require_auth_is_read_as_libpq_reads_it() {
        assert_eq!(
            require_auth("scram-sha-256,none").unwrap(),
            Auth {
                password: false,
                md5: false,
                scram: true,
                none: true,
            }
        );
        assert_eq!(
            require_auth("!password,!md5").unwrap(),
            Auth {
                password: false,
                md5: false,
                scram: true,
                none: true,
            }
        );
        assert!(require_auth("scram-sha-256,!md5").is_err());
        assert!(require_auth("kerberos").is_err());
    }

    #[test]
    fn a_cancel_request_is_kept_whole_to_pass_on() {
        let mut packet = 16i32.to_be_bytes().to_vec();
        packet.extend_from_slice(&CANCEL_REQUEST.to_be_bytes());
        packet.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut client = script(std::slice::from_ref(&packet));
        assert_eq!(startup(&mut client).unwrap(), Startup::Cancel(packet));
    }

    #[test]
    fn the_user_is_pinned_to_the_urls() {
        let params = b"user\0app\0database\0db\0\0";
        assert_eq!(
            pin_user(params, "app").unwrap(),
            b"user\0app\0database\0db\0\0"
        );
        assert!(pin_user(b"user\0admin\0\0", "app").is_err());
        assert!(
            pin_user(b"user\0app\0user\0admin\0\0", "app").is_err(),
            "Postgres takes the last user"
        );
        assert_eq!(
            pin_user(b"database\0db\0\0", "app").unwrap(),
            b"user\0app\0database\0db\0\0"
        );
        assert_eq!(pin_user(b"user\0me\0\0", "").unwrap(), b"user\0me\0\0");
    }
}
