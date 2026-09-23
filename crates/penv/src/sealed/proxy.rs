//! The local proxy a sealed run's child sends its HTTP through. For a host a
//! sealed key allows, it terminates TLS with a certificate from the run's own
//! authority, swaps each placeholder for its value in the request, and swaps
//! each value back to its placeholder in the response. Every other host is a
//! plain tunnel: nothing is read and nothing is swapped, so a placeholder sent
//! there stays a placeholder.

use std::io::{self, BufReader, Write};
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

/// One key a sealed run holds back from the child.
#[derive(Debug, Clone)]
pub struct Sealed {
    pub name: String,
    pub placeholder: String,
    pub value: String,
    pub hosts: Vec<String>,
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
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let proxy = proxy.clone();
                thread::spawn(move || {
                    let _ = proxy.serve(stream);
                });
            }
        });
        Ok(port)
    }

    fn allowed(&self, host: &str) -> bool {
        self.keys
            .iter()
            .any(|k| penv_schema::placeholder::host_allowed(&k.hosts, host))
    }

    /// Placeholder → value, for the keys that may go to `host`.
    fn request_swaps(&self, host: &str) -> Swaps {
        Swaps(
            self.keys
                .iter()
                .filter(|k| penv_schema::placeholder::host_allowed(&k.hosts, host))
                .map(|k| {
                    (
                        k.placeholder.clone().into_bytes(),
                        k.value.clone().into_bytes(),
                    )
                })
                .collect(),
        )
    }

    /// Value → placeholder, for every sealed key: an echoed secret goes back
    /// to the child only as its placeholder.
    fn response_swaps(&self) -> Swaps {
        let mut swaps: Vec<(Vec<u8>, Vec<u8>)> = self
            .keys
            .iter()
            .filter(|k| k.value.len() >= 4)
            .map(|k| {
                (
                    k.value.clone().into_bytes(),
                    k.placeholder.clone().into_bytes(),
                )
            })
            .collect();
        // Longest first, so a value that contains another is swapped whole.
        swaps.sort_by_key(|s| std::cmp::Reverse(s.0.len()));
        Swaps(swaps)
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
        self.forward_plain(stream, reader, head, &method, &target)
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
        let response_swaps = self.response_swaps();
        loop {
            let Some(mut request) = http::read_head(&mut from_client)? else {
                return Ok(());
            };
            let method = request
                .start
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();
            // Compressed responses cannot be read for values to swap back.
            request.set("Accept-Encoding", "identity".to_string());
            if request
                .get("expect")
                .is_some_and(|v| v.eq_ignore_ascii_case("100-continue"))
            {
                request.remove("expect");
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
            http::relay(
                &mut from_client,
                from_upstream.get_mut(),
                request,
                framing,
                &request_swaps,
                &Swaps::default(),
            )?;

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
                // An upgraded connection (WebSocket) is passed through unread.
                from_client
                    .get_mut()
                    .write_all(&response.to_bytes(&response_swaps))?;
                return Err(refused("an upgrade through a sealed host"));
            }
            let framing = http::response_framing(&response, &method)?;
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
        mut head: Head,
        method: &str,
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
        let request_swaps = if loopback && self.allowed(&host) {
            self.request_swaps(&host)
        } else {
            Swaps::default()
        };
        head.start = format!(
            "{method} {path} {}",
            head.start.rsplit(' ').next().unwrap_or("HTTP/1.1")
        );
        head.set("Connection", "close".to_string());
        head.remove("proxy-connection");
        head.set("Accept-Encoding", "identity".to_string());
        let mut upstream = connect(&host, port)?;
        let framing = http::request_framing(&head)?;
        http::relay(
            &mut reader,
            &mut upstream,
            head,
            framing,
            &request_swaps,
            &Swaps::default(),
        )?;
        let mut from_upstream = BufReader::new(upstream);
        let Some(response) = http::read_head(&mut from_upstream)? else {
            return Ok(());
        };
        let framing = http::response_framing(&response, method)?;
        let mut client = client;
        http::relay(
            &mut from_upstream,
            &mut client,
            response,
            framing,
            &self.response_swaps(),
            &self.response_swaps(),
        )?;
        let _ = client.shutdown(Shutdown::Both);
        Ok(())
    }
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
}
