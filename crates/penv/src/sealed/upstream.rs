//! Connections from the sealed proxies to the real service: TCP, TLS with or
//! without a certificate check, and passing bytes both ways once set up.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use super::http::{Streamer, Swaps};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, Connection, DigitallySignedStruct, RootCertStore,
    SignatureScheme, StreamOwned,
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

impl Upstream {
    pub fn halves(self) -> io::Result<(ReadHalf, WriteHalf)> {
        match self {
            Upstream::Plain(sock) => halves(None, sock),
            Upstream::Tls(stream) => {
                let StreamOwned { conn, sock } = *stream;
                halves(Some(conn.into()), sock)
            }
        }
    }
}

/// TLS state shared by the two halves of one connection.
struct Shared {
    conn: Mutex<Connection>,
    /// Held while encrypted bytes go out, so records leave in the order made.
    sending: Mutex<()>,
}

/// A connection's reading side. Over TLS it waits on the socket holding no
/// lock and takes the shared state only to decrypt what arrived, so the other
/// direction is never held up by a read.
pub struct ReadHalf {
    tls: Option<Arc<Shared>>,
    sock: TcpStream,
    /// Bytes read from the socket and not yet given to TLS.
    backlog: Vec<u8>,
}

/// A connection's writing side.
pub struct WriteHalf {
    tls: Option<Arc<Shared>>,
    sock: TcpStream,
}

/// Split `sock`, carrying `conn` when it is TLS, into halves each direction
/// can block on alone.
pub fn halves(conn: Option<Connection>, sock: TcpStream) -> io::Result<(ReadHalf, WriteHalf)> {
    sock.set_read_timeout(None)?;
    let tls = conn.map(|conn| {
        Arc::new(Shared {
            conn: Mutex::new(conn),
            sending: Mutex::new(()),
        })
    });
    Ok((
        ReadHalf {
            tls: tls.clone(),
            sock: sock.try_clone()?,
            backlog: Vec::new(),
        },
        WriteHalf { tls, sock },
    ))
}

fn poisoned() -> io::Error {
    io::Error::other("a sealed connection's lock was poisoned")
}

impl Shared {
    fn lock(&self) -> io::Result<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| poisoned())
    }

    /// Send what `conn` has encrypted, letting go of it before the socket can block.
    fn send(&self, mut conn: MutexGuard<'_, Connection>, sock: &mut TcpStream) -> io::Result<()> {
        let mut out = Vec::new();
        while conn.wants_write() {
            conn.write_tls(&mut out)?;
        }
        if out.is_empty() {
            return Ok(());
        }
        let _order = self.sending.lock().map_err(|_| poisoned())?;
        drop(conn);
        sock.write_all(&out)
    }
}

impl Read for ReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(tls) = self.tls.clone() else {
            return self.sock.read(buf);
        };
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let mut conn = tls.lock()?;
            match conn.reader().read(buf) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                other => return other,
            }
            if !self.backlog.is_empty() {
                // Feed a record at a time, stopping once there is plaintext, so
                // rustls's buffer of it never overfills.
                let mut fed = &self.backlog[..];
                let mut state = Ok(());
                while !fed.is_empty() {
                    conn.read_tls(&mut fed)?;
                    match conn.process_new_packets() {
                        Ok(io) if io.plaintext_bytes_to_read() > 0 => break,
                        Ok(_) => {}
                        Err(e) => {
                            state = Err(io::Error::new(io::ErrorKind::InvalidData, e));
                            break;
                        }
                    }
                }
                let used = self.backlog.len() - fed.len();
                self.backlog.drain(..used);
                // Alerts and key updates the records called for.
                tls.send(conn, &mut self.sock)?;
                state?;
                continue;
            }
            drop(conn);
            let mut raw = [0u8; 16 * 1024];
            let n = self.sock.read(&mut raw)?;
            if n == 0 {
                let mut conn = tls.lock()?;
                conn.read_tls(&mut io::empty())?;
                conn.process_new_packets()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                continue;
            }
            self.backlog.extend_from_slice(&raw[..n]);
        }
    }
}

impl Write for WriteHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(tls) = &self.tls else {
            return self.sock.write(buf);
        };
        let mut conn = tls.lock()?;
        let n = conn.writer().write(buf)?;
        tls.send(conn, &mut self.sock)?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        match &self.tls {
            None => self.sock.flush(),
            Some(tls) => tls.send(tls.lock()?, &mut self.sock),
        }
    }
}

impl WriteHalf {
    /// No more is coming this way: close_notify over TLS, then a half-close.
    pub fn close(&mut self) {
        if let Some(tls) = &self.tls
            && let Ok(mut conn) = tls.lock()
        {
            conn.send_close_notify();
            let _ = tls.send(conn, &mut self.sock);
        }
        let _ = self.sock.shutdown(Shutdown::Write);
    }

    /// End the connection both ways, which also wakes its read half.
    pub fn end(&mut self) {
        self.close();
        let _ = self.sock.shutdown(Shutdown::Both);
    }
}

/// One direction of a stream, rewritten: `push` each read as it arrives, keeping
/// whatever state makes a message split across reads come out whole, and
/// `finish` at the end.
pub trait Rewrite: Send + 'static {
    fn push(&mut self, bytes: &[u8]) -> Vec<u8>;
    fn finish(&mut self) -> Vec<u8> {
        Vec::new()
    }
}

impl Rewrite for Streamer {
    fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        Streamer::push(self, bytes)
    }
    fn finish(&mut self) -> Vec<u8> {
        Streamer::finish(self)
    }
}

/// Bytes both ways until either side closes.
pub fn pipe(client: TcpStream, up: Upstream) -> io::Result<()> {
    pipe_with(
        client,
        up,
        Streamer::new(Swaps::default()),
        Streamer::new(Swaps::default()),
    )
}

/// `pipe`, with what the client sends passed through `rewrite` on its way up
/// and what comes back through `down`.
pub fn pipe_with(
    client: TcpStream,
    up: Upstream,
    mut rewrite: impl Rewrite,
    mut down: impl Rewrite,
) -> io::Result<()> {
    let (mut up_read, mut up_write) = up.halves()?;
    let (mut client_read, mut client_write) = halves(None, client)?;
    let t = thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = match client_read.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if up_write.write_all(&rewrite.push(&buf[..n])).is_err() {
                break;
            }
        }
        up_write.close();
    });
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = match up_read.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if client_write.write_all(&down.push(&buf[..n])).is_err() {
            break;
        }
    }
    let _ = client_write.write_all(&down.finish());
    client_write.end();
    let _ = t.join();
    Ok(())
}
