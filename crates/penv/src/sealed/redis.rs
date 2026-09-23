//! A sealed Redis URL. The command connects to a loopback port and
//! authenticates with the placeholder; penv swaps the real password into `AUTH`
//! and `HELLO ... AUTH` on the way to the server. Any other command carries the
//! placeholder as it is, so it can never be stored or echoed as the value.

use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use rustls::ClientConfig;

use super::upstream::{self, Upstream};

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

fn serve(client: TcpStream, target: &Target) -> io::Result<()> {
    let tcp = upstream::tcp(&target.host, target.port)?;
    let up = match &target.tls {
        Some(config) => upstream::tls(tcp, &target.host, config.clone())?,
        None => Upstream::Plain(tcp),
    };
    let mut rewriter = Rewriter::new(
        target.placeholder.clone().into_bytes(),
        target.password.clone().into_bytes(),
    );
    upstream::pipe_with(client, up, move |bytes| rewriter.push(bytes))
}

/// Client-to-server RESP, rewritten one whole command at a time.
pub struct Rewriter {
    placeholder: Vec<u8>,
    password: Vec<u8>,
    pending: Vec<u8>,
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
    let mut args = Vec::with_capacity(count);
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
}
