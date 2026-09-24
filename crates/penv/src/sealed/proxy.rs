//! The local proxy a sealed run's child sends its HTTP through. For a host a
//! sealed key allows, it terminates TLS with a certificate from the run's own
//! authority, swaps each placeholder for its value in the request, and swaps
//! each value back to its placeholder in the response. Every other host is a
//! plain tunnel: nothing is read and nothing is swapped, so a placeholder sent
//! there stays a placeholder.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
};

use super::ca::Ca;
use super::http::{self, Head, Swaps};

const IDLE: Duration = Duration::from_secs(300);

/// Byte-string swaps: `(from, to)`.
type Pairs = Vec<(Vec<u8>, Vec<u8>)>;

/// One key a sealed run holds back from the child.
#[derive(Debug, Clone)]
pub struct Sealed {
    pub name: String,
    pub placeholder: String,
    pub value: String,
    pub hosts: Vec<String>,
    /// An AWS secret access key: never sent, used to sign requests again.
    pub sigv4: bool,
}

pub struct Proxy {
    keys: Vec<Sealed>,
    ca: Ca,
    upstream: Arc<ClientConfig>,
}

impl Proxy {
    /// `extra_roots` are trusted for upstream hosts beside the Mozilla roots:
    /// the bundle `SSL_CERT_FILE` names, when penv trusts it.
    pub fn new(
        keys: Vec<Sealed>,
        extra_roots: Vec<CertificateDer<'static>>,
    ) -> Result<Proxy, String> {
        let ca = Ca::new().map_err(|e| e.to_string())?;
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for cert in extra_roots {
            let _ = roots.add(cert);
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut upstream = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        upstream.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Proxy {
            keys,
            ca,
            upstream: Arc::new(upstream),
        })
    }

    pub fn ca_pem(&self) -> &str {
        self.ca.pem()
    }

    /// Listen on a loopback port and serve until the process ends.
    pub fn start(self) -> io::Result<u16> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let proxy = Arc::new(self);
        super::serve_each(listener, move |stream| {
            let _ = proxy.serve(stream);
        });
        Ok(port)
    }

    fn allowed(&self, host: &str) -> bool {
        self.keys
            .iter()
            .any(|k| penv_schema::placeholder::host_allowed(&k.hosts, host))
    }

    /// One request on its way up: values into the head, and a SigV4 signature
    /// made again with the real secret access key when one may go to `host`.
    /// Returns what the response must swap back beside the values: the
    /// signature penv made, as the one the command sent.
    fn send(
        &self,
        reader: &mut impl BufRead,
        writer: &mut impl Write,
        request: Head,
        framing: http::Framing,
        host: &str,
        swaps: &Swaps,
    ) -> io::Result<Pairs> {
        let aws = self
            .keys
            .iter()
            .find(|k| k.sigv4 && penv_schema::placeholder::host_allowed(&k.hosts, host));
        let (Some(aws), true) = (aws, super::sigv4::is_signed(&request)) else {
            http::relay(
                reader,
                writer,
                request,
                framing,
                swaps,
                &Swaps::default(),
                true,
            )?;
            return Ok(Vec::new());
        };
        let sent = super::sigv4::signature(&request).unwrap_or_default();
        let mut head = request.swapped(swaps);
        let echo = |made: String| vec![(made.into_bytes(), sent.clone().into_bytes())];
        // A declared payload hash (S3) is signed as it is and the body streams
        // up untouched; otherwise the body is read and hashed first.
        if let Some(hash) = super::sigv4::declared_hash(&head).map_err(refused)? {
            let made = super::sigv4::resign(&mut head, &hash, &aws.value).map_err(refused)?;
            http::relay(
                reader,
                writer,
                head,
                framing,
                &Swaps::default(),
                &Swaps::default(),
                true,
            )?;
            return Ok(echo(made));
        }
        let body = match framing {
            http::Framing::None => Vec::new(),
            http::Framing::Length(n) if n <= http::MAX_BODY => super::read_len(reader, n)?,
            _ => {
                return Err(refused(
                    "a signed AWS request whose body penv cannot hash (chunked or over 64 MiB)",
                ));
            }
        };
        let hash = super::sigv4::sha256_hex(&body);
        let made = super::sigv4::resign(&mut head, &hash, &aws.value).map_err(refused)?;
        let framing = if body.is_empty() {
            http::Framing::None
        } else {
            http::Framing::Length(body.len())
        };
        http::relay(
            &mut io::Cursor::new(body),
            writer,
            head,
            framing,
            &Swaps::default(),
            &Swaps::default(),
            true,
        )?;
        Ok(echo(made))
    }

    /// Placeholder → value, for the keys that may go to `host`.
    fn request_swaps(&self, host: &str) -> Swaps {
        Swaps::new(
            self.keys
                .iter()
                .filter(|k| !k.sigv4 && penv_schema::placeholder::host_allowed(&k.hosts, host))
                .map(|k| {
                    (
                        k.placeholder.clone().into_bytes(),
                        k.value.clone().into_bytes(),
                    )
                })
                .collect(),
        )
    }

    fn serve(&self, stream: TcpStream) -> io::Result<()> {
        stream.set_read_timeout(Some(IDLE))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let Some(head) = http::read_head(&mut reader)? else {
            return Ok(());
        };
        let mut parts = head.start.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let target = parts.next().unwrap_or_default().to_string();
        if method.eq_ignore_ascii_case("CONNECT") {
            let (host, port) =
                split_host_port(&target, 443).ok_or_else(|| refused("a CONNECT target"))?;
            let mut client = stream;
            client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")?;
            if self.allowed(&host) {
                return self.intercept(client, reader, &host, port);
            }
            return tunnel(client, reader, &host, port);
        }
        self.forward_plain(stream, reader, head, &target)
    }

    /// TLS both ways, rewriting each request and response.
    fn intercept(
        &self,
        client: TcpStream,
        reader: BufReader<TcpStream>,
        host: &str,
        port: u16,
    ) -> io::Result<()> {
        if !reader.buffer().is_empty() {
            return Err(refused("bytes sent before the tunnel opened"));
        }
        let leaf = self.ca.leaf(host).map_err(io::Error::other)?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut server = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(Fixed(leaf)));
        // One HTTP/1.1 exchange at a time is what the rewriting reads.
        server.alpn_protocols = vec![b"http/1.1".to_vec()];
        let client_tls = ServerConnection::new(Arc::new(server)).map_err(io::Error::other)?;
        let mut from_client = BufReader::new(StreamOwned::new(client_tls, client));

        let upstream_tcp = connect(host, port)?;
        let name = ServerName::try_from(host.to_string()).map_err(|_| refused("a host name"))?;
        let upstream_tls =
            ClientConnection::new(self.upstream.clone(), name).map_err(io::Error::other)?;
        let mut from_upstream = BufReader::new(StreamOwned::new(upstream_tls, upstream_tcp));

        let request_swaps = self.request_swaps(host);
        let base_swaps = response_swaps(&self.keys);
        loop {
            let Some(mut request) = http::read_head(&mut from_client)? else {
                return Ok(());
            };
            if let Err(e) = same_origin(&request, host, port) {
                fail(from_client.get_mut(), "421 Misdirected Request", &e);
                return Err(e);
            }
            let method = request
                .start
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();
            let chunked_ok = request.start.ends_with("HTTP/1.1");
            let mut echoes: Pairs = basic_auth(&mut request, &request_swaps)
                .into_iter()
                .collect();
            // Compressed WebSocket frames cannot be read for values to swap back.
            request.remove("sec-websocket-extensions");
            // Compressed responses cannot be read for values to swap back.
            request.set("Accept-Encoding", "identity".to_string());
            if continue_expected(&mut request) {
                from_client
                    .get_mut()
                    .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            }
            let closing = request
                .get("connection")
                .is_some_and(|v| v.eq_ignore_ascii_case("close"));
            let framing = http::request_framing(&request)?;
            // Values go into the request head only: a header or the request
            // line. A body goes up unchanged, so an allowed host's write API
            // (a gist, an issue, a message) cannot be used to publish a value.
            echoes.extend(self.send(
                &mut from_client,
                from_upstream.get_mut(),
                request,
                framing,
                host,
                &request_swaps,
            )?);
            let response_swaps = if echoes.is_empty() {
                base_swaps.clone()
            } else {
                base_swaps.with(echoes)
            };

            let mut response = loop {
                let head = http::read_head(&mut from_upstream)?
                    .ok_or_else(|| refused("an upstream that closed"))?;
                match head.status() {
                    Some(s) if (100..200).contains(&s) && s != 101 => {
                        from_client
                            .get_mut()
                            .write_all(&head.to_bytes(&response_swaps))?;
                    }
                    _ => break head,
                }
            };
            if response.status() == Some(101) {
                let websocket = response
                    .get("upgrade")
                    .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
                if !websocket {
                    return Err(refused("an upgrade to something other than WebSocket"));
                }
                from_client
                    .get_mut()
                    .write_all(&response.to_bytes(&response_swaps))?;
                from_client.get_mut().flush()?;
                return super::websocket::splice(from_client, from_upstream, response_swaps);
            }
            let framing = http::response_framing(&response, &method)?;
            if let Err(e) = http::readable(&response, &framing) {
                fail(from_client.get_mut(), "502 Bad Gateway", &e);
                return Err(e);
            }
            let ends = framing == http::Framing::Close
                || response
                    .get("connection")
                    .is_some_and(|v| v.eq_ignore_ascii_case("close"));
            if ends {
                response.set("Connection", "close".to_string());
            }
            http::relay(
                &mut from_upstream,
                from_client.get_mut(),
                response,
                framing,
                &response_swaps,
                &response_swaps,
                chunked_ok,
            )?;
            if closing || ends {
                from_client.get_mut().conn.send_close_notify();
                let _ = from_client.get_mut().flush();
                return Ok(());
            }
        }
    }

    /// Plain HTTP through the proxy (`GET http://host/path`). Values go over
    /// plain HTTP only to loopback hosts, where tests and local servers live;
    /// anywhere else the request leaves with its placeholders.
    fn forward_plain(
        &self,
        client: TcpStream,
        mut reader: BufReader<TcpStream>,
        head: Head,
        target: &str,
    ) -> io::Result<()> {
        let rest = target
            .strip_prefix("http://")
            .ok_or_else(|| refused("a request that is neither CONNECT nor http://"))?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (host, port) = split_host_port(authority, 80).ok_or_else(|| refused("a host"))?;
        let loopback = matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1");
        let upstream = connect(&host, port)?;
        let mut from_upstream = BufReader::new(upstream.try_clone()?);
        let mut to_upstream = upstream;
        let mut client = client;
        let swap = loopback && self.allowed(&host);
        let host = if swap { Some(host.as_str()) } else { None };
        self.exchange_plain(
            &mut reader,
            &mut client,
            &mut from_upstream,
            &mut to_upstream,
            head,
            path,
            host,
        )?;
        let _ = client.shutdown(Shutdown::Both);
        Ok(())
    }

    /// One plain request and its response, after which the proxy closes, so
    /// the response says so. `swap_for` is the host values may go to, if any.
    #[allow(clippy::too_many_arguments)]
    fn exchange_plain(
        &self,
        from_client: &mut impl BufRead,
        to_client: &mut impl Write,
        from_upstream: &mut impl BufRead,
        to_upstream: &mut impl Write,
        mut head: Head,
        path: &str,
        swap_for: Option<&str>,
    ) -> io::Result<()> {
        let mut parts = head.start.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let version = head
            .start
            .rsplit(' ')
            .next()
            .unwrap_or("HTTP/1.1")
            .to_string();
        head.start = format!("{method} {path} {version}");
        head.set("Connection", "close".to_string());
        head.remove("proxy-connection");
        head.set("Accept-Encoding", "identity".to_string());
        if continue_expected(&mut head) {
            to_client.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            to_client.flush()?;
        }
        let framing = http::request_framing(&head)?;
        let echoes = match swap_for {
            Some(host) => {
                let request_swaps = self.request_swaps(host);
                let mut echoes: Pairs = basic_auth(&mut head, &request_swaps).into_iter().collect();
                echoes.extend(self.send(
                    from_client,
                    to_upstream,
                    head,
                    framing,
                    host,
                    &request_swaps,
                )?);
                echoes
            }
            None => {
                http::relay(
                    from_client,
                    to_upstream,
                    head,
                    framing,
                    &Swaps::default(),
                    &Swaps::default(),
                    true,
                )?;
                Vec::new()
            }
        };
        to_upstream.flush()?;
        let swaps = response_swaps(&self.keys).with(echoes);
        let mut response = loop {
            let Some(head) = http::read_head(from_upstream)? else {
                return Ok(());
            };
            match head.status() {
                Some(s) if (100..200).contains(&s) && s != 101 => {
                    to_client.write_all(&head.to_bytes(&swaps))?;
                }
                _ => break head,
            }
        };
        let framing = http::response_framing(&response, &method)?;
        if let Err(e) = http::readable(&response, &framing) {
            fail(to_client, "502 Bad Gateway", &e);
            return Err(e);
        }
        response.set("Connection", "close".to_string());
        http::relay(
            from_upstream,
            to_client,
            response,
            framing,
            &swaps,
            &swaps,
            version == "HTTP/1.1",
        )
    }
}

/// Value → placeholder, for every sealed key: an echoed secret goes back to
/// the child only as its placeholder, as written and in the forms an echo
/// commonly carries it: base64 (a decoded Basic header, at each of the three
/// alignments a value can take inside a longer encoding), percent-encoded, hex.
fn response_swaps(keys: &[Sealed]) -> Swaps {
    let mut swaps: Pairs = Vec::new();
    for k in keys.iter().filter(|k| k.value.len() >= 4) {
        let (value, placeholder) = (k.value.as_bytes(), k.placeholder.as_bytes());
        let mut forms = vec![
            (k.value.clone(), k.placeholder.clone()),
            (
                penv_cloud::b64::encode(value),
                penv_cloud::b64::encode(placeholder),
            ),
            (percent(value, false), percent(placeholder, false)),
            (percent(value, true), percent(placeholder, true)),
            (hex(value, false), hex(placeholder, false)),
            (hex(value, true), hex(placeholder, true)),
        ];
        for offset in 0..3 {
            let core = b64_core(value, offset);
            // Shorter, a run of base64 could match by chance.
            if core.len() >= 16 {
                forms.push((core, b64_core(placeholder, offset)));
            }
        }
        for (from, to) in forms {
            if from.len() >= 4 {
                swaps.push((from.into_bytes(), to.into_bytes()));
            }
        }
    }
    Swaps::new(swaps)
}

/// The base64 characters that encode `value` alone when it starts `offset`
/// bytes into a group of three: the part of any longer encoding holding it
/// that does not depend on the bytes around it.
fn b64_core(value: &[u8], offset: usize) -> String {
    let mut bytes = vec![0u8; offset];
    bytes.extend_from_slice(value);
    let encoded = penv_cloud::b64::encode(&bytes);
    // Character j holds bits 6j..6j+6; keep those wholly inside the value.
    let first = (8 * offset).div_ceil(6);
    let last = (8 * (offset + value.len())) / 6;
    encoded
        .get(first..last.max(first))
        .unwrap_or("")
        .to_string()
}

/// `Authorization: Basic base64(user:placeholder)`, as `curl -u` and most HTTP
/// clients send it: the placeholder is inside the base64, so decode, swap, and
/// encode again. Returns the header as sent and as the command wrote it, for
/// the response to swap back: an echo of it would otherwise carry the value.
fn basic_auth(head: &mut Head, swaps: &Swaps) -> Option<(Vec<u8>, Vec<u8>)> {
    let value = head.get("authorization")?.to_string();
    let encoded = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))?
        .trim();
    let decoded = penv_cloud::b64::decode(encoded)?;
    let swapped = swaps.apply(&decoded);
    if swapped == decoded {
        return None;
    }
    let sent = penv_cloud::b64::encode(&swapped);
    head.set("Authorization", format!("Basic {sent}"));
    Some((sent.into_bytes(), encoded.as_bytes().to_vec()))
}

/// Whether the request waits for `100 Continue`, which the proxy then answers
/// itself: the body is read as one piece with the head before anything goes up.
fn continue_expected(head: &mut Head) -> bool {
    let expected = head
        .get("expect")
        .is_some_and(|v| v.eq_ignore_ascii_case("100-continue"));
    head.remove("expect");
    expected
}

/// A request on a tunnel opened to `host:port` must be for that origin. At a
/// front end that routes by Host, another name would reach another origin,
/// with the values in the request.
fn same_origin(request: &Head, host: &str, port: u16) -> io::Result<()> {
    let origin = Some((host.to_string(), port));
    let target = request.start.split_whitespace().nth(1).unwrap_or_default();
    let lower = target.to_ascii_lowercase();
    let absolute = [("https://", 443), ("http://", 80)]
        .into_iter()
        .find_map(|(scheme, default)| lower.strip_prefix(scheme).map(|rest| (rest, default)));
    if let Some((rest, default)) = absolute {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if split_host_port(authority, default) != origin {
            return Err(refused(
                "a request for another host than the tunnel was opened to",
            ));
        }
    }
    let hosts: Vec<&str> = request.all("host").collect();
    match hosts.as_slice() {
        [one] if split_host_port(one.trim(), 443) == origin => Ok(()),
        [_] => Err(refused(
            "a request whose Host is not the host the tunnel was opened to",
        )),
        _ => Err(refused("a request without exactly one Host header")),
    }
}

/// Tell the command why its request went no further.
fn fail(writer: &mut impl Write, status: &str, why: &io::Error) {
    let body = format!("{why}\n");
    let _ = write!(
        writer,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = writer.flush();
}

fn percent(value: &[u8], lower: bool) -> String {
    value
        .iter()
        .map(|&b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else if lower {
                format!("%{b:02x}")
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn hex(value: &[u8], upper: bool) -> String {
    value
        .iter()
        .map(|b| {
            if upper {
                format!("{b:02X}")
            } else {
                format!("{b:02x}")
            }
        })
        .collect()
}

#[derive(Debug)]
struct Fixed(Arc<CertifiedKey>);

impl ResolvesServerCert for Fixed {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

fn refused(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("the sealed proxy refused {what}"),
    )
}

fn split_host_port(authority: &str, default: u16) -> Option<(String, u16)> {
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        // [v6]:port
        let (h, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None if after.is_empty() => default,
            None => return None,
        };
        (h, port)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (h, p.parse().ok()?),
            Some(_) => return None,
            None => (authority, default),
        }
    };
    let host = host.to_ascii_lowercase();
    (!host.is_empty()).then_some((host, port))
}

fn connect(host: &str, port: u16) -> io::Result<TcpStream> {
    let addrs: Vec<_> = (host, port).to_socket_addrs()?.collect();
    let mut last = io::Error::other("no address");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(15)) {
            Ok(s) => {
                s.set_read_timeout(Some(IDLE))?;
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Bytes both ways, unread.
fn tunnel(
    client: TcpStream,
    reader: BufReader<TcpStream>,
    host: &str,
    port: u16,
) -> io::Result<()> {
    let upstream = connect(host, port)?;
    let mut up_write = upstream.try_clone()?;
    up_write.write_all(reader.buffer())?;
    let mut client_read = client.try_clone()?;
    let copy_up = thread::spawn(move || {
        let _ = io::copy(&mut client_read, &mut up_write);
        let _ = up_write.shutdown(Shutdown::Write);
    });
    let mut up_read = upstream;
    let mut client_write = client;
    let _ = io::copy(&mut up_read, &mut client_write);
    let _ = client_write.shutdown(Shutdown::Write);
    let _ = copy_up.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const VALUE: &str = "sk_test_FAKEvalue0123456789abcdef";
    const PLACEHOLDER: &str = "sk_test_penvphPLACEHOLDERxyz01234";

    fn key() -> Sealed {
        Sealed {
            name: "API_KEY".into(),
            placeholder: PLACEHOLDER.into(),
            value: VALUE.into(),
            hosts: vec!["api.test".into()],
            sigv4: false,
        }
    }

    fn head(text: &str) -> Head {
        http::read_head(&mut Cursor::new(text.as_bytes().to_vec()))
            .unwrap()
            .unwrap()
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn authorities_split_and_user_info_is_refused() {
        assert_eq!(
            split_host_port("API.test:8443", 443),
            Some(("api.test".into(), 8443))
        );
        assert_eq!(
            split_host_port("api.test", 443),
            Some(("api.test".into(), 443))
        );
        assert_eq!(split_host_port("u@api.test:443", 443), None);
        assert_eq!(split_host_port("[::1]:5", 1), Some(("::1".into(), 5)));
    }

    #[test]
    fn an_echoed_basic_header_comes_back_as_the_command_sent_it() {
        let request_swaps = Swaps::new(vec![(PLACEHOLDER.into(), VALUE.into())]);
        let original = penv_cloud::b64::encode(format!("user:{PLACEHOLDER}").as_bytes());
        let mut request = head(&format!(
            "GET / HTTP/1.1\r\nHost: api.test\r\nAuthorization: Basic {original}\r\n\r\n"
        ));
        let echo = basic_auth(&mut request, &request_swaps).unwrap();
        let sent = request.get("authorization").unwrap().to_string();
        assert_ne!(sent, format!("Basic {original}"));
        let swaps = response_swaps(&[key()]).with(vec![echo]);
        let out = swaps.apply(format!("{{\"authorization\":\"{sent}\"}}").as_bytes());
        assert_eq!(
            out,
            format!("{{\"authorization\":\"Basic {original}\"}}").as_bytes()
        );
    }

    #[test]
    fn a_value_inside_a_longer_base64_encoding_is_swapped_at_every_alignment() {
        let swaps = response_swaps(&[key()]);
        for prefix in ["", "a", "u:", "user:"] {
            let encoded = penv_cloud::b64::encode(format!("{prefix}{VALUE}!").as_bytes());
            let out = swaps.apply(encoded.as_bytes());
            let decoded =
                penv_cloud::b64::decode(std::str::from_utf8(&out).unwrap()).unwrap_or_default();
            assert!(!contains(&decoded, VALUE.as_bytes()), "{prefix:?}");
            assert!(
                contains(&decoded, &PLACEHOLDER.as_bytes()[1..PLACEHOLDER.len() - 1]),
                "{prefix:?}"
            );
        }
    }

    #[test]
    fn percent_and_hex_echoes_in_either_case_are_swapped() {
        let value = "fake/secret+value=0123456789";
        let k = Sealed {
            value: value.into(),
            ..key()
        };
        let swaps = response_swaps(&[k]);
        for form in [
            percent(value.as_bytes(), false),
            percent(value.as_bytes(), true),
            hex(value.as_bytes(), false),
            hex(value.as_bytes(), true),
        ] {
            assert_ne!(swaps.apply(form.as_bytes()), form.as_bytes(), "{form}");
        }
    }

    #[test]
    fn a_request_for_another_origin_on_the_tunnel_is_refused() {
        let ok = [
            "GET / HTTP/1.1\r\nHost: api.test\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: API.test:443\r\n\r\n",
            "GET https://api.test/v1 HTTP/1.1\r\nHost: api.test\r\n\r\n",
        ];
        for text in ok {
            assert!(
                same_origin(&head(text), "api.test", 443).is_ok(),
                "{text:?}"
            );
        }
        let refused = [
            "GET / HTTP/1.1\r\nHost: other.test\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: api.test:8443\r\n\r\n",
            "GET / HTTP/1.1\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: api.test\r\nHost: other.test\r\n\r\n",
            "GET https://other.test/v1 HTTP/1.1\r\nHost: api.test\r\n\r\n",
        ];
        for text in refused {
            assert!(
                same_origin(&head(text), "api.test", 443).is_err(),
                "{text:?}"
            );
        }
    }

    #[test]
    fn plain_http_answers_expect_itself_and_says_it_closes() {
        let proxy = Proxy::new(vec![key()], Vec::new()).unwrap();
        let mut from_client = Cursor::new(b"hello".to_vec());
        let mut to_client = Vec::new();
        let mut from_upstream =
            Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec());
        let mut to_upstream = Vec::new();
        let request = head(
            "POST http://127.0.0.1:9/up HTTP/1.1\r\nHost: 127.0.0.1:9\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n",
        );
        proxy
            .exchange_plain(
                &mut from_client,
                &mut to_client,
                &mut from_upstream,
                &mut to_upstream,
                request,
                "/up",
                None,
            )
            .unwrap();
        let to_client = String::from_utf8(to_client).unwrap();
        assert!(
            to_client.starts_with("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\n"),
            "{to_client}"
        );
        assert!(to_client.contains("Connection: close\r\n"), "{to_client}");
        let to_upstream = String::from_utf8(to_upstream).unwrap();
        assert!(
            !to_upstream.to_ascii_lowercase().contains("expect"),
            "{to_upstream}"
        );
        assert!(to_upstream.ends_with("\r\n\r\nhello"), "{to_upstream}");
    }

    #[test]
    fn a_compressed_response_on_plain_http_is_refused_not_relayed() {
        let proxy = Proxy::new(vec![key()], Vec::new()).unwrap();
        let mut to_client = Vec::new();
        let mut from_upstream = Cursor::new(
            b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\n\r\nzzzz".to_vec(),
        );
        let result = proxy.exchange_plain(
            &mut Cursor::new(Vec::new()),
            &mut to_client,
            &mut from_upstream,
            &mut Vec::new(),
            head("GET http://127.0.0.1:9/ HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\r\n"),
            "/",
            None,
        );
        assert!(result.is_err());
        let to_client = String::from_utf8(to_client).unwrap();
        assert!(to_client.starts_with("HTTP/1.1 502 "), "{to_client}");
        assert!(!to_client.contains("zzzz"));
    }
}
