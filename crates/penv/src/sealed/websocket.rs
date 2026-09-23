//! A WebSocket through an allowed host. The handshake is an HTTP request, so
//! the value went in with it; after the switch, the command's frames pass as
//! they are (a placeholder in a message stays one, like a request body) and the
//! server's text and binary frames have every value swapped back to its
//! placeholder before the command reads them. Compression is refused in the
//! handshake so frames can be read.

use std::io::{self, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rustls::{ClientConnection, ServerConnection, StreamOwned};

use super::http::Swaps;

type Client = StreamOwned<ServerConnection, TcpStream>;
type Upstream = StreamOwned<ClientConnection, TcpStream>;

/// One frame the server sent: header fields and the unmasked payload.
struct Frame {
    fin_rsv_opcode: u8,
    payload: Vec<u8>,
}

fn read_frame(r: &mut impl Read) -> io::Result<Frame> {
    let mut head = [0u8; 2];
    r.read_exact(&mut head)?;
    let masked = head[1] & 0x80 != 0;
    let mut len = u64::from(head[1] & 0x7f);
    if len == 126 {
        let mut b = [0u8; 2];
        r.read_exact(&mut b)?;
        len = u64::from(u16::from_be_bytes(b));
    } else if len == 127 {
        let mut b = [0u8; 8];
        r.read_exact(&mut b)?;
        len = u64::from_be_bytes(b);
    }
    if len > 64 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a WebSocket frame over 64 MiB",
        ));
    }
    let mut mask = [0u8; 4];
    if masked {
        r.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok(Frame {
        fin_rsv_opcode: head[0],
        payload,
    })
}

/// A server frame, unmasked, as servers send them.
fn write_frame(w: &mut impl Write, frame: &Frame) -> io::Result<()> {
    let mut out = vec![frame.fin_rsv_opcode];
    let len = frame.payload.len();
    if len < 126 {
        out.push(len as u8);
    } else if len <= u16::MAX as usize {
        out.push(126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(&frame.payload);
    w.write_all(&out)?;
    w.flush()
}

/// Server frames with values swapped back; control frames as they are.
pub fn rewrite_frame(frame: &mut Vec<u8>, opcode: u8, swaps: &Swaps) {
    // 0x0 continuation, 0x1 text, 0x2 binary.
    if opcode <= 0x2 && !swaps.is_empty() {
        *frame = swaps.apply(frame);
    }
}

fn locked_read<S: Read>(stream: &Mutex<S>, buf: &mut [u8]) -> io::Result<usize> {
    let mut s = stream.lock().map_err(|_| io::Error::other("poisoned"))?;
    s.read(buf)
}

/// Pass the upgraded connection both ways until either side closes. Each TLS
/// stream is one object shared by the two directions, read under a short
/// socket timeout so the other direction's writes are never held up long.
pub fn splice(
    from_client: BufReader<Client>,
    from_upstream: BufReader<Upstream>,
    swaps: Swaps,
) -> io::Result<()> {
    let client_early = from_client.buffer().to_vec();
    let upstream_early = from_upstream.buffer().to_vec();
    let client = from_client.into_inner();
    let upstream = from_upstream.into_inner();
    client
        .sock
        .set_read_timeout(Some(Duration::from_millis(20)))?;
    upstream
        .sock
        .set_read_timeout(Some(Duration::from_millis(20)))?;
    let client = Arc::new(Mutex::new(client));
    let upstream = Arc::new(Mutex::new(upstream));

    // Command to server: bytes as they are.
    let (c, u) = (client.clone(), upstream.clone());
    let up = thread::spawn(move || {
        if !client_early.is_empty() && u.lock().map(|mut s| s.write_all(&client_early)).is_err() {
            return;
        }
        let mut buf = [0u8; 16 * 1024];
        loop {
            match locked_read(&c, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let Ok(mut s) = u.lock() else { break };
                    if s.write_all(&buf[..n]).and_then(|_| s.flush()).is_err() {
                        break;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => break,
            }
        }
    });

    // Server to command: frame by frame, swapped. A reader that waits out the
    // short timeouts turns the shared stream back into a blocking one.
    struct Patient {
        early: Vec<u8>,
        stream: Arc<Mutex<Upstream>>,
        done: Arc<std::sync::atomic::AtomicBool>,
    }
    impl Read for Patient {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !self.early.is_empty() {
                let n = buf.len().min(self.early.len());
                buf[..n].copy_from_slice(&self.early[..n]);
                self.early.drain(..n);
                return Ok(n);
            }
            loop {
                match locked_read(&self.stream, buf) {
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        if self.done.load(std::sync::atomic::Ordering::Relaxed) {
                            return Ok(0);
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                    other => return other,
                }
            }
        }
    }
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut reader = Patient {
        early: upstream_early,
        stream: upstream.clone(),
        done: done.clone(),
    };
    let watcher = {
        let done = done.clone();
        thread::spawn(move || {
            let _ = up.join();
            done.store(true, std::sync::atomic::Ordering::Relaxed);
        })
    };
    // A message split into fragments is joined and sent as one frame, which
    // RFC 6455 lets an intermediary do when no extension is in use, so a value
    // split across fragments is still swapped.
    let mut message: Option<Frame> = None;
    while let Ok(frame) = read_frame(&mut reader) {
        // RSV bits mean an extension, such as compression, the handshake did not
        // agree to: unreadable, so the connection ends.
        if frame.fin_rsv_opcode & 0x70 != 0 {
            break;
        }
        let fin = frame.fin_rsv_opcode & 0x80 != 0;
        let opcode = frame.fin_rsv_opcode & 0x0f;
        let out = match (opcode, message.as_mut()) {
            (0x8..=0xf, _) => Some(frame),
            (0x0, Some(open)) => {
                open.payload.extend_from_slice(&frame.payload);
                if open.payload.len() > 64 * 1024 * 1024 {
                    break;
                }
                if fin { message.take() } else { None }
            }
            (0x0, None) => break,
            (_, _) if fin => Some(frame),
            (_, _) => {
                message = Some(frame);
                None
            }
        };
        let Some(mut frame) = out else { continue };
        let opcode = frame.fin_rsv_opcode & 0x0f;
        frame.fin_rsv_opcode |= 0x80;
        rewrite_frame(&mut frame.payload, opcode, &swaps);
        let Ok(mut c) = client.lock() else { break };
        if write_frame(&mut *c, &frame).is_err() || opcode == 0x8 {
            break;
        }
    }
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut c) = client.lock() {
        c.conn.send_close_notify();
        let _ = c.flush();
        let _ = c.sock.shutdown(std::net::Shutdown::Both);
    }
    let _ = watcher.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_server_frame_round_trips_and_its_text_is_swapped() {
        let swaps = Swaps(vec![(
            b"sk_real_value".to_vec(),
            b"PLACEHOLDER_xyz".to_vec(),
        )]);
        for len in [5usize, 200, 70_000] {
            let mut text = vec![b'a'; len];
            text.extend_from_slice(b" sk_real_value");
            let mut wire = Vec::new();
            write_frame(
                &mut wire,
                &Frame {
                    fin_rsv_opcode: 0x81,
                    payload: text.clone(),
                },
            )
            .unwrap();
            let mut frame = read_frame(&mut Cursor::new(wire)).unwrap();
            assert_eq!(frame.payload, text);
            rewrite_frame(&mut frame.payload, frame.fin_rsv_opcode & 0x0f, &swaps);
            assert!(frame.payload.ends_with(b" PLACEHOLDER_xyz"));
        }
        let mut ping = b"sk_real_value".to_vec();
        rewrite_frame(&mut ping, 0x9, &swaps);
        assert_eq!(ping, b"sk_real_value", "control frames pass as they are");
    }

    #[test]
    fn a_masked_frame_is_unmasked() {
        let wire = [0x81u8, 0x83, 1, 2, 3, 4, b'a' ^ 1, b'b' ^ 2, b'c' ^ 3];
        let frame = read_frame(&mut Cursor::new(wire.to_vec())).unwrap();
        assert_eq!(frame.payload, b"abc");
    }
}
