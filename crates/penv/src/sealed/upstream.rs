//! Connections from the sealed proxies to the real service: TCP, TLS with or
//! without a certificate check, and passing bytes both ways once set up.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, RootCertStore, SignatureScheme,
    StreamOwned,
};

pub enum Upstream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for Upstream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Upstream::Plain(s) => s.read(buf),
            Upstream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Upstream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Upstream::Plain(s) => s.write(buf),
            Upstream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Upstream::Plain(s) => s.flush(),
            Upstream::Tls(s) => s.flush(),
        }
    }
}

pub fn tcp(host: &str, port: u16) -> io::Result<TcpStream> {
    let mut last = io::Error::other("no address");
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(15)) {
            Ok(s) => {
                s.set_nodelay(true)?;
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

pub fn tls(tcp: TcpStream, host: &str, config: Arc<ClientConfig>) -> io::Result<Upstream> {
    let name =
        ServerName::try_from(host.to_string()).map_err(|_| io::Error::other("not a host name"))?;
    let conn = ClientConnection::new(config, name).map_err(io::Error::other)?;
    let mut stream = StreamOwned::new(conn, tcp);
    // Finish the handshake now, so a bad certificate fails here and not mid-query.
    while stream.conn.is_handshaking() {
        stream.conn.complete_io(&mut stream.sock)?;
    }
    Ok(Upstream::Tls(Box::new(stream)))
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// TLS that checks the certificate against the Mozilla roots and `extra`.
pub fn verified(extra: &[CertificateDer<'static>]) -> Result<Arc<ClientConfig>, String> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for cert in extra {
        let _ = roots.add(cert.clone());
    }
    Ok(Arc::new(
        ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}

/// TLS that encrypts without checking who answers: what libpq does for
/// `sslmode=require`, and what a database behind a private CA often relies on.
pub fn unverified() -> Result<Arc<ClientConfig>, String> {
    Ok(Arc::new(
        ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyCertificate))
            .with_no_client_auth(),
    ))
}

#[derive(Debug)]
struct AnyCertificate;

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Bytes both ways until either side closes. A TLS upstream is one object, so
/// both directions share it behind a lock, reading it with a short timeout so
/// a write is never held up for long.
pub fn pipe(client: TcpStream, up: Upstream) -> io::Result<()> {
    pipe_with(client, up, |bytes: &[u8]| bytes.to_vec())
}

/// `pipe`, with each read from the client passed through `rewrite` on its way
/// up. `rewrite` keeps its own state, so a message split across reads is seen whole.
pub fn pipe_with(
    client: TcpStream,
    up: Upstream,
    mut rewrite: impl FnMut(&[u8]) -> Vec<u8> + Send + 'static,
) -> io::Result<()> {
    match up {
        Upstream::Plain(up) => {
            let mut up_write = up.try_clone()?;
            let mut client_read = client.try_clone()?;
            let t = thread::spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                loop {
                    let n = match client_read.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if up_write.write_all(&rewrite(&buf[..n])).is_err() {
                        break;
                    }
                }
                let _ = up_write.shutdown(Shutdown::Write);
            });
            let mut up_read = up;
            let mut client_write = client;
            let _ = io::copy(&mut up_read, &mut client_write);
            let _ = client_write.shutdown(Shutdown::Both);
            let _ = t.join();
            Ok(())
        }
        Upstream::Tls(stream) => {
            stream
                .sock
                .set_read_timeout(Some(Duration::from_millis(20)))?;
            let shared = Arc::new(Mutex::new(stream));
            let writer = shared.clone();
            let mut client_read = client.try_clone()?;
            let t = thread::spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                loop {
                    let n = match client_read.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let out = rewrite(&buf[..n]);
                    let Ok(mut up) = writer.lock() else { break };
                    if up.write_all(&out).and_then(|_| up.flush()).is_err() {
                        break;
                    }
                }
                if let Ok(mut up) = writer.lock() {
                    up.conn.send_close_notify();
                    let _ = up.flush();
                }
            });
            let mut client_write = client;
            let mut buf = [0u8; 16 * 1024];
            loop {
                let read = {
                    let Ok(mut up) = shared.lock() else { break };
                    up.read(&mut buf)
                };
                match read {
                    Ok(0) => break,
                    Ok(n) => {
                        if client_write.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        if t.is_finished() {
                            break;
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
            let _ = client_write.shutdown(Shutdown::Both);
            let _ = t.join();
            Ok(())
        }
    }
}
