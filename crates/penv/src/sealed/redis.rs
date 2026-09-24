//! A sealed Redis URL. The command connects to a loopback port and
//! authenticates with the placeholder; penv swaps the real password into `AUTH`
//! and `HELLO ... AUTH` on the way to the server. Any other command carries the
//! placeholder as it is, so it can never be stored or echoed as the value, and
//! a reply that holds the password carries the placeholder back instead.

use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use rustls::ClientConfig;

use super::http::{Streamer, Swaps};
use super::upstream::{self, Rewrite, Upstream};

#[derive(Clone)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub password: String,
    pub placeholder: String,
    /// `rediss://`: TLS with the certificate checked.
    pub tls: Option<Arc<ClientConfig>>,
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

fn serve(client: TcpStream, target: &Target) -> io::Result<()> {
    let tcp = upstream::tcp(&target.host, target.port)?;
    let up = match &target.tls {
        Some(config) => upstream::tls(tcp, &target.host, config.clone())?,
        None => Upstream::Plain(tcp),
    };
    let rewriter = Rewriter::new(
        target.placeholder.clone().into_bytes(),
        target.password.clone().into_bytes(),
    );
    let replies = Replies::new(Swaps::new(vec![(
        target.password.clone().into_bytes(),
        target.placeholder.clone().into_bytes(),
    )]));
    upstream::pipe_with(client, up, rewriter, replies)
}

/// Client-to-server RESP, rewritten one whole command at a time.
pub struct Rewriter {
    placeholder: Vec<u8>,
    password: Vec<u8>,
    pending: Vec<u8>,
}

impl Rewrite for Rewriter {
    fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        Rewriter::push(self, bytes)
    }
}

impl Rewriter {
    pub fn new(placeholder: Vec<u8>, password: Vec<u8>) -> Rewriter {
        Rewriter {
            placeholder,
            password,
            pending: Vec::new(),
        }
    }

    /// Every complete command in what has arrived, rewritten; a partial one waits.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            match parse(&self.pending) {
                Parsed::Command(args, used) => {
                    out.extend(self.encode(args));
                    self.pending.drain(..used);
                }
                Parsed::Incomplete => break,
                Parsed::Invalid => {
                    // Not RESP penv can read: pass it on as it came. The server
                    // answers with an error; nothing here is a value.
                    out.append(&mut self.pending);
                    break;
                }
            }
        }
        out
    }

    fn encode(&self, mut args: Vec<Vec<u8>>) -> Vec<u8> {
        let command = args
            .first()
            .map(|a| a.to_ascii_uppercase())
            .unwrap_or_default();
        let swap = |arg: &mut Vec<u8>| {
            if *arg == self.placeholder {
                *arg = self.password.clone();
            }
        };
        match command.as_slice() {
            b"AUTH" => args.iter_mut().skip(1).for_each(swap),
            b"HELLO" => {
                if let Some(at) = args.iter().position(|a| a.eq_ignore_ascii_case(b"AUTH")) {
                    args.iter_mut().skip(at + 2).take(1).for_each(swap);
                }
            }
            _ => {}
        }
        let mut out = format!("*{}\r\n", args.len()).into_bytes();
        for arg in args {
            out.extend(format!("${}\r\n", arg.len()).into_bytes());
            out.extend(arg);
            out.extend(b"\r\n");
        }
        out
    }
}

/// Server-to-client RESP with the password swapped back to the placeholder,
/// one whole reply element at a time so a string's declared length follows it.
pub struct Replies {
    swaps: Swaps,
    pending: Vec<u8>,
    /// Set once the stream stops parsing as RESP: swapped byte by byte from then on.
    raw: Option<Streamer>,
}

impl Replies {
    pub fn new(swaps: Swaps) -> Replies {
        Replies {
            swaps,
            pending: Vec::new(),
            raw: None,
        }
    }

    fn go_raw(&mut self, out: &mut Vec<u8>, at: usize) {
        let mut stream = Streamer::new(self.swaps.clone());
        out.extend(stream.push(&self.pending[at..]));
        self.pending.clear();
        self.raw = Some(stream);
    }
}

impl Rewrite for Replies {
    fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        if let Some(raw) = self.raw.as_mut() {
            return raw.push(bytes);
        }
        self.pending.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut at = 0;
        while at < self.pending.len() {
            let Some((text, next)) = line(&self.pending, at) else {
                if self.pending.len() - at > 64 * 1024 {
                    self.go_raw(&mut out, at);
                    return out;
                }
                break;
            };
            // Blob strings, blob errors, verbatim strings and streamed parts
            // carry a length; a number there means that many bytes follow.
            let len = match text.first() {
                Some(b'$' | b'!' | b'=' | b';') => std::str::from_utf8(&text[1..])
                    .ok()
                    .and_then(|l| l.parse::<usize>().ok()),
                _ => None,
            };
            let Some(len) = len else {
                out.extend(self.swaps.apply(text));
                out.extend(b"\r\n");
                at = next;
                continue;
            };
            if len > 512 << 20 {
                self.go_raw(&mut out, at);
                return out;
            }
            if self.pending.len() < next + len + 2 {
                break;
            }
            if &self.pending[next + len..next + len + 2] != b"\r\n" {
                self.go_raw(&mut out, at);
                return out;
            }
            let body = self.swaps.apply(&self.pending[next..next + len]);
            out.push(text[0]);
            out.extend(body.len().to_string().into_bytes());
            out.extend(b"\r\n");
            out.extend(body);
            out.extend(b"\r\n");
            at = next + len + 2;
        }
        self.pending.drain(..at);
        out
    }

    fn finish(&mut self) -> Vec<u8> {
        match self.raw.as_mut() {
            Some(raw) => raw.finish(),
            None => self.swaps.apply(&std::mem::take(&mut self.pending)),
        }
    }
}

enum Parsed {
    Command(Vec<Vec<u8>>, usize),
    Incomplete,
    Invalid,
}

fn line(buf: &[u8], from: usize) -> Option<(&[u8], usize)> {
    let end = buf[from..].windows(2).position(|w| w == b"\r\n")? + from;
    Some((&buf[from..end], end + 2))
}

fn parse(buf: &[u8]) -> Parsed {
    if buf.is_empty() {
        return Parsed::Incomplete;
    }
    if buf[0] != b'*' {
        // An inline command: words up to the end of the line.
        return match line(buf, 0) {
            Some((text, used)) => Parsed::Command(
                text.split(|b| *b == b' ')
                    .filter(|w| !w.is_empty())
                    .map(<[u8]>::to_vec)
                    .collect(),
                used,
            ),
            None if buf.len() > 64 * 1024 => Parsed::Invalid,
            None => Parsed::Incomplete,
        };
    }
    let Some((count, mut at)) = line(buf, 1) else {
        return if buf.len() > 64 {
            Parsed::Invalid
        } else {
            Parsed::Incomplete
        };
    };
    let Some(count) = std::str::from_utf8(count)
        .ok()
        .and_then(|c| c.parse::<usize>().ok())
        .filter(|c| *c <= 1 << 20)
    else {
        return Parsed::Invalid;
    };
    // The count is the peer's word; the list grows as the arguments arrive.
    let mut args = Vec::with_capacity(count.min(16));
    for _ in 0..count {
        if at >= buf.len() {
            return Parsed::Incomplete;
        }
        if buf[at] != b'$' {
            return Parsed::Invalid;
        }
        let Some((len, next)) = line(buf, at + 1) else {
            return Parsed::Incomplete;
        };
        let Some(len) = std::str::from_utf8(len)
            .ok()
            .and_then(|l| l.parse::<usize>().ok())
            .filter(|l| *l <= 512 << 20)
        else {
            return Parsed::Invalid;
        };
        if buf.len() < next + len + 2 {
            return Parsed::Incomplete;
        }
        args.push(buf[next..next + len].to_vec());
        at = next + len + 2;
    }
    Parsed::Command(args, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(args: &[&str]) -> Vec<u8> {
        let mut out = format!("*{}\r\n", args.len()).into_bytes();
        for a in args {
            out.extend(format!("${}\r\n{a}\r\n", a.len()).into_bytes());
        }
        out
    }

    #[test]
    fn auth_and_hello_get_the_password_and_nothing_else_does() {
        let mut r = Rewriter::new(b"PH".to_vec(), b"secret".to_vec());
        assert_eq!(r.push(&resp(&["AUTH", "PH"])), resp(&["AUTH", "secret"]));
        assert_eq!(
            r.push(&resp(&["auth", "default", "PH"])),
            resp(&["auth", "default", "secret"])
        );
        assert_eq!(
            r.push(&resp(&["HELLO", "3", "AUTH", "default", "PH"])),
            resp(&["HELLO", "3", "AUTH", "default", "secret"])
        );
        assert_eq!(
            r.push(&resp(&["SET", "k", "PH"])),
            resp(&["SET", "k", "PH"]),
            "a value stays a placeholder"
        );
        assert_eq!(
            r.push(b"AUTH PH\r\n"),
            resp(&["AUTH", "secret"]),
            "inline commands too"
        );
    }

    #[test]
    fn a_command_split_across_reads_waits_for_the_rest() {
        let mut r = Rewriter::new(b"PH".to_vec(), b"secret".to_vec());
        let whole = resp(&["AUTH", "PH"]);
        let (a, b) = whole.split_at(7);
        assert!(r.push(a).is_empty());
        assert_eq!(r.push(b), resp(&["AUTH", "secret"]));
        let two = [resp(&["PING"]), resp(&["AUTH", "PH"])].concat();
        assert_eq!(
            r.push(&two),
            [resp(&["PING"]), resp(&["AUTH", "secret"])].concat()
        );
    }

    fn replies() -> Replies {
        Replies::new(Swaps::new(vec![(
            b"fake-redis-pw".to_vec(),
            b"penvphPLACEHOLDER".to_vec(),
        )]))
    }

    #[test]
    fn a_reply_holding_the_password_carries_the_placeholder_and_its_length_follows() {
        let mut r = replies();
        let reply = b"*3\r\n$4\r\nuser\r\n$18\r\nfake-redis-pw:rest\r\n-ERR bad fake-redis-pw\r\n";
        let (a, b) = reply.split_at(20);
        let mut out = r.push(a);
        out.extend(r.push(b));
        out.extend(r.finish());
        assert_eq!(
            out,
            b"*3\r\n$4\r\nuser\r\n$22\r\npenvphPLACEHOLDER:rest\r\n-ERR bad penvphPLACEHOLDER\r\n"
        );
        assert_eq!(r.push(b"+OK\r\n"), b"+OK\r\n", "nothing held back");
        assert_eq!(r.push(b"$-1\r\n:5\r\n"), b"$-1\r\n:5\r\n");
    }

    #[test]
    fn replies_that_stop_being_resp_are_still_swapped() {
        let mut r = replies();
        let mut out = r.push(b"$3\r\nabcXYfake-redis");
        out.extend(r.push(b"-pw"));
        out.extend(r.finish());
        assert!(!out.windows(13).any(|w| w == b"fake-redis-pw"));
    }
}
