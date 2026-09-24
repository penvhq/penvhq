//! A sealed run: the child holds placeholders for keys with `@hosts`, and the
//! values go into its requests on the way out, through a proxy in this process.

pub mod ca;
pub mod http;
pub mod postgres;
pub mod proxy;
pub mod redis;
pub mod run;
pub mod scram;
pub mod sigv4;
pub mod upstream;
pub mod url;
pub mod websocket;

use std::io::{self, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Connections one sealed listener serves at once; one more is closed unanswered.
pub const MAX_CONNECTIONS: usize = 256;

/// A count of connections in service, shared by a listener's threads.
pub struct Slots {
    used: AtomicUsize,
    max: usize,
}

/// One connection's place in `Slots`, given back when it is dropped.
pub struct Slot(Arc<Slots>);

impl Slots {
    pub fn new(max: usize) -> Arc<Slots> {
        Arc::new(Slots {
            used: AtomicUsize::new(0),
            max,
        })
    }

    pub fn take(self: &Arc<Self>) -> Option<Slot> {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.max).then_some(n + 1)
            })
            .ok()
            .map(|_| Slot(self.clone()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.used.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Accept on `listener` until the process ends, each connection on its own
/// thread, at most `MAX_CONNECTIONS` at once.
pub fn serve_each(listener: TcpListener, serve: impl Fn(TcpStream) + Send + Sync + 'static) {
    let serve = Arc::new(serve);
    let slots = Slots::new(MAX_CONNECTIONS);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let Some(slot) = slots.take() else {
                continue;
            };
            let serve = serve.clone();
            std::thread::spawn(move || {
                let _slot = slot;
                serve(stream);
            });
        }
    });
}

/// Exactly `n` bytes, the buffer growing as they arrive rather than sized by
/// what the peer declared.
pub fn read_len(reader: &mut impl Read, n: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    reader.take(n as u64).read_to_end(&mut out)?;
    if out.len() < n {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(out)
}

/// An AWS secret access key, which a sealed run keeps and signs with.
pub fn is_aws_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper == "AWS_SECRET_ACCESS_KEY" || upper.ends_with("_AWS_SECRET_ACCESS_KEY")
}

/// Why a key looks like it signs requests rather than sending itself, from its
/// name. A placeholder cannot stand in for such a key: the signature is made
/// in the command, with whatever value it holds.
pub fn signing_secret(name: &str) -> Option<&'static str> {
    let upper = name.to_ascii_uppercase();
    if is_aws_secret(&upper) {
        // SigV4: the proxy signs each request again with the real key.
        return None;
    }
    for (word, why) in [
        ("SIGNING", "its name says it signs"),
        ("HMAC", "its name says it computes HMACs"),
        (
            "WEBHOOK_SECRET",
            "a webhook secret verifies or signs payloads",
        ),
        ("JWT_SECRET", "a JWT secret signs tokens"),
        ("JWT_PRIVATE_KEY", "a JWT key signs tokens"),
    ] {
        if upper.contains(word) {
            return Some(why);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_refuse_beyond_the_cap_and_come_back_when_dropped() {
        let slots = Slots::new(2);
        let a = slots.take().unwrap();
        let _b = slots.take().unwrap();
        assert!(slots.take().is_none());
        drop(a);
        assert!(slots.take().is_some());
    }

    #[test]
    fn a_declared_length_the_peer_never_sends_is_an_error() {
        let mut short = io::Cursor::new(vec![1u8; 10]);
        assert_eq!(
            read_len(&mut short, 1 << 30).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(
            read_len(&mut io::Cursor::new(b"abc".to_vec()), 2).unwrap(),
            b"ab"
        );
    }
}
