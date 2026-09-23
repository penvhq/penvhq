//! HTTP/1.1 messages through the sealed proxy: read one message, swap bytes in
//! its head and body, write it back framed so the other side can read it. Pure
//! over `Read` and `Write`, so the tests need no network.

use std::io::{self, BufRead, Read, Write};

/// A head bigger than this is refused rather than buffered.
pub const MAX_HEAD: usize = 64 * 1024;
/// A body with a length is buffered to rewrite it; one bigger than this is refused.
pub const MAX_BODY: usize = 64 * 1024 * 1024;

/// Byte strings to replace, in order. Every `from` is at least 16 bytes, so one
/// never occurs inside another by chance.
#[derive(Debug, Clone, Default)]
pub struct Swaps(pub Vec<(Vec<u8>, Vec<u8>)>);

impl Swaps {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The longest `from`: how much of a stream must be held back so a match
    /// split across two reads is still found.
    pub fn longest(&self) -> usize {
        self.0.iter().map(|(from, _)| from.len()).max().unwrap_or(0)
    }

    pub fn apply(&self, input: &[u8]) -> Vec<u8> {
        let mut out = input.to_vec();
        for (from, to) in &self.0 {
            if from.is_empty() {
                continue;
            }
            out = replace(&out, from, to);
        }
        out
    }
}

fn replace(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if haystack[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// `GET /v1/charges HTTP/1.1` or `HTTP/1.1 200 OK`.
    pub start: String,
    pub headers: Vec<(String, String)>,
}

impl Head {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn remove(&mut self, name: &str) {
        self.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    }

    pub fn set(&mut self, name: &str, value: String) {
        self.remove(name);
        self.headers.push((name.to_string(), value));
    }

    fn chunked(&self) -> bool {
        self.get("transfer-encoding")
            .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    }

    /// The one length the message declares. Two that disagree, a list, or a
    /// length beside chunked encoding is how requests are smuggled past a
    /// proxy, so each is refused rather than guessed at.
    fn length(&self) -> io::Result<Option<usize>> {
        let mut found: Option<usize> = None;
        for (_, v) in self
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        {
            let n = v
                .trim()
                .parse::<usize>()
                .map_err(|_| bad("a Content-Length that is not one number"))?;
            if found.is_some_and(|f| f != n) {
                return Err(bad("two Content-Length headers that disagree"));
            }
            found = Some(n);
        }
        Ok(found)
    }

    fn framing_conflict(&self) -> io::Result<()> {
        let encodings: Vec<&str> = self
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("transfer-encoding"))
            .map(|(_, v)| v.as_str())
            .collect();
        if encodings.is_empty() {
            return Ok(());
        }
        let joined = encodings.join(",").to_ascii_lowercase();
        let last = joined.rsplit(',').next().unwrap_or("").trim();
        if last != "chunked" {
            return Err(bad("a Transfer-Encoding that does not end in chunked"));
        }
        if self.get("content-length").is_some() {
            return Err(bad("a Content-Length beside Transfer-Encoding"));
        }
        Ok(())
    }

    pub fn status(&self) -> Option<u16> {
        let mut parts = self.start.split_whitespace();
        let version = parts.next()?;
        if !version.starts_with("HTTP/") {
            return None;
        }
        parts.next()?.parse().ok()
    }

    /// Head bytes with every swap applied to the start line and header values.
    pub fn to_bytes(&self, swaps: &Swaps) -> Vec<u8> {
        let mut out = swaps.apply(self.start.as_bytes());
        out.extend_from_slice(b"\r\n");
        for (name, value) in &self.headers {
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(&swaps.apply(value.as_bytes()));
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out
    }
}

fn bad(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("the proxy refused {what}"),
    )
}

/// One head, or `None` when the other side closed cleanly before sending one.
pub fn read_head(reader: &mut impl BufRead) -> io::Result<Option<Head>> {
    let mut lines: Vec<String> = Vec::new();
    let mut total = 0;
    loop {
        let mut line = Vec::new();
        let n = reader
            .take((MAX_HEAD - total + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            return if lines.is_empty() {
                Ok(None)
            } else {
                Err(bad("a head cut short"))
            };
        }
        total += n;
        if total > MAX_HEAD {
            return Err(bad("a head over 64 KiB"));
        }
        let text = String::from_utf8(line).map_err(|_| bad("a head that is not text"))?;
        let text = text.trim_end_matches(['\r', '\n']).to_string();
        if text.is_empty() {
            if lines.is_empty() {
                continue; // stray blank line between messages
            }
            break;
        }
        lines.push(text);
    }
    let start = lines.remove(0);
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| bad("a header line with no colon"))?;
        if name.trim() != name || name.is_empty() {
            return Err(bad("a header name with spaces"));
        }
        headers.push((name.to_string(), value.trim().to_string()));
    }
    Ok(Some(Head { start, headers }))
}

/// How a body ends.
#[derive(Debug, PartialEq, Eq)]
pub enum Framing {
    None,
    Length(usize),
    Chunked,
    /// A response with neither: the body runs to the end of the connection.
    Close,
}

pub fn request_framing(head: &Head) -> io::Result<Framing> {
    head.framing_conflict()?;
    if head.chunked() {
        return Ok(Framing::Chunked);
    }
    Ok(match head.length()? {
        Some(0) | None => Framing::None,
        Some(n) => Framing::Length(n),
    })
}

/// `request_method` is the method of the request this answers: a HEAD response
/// has no body whatever it says.
pub fn response_framing(head: &Head, request_method: &str) -> io::Result<Framing> {
    let status = head
        .status()
        .ok_or_else(|| bad("a response with no status"))?;
    if request_method.eq_ignore_ascii_case("HEAD")
        || (100..200).contains(&status)
        || status == 204
        || status == 304
    {
        return Ok(Framing::None);
    }
    head.framing_conflict()?;
    if head.chunked() {
        return Ok(Framing::Chunked);
    }
    Ok(match head.length()? {
        Some(0) => Framing::None,
        Some(n) => Framing::Length(n),
        None => Framing::Close,
    })
}

/// Copy a body from `reader` to `writer` with `swaps` applied (the head gets
/// `head_swaps`), and write the
/// head first with framing that matches what is sent: `Content-Length` for a
/// body that had one (the new length), chunked for chunked, and the stream as it
/// comes for a body that runs to the close. Chunked bodies stream, holding back
/// only enough bytes to catch a swap split across reads, so a server-sent event
/// stream still arrives as it happens.
pub fn relay(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    mut head: Head,
    framing: Framing,
    head_swaps: &Swaps,
    swaps: &Swaps,
) -> io::Result<()> {
    match framing {
        Framing::None => {
            writer.write_all(&head.to_bytes(head_swaps))?;
        }
        Framing::Length(n) => {
            if n > MAX_BODY {
                return Err(bad("a body over 64 MiB"));
            }
            let mut body = vec![0u8; n];
            reader.read_exact(&mut body)?;
            let body = swaps.apply(&body);
            head.set("Content-Length", body.len().to_string());
            writer.write_all(&head.to_bytes(head_swaps))?;
            writer.write_all(&body)?;
        }
        Framing::Chunked => {
            writer.write_all(&head.to_bytes(head_swaps))?;
            let mut stream = Streamer::new(swaps);
            loop {
                let mut size_line = Vec::new();
                reader.take(1024).read_until(b'\n', &mut size_line)?;
                let text = String::from_utf8_lossy(&size_line);
                let size = text.trim().split(';').next().unwrap_or("");
                let size = usize::from_str_radix(size.trim(), 16)
                    .map_err(|_| bad("a chunk size that is not hex"))?;
                if size == 0 {
                    // Trailers, if any, up to the blank line.
                    loop {
                        let mut line = Vec::new();
                        if reader.take(8192).read_until(b'\n', &mut line)? == 0
                            || line == b"\r\n"
                            || line == b"\n"
                        {
                            break;
                        }
                    }
                    let rest = stream.finish();
                    write_chunk(writer, &rest)?;
                    writer.write_all(b"0\r\n\r\n")?;
                    writer.flush()?;
                    return Ok(());
                }
                if size > MAX_BODY {
                    return Err(bad("a chunk over 64 MiB"));
                }
                let mut chunk = vec![0u8; size];
                reader.read_exact(&mut chunk)?;
                let mut crlf = [0u8; 2];
                reader.read_exact(&mut crlf)?;
                let ready = stream.push(&chunk);
                write_chunk(writer, &ready)?;
                writer.flush()?;
            }
        }
        Framing::Close => {
            writer.write_all(&head.to_bytes(head_swaps))?;
            let mut stream = Streamer::new(swaps);
            let mut buf = [0u8; 16 * 1024];
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                writer.write_all(&stream.push(&buf[..n]))?;
                writer.flush()?;
            }
            writer.write_all(&stream.finish())?;
        }
    }
    writer.flush()
}

fn write_chunk(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    write!(writer, "{:x}\r\n", bytes.len())?;
    writer.write_all(bytes)?;
    writer.write_all(b"\r\n")
}

/// Swaps over a stream: everything but the last `longest - 1` bytes is final
/// once no match can start in it.
pub struct Streamer<'a> {
    swaps: &'a Swaps,
    pending: Vec<u8>,
}

impl<'a> Streamer<'a> {
    pub fn new(swaps: &'a Swaps) -> Streamer<'a> {
        Streamer {
            swaps,
            pending: Vec::new(),
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let keep = self.swaps.longest().saturating_sub(1);
        if self.pending.len() <= keep {
            return Vec::new();
        }
        // Swap across everything, then hold back a tail no swap produced, so a
        // match straddling the cut is seen whole next time.
        let cut = self.pending.len() - keep;
        let (mut head_end, mut out) = (0, Vec::new());
        let mut i = 0;
        while i < cut {
            if let Some((from, to)) = self
                .swaps
                .0
                .iter()
                .find(|(from, _)| !from.is_empty() && self.pending[i..].starts_with(from))
            {
                out.extend_from_slice(to);
                i += from.len();
            } else {
                out.push(self.pending[i]);
                i += 1;
            }
            head_end = i;
        }
        self.pending.drain(..head_end);
        out
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let rest = std::mem::take(&mut self.pending);
        self.swaps.apply(&rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn swaps() -> Swaps {
        Swaps(vec![(
            b"PLACEHOLDER_0123456789ab".to_vec(),
            b"sk_real".to_vec(),
        )])
    }

    fn relay_all(input: &[u8], request: bool) -> String {
        let mut reader = Cursor::new(input.to_vec());
        let head = read_head(&mut reader).unwrap().unwrap();
        let framing = if request {
            request_framing(&head).unwrap()
        } else {
            response_framing(&head, "GET").unwrap()
        };
        let mut out = Vec::new();
        relay(&mut reader, &mut out, head, framing, &swaps(), &swaps()).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_header_and_a_body_with_a_length_are_swapped_and_the_length_follows() {
        let body = "{\"key\":\"PLACEHOLDER_0123456789ab\"}";
        let req = format!(
            "POST /v1 HTTP/1.1\r\nHost: api.test\r\nAuthorization: Bearer PLACEHOLDER_0123456789ab\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let out = relay_all(req.as_bytes(), true);
        assert!(out.contains("Authorization: Bearer sk_real\r\n"), "{out}");
        assert!(out.ends_with("{\"key\":\"sk_real\"}"), "{out}");
        assert!(out.contains("Content-Length: 17\r\n"), "{out}");
        assert!(!out.contains("PLACEHOLDER"));
    }

    #[test]
    fn a_chunked_stream_is_swapped_even_across_chunk_boundaries() {
        let res = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\ndata: \r\nA\r\nPLACEHOLDE\r\n12\r\nR_0123456789ab end\r\n0\r\n\r\n";
        let out = relay_all(res.as_bytes(), false);
        let (_, body) = out.split_once("\r\n\r\n").unwrap();
        let mut joined = String::new();
        let mut rest = body;
        loop {
            let (size, tail) = rest.split_once("\r\n").unwrap();
            let size = usize::from_str_radix(size, 16).unwrap();
            if size == 0 {
                break;
            }
            joined.push_str(&tail[..size]);
            rest = &tail[size + 2..];
        }
        assert_eq!(joined, "data: sk_real end");
    }

    #[test]
    fn a_response_that_runs_to_the_close_is_swapped() {
        let out = relay_all(
            b"HTTP/1.0 200 OK\r\n\r\nx PLACEHOLDER_0123456789ab y",
            false,
        );
        assert!(out.ends_with("x sk_real y"), "{out}");
    }

    #[test]
    fn head_and_no_content_responses_carry_no_body() {
        let head = read_head(&mut Cursor::new(
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n".to_vec(),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(response_framing(&head, "HEAD").unwrap(), Framing::None);
        let head = read_head(&mut Cursor::new(
            b"HTTP/1.1 204 No Content\r\n\r\n".to_vec(),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(response_framing(&head, "GET").unwrap(), Framing::None);
    }

    #[test]
    fn malformed_heads_are_refused() {
        assert!(
            read_head(&mut Cursor::new(
                b"GET / HTTP/1.1\r\nno colon here\r\n\r\n".to_vec()
            ))
            .is_err()
        );
        assert!(
            read_head(&mut Cursor::new(
                b"GET / HTTP/1.1\r\nHost : x\r\n\r\n".to_vec()
            ))
            .is_err()
        );
        let huge = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD + 10));
        assert!(read_head(&mut Cursor::new(huge.into_bytes())).is_err());
        let head = read_head(&mut Cursor::new(
            b"POST / HTTP/1.1\r\nContent-Length: 1x\r\n\r\n".to_vec(),
        ))
        .unwrap()
        .unwrap();
        assert!(request_framing(&head).is_err());
        assert!(read_head(&mut Cursor::new(Vec::new())).unwrap().is_none());
    }

    #[test]
    fn framing_a_proxy_could_be_smuggled_through_is_refused() {
        for head in [
            "POST / HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length: 5, 5\r\n\r\n",
            "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 5\r\n\r\n",
            "POST / HTTP/1.1\r\nTransfer-Encoding: chunked, identity\r\n\r\n",
            "POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n",
        ] {
            let parsed = read_head(&mut Cursor::new(head.as_bytes().to_vec()))
                .unwrap()
                .unwrap();
            assert!(request_framing(&parsed).is_err(), "{head:?}");
        }
        let same = read_head(&mut Cursor::new(
            b"POST / HTTP/1.1\r\nContent-Length: 5\r\ncontent-length: 5\r\n\r\n".to_vec(),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(request_framing(&same).unwrap(), Framing::Length(5));
    }

    #[test]
    fn the_streamer_never_emits_half_a_match() {
        let swaps = swaps();
        let mut s = Streamer::new(&swaps);
        let mut out = Vec::new();
        for byte in b"aPLACEHOLDER_0123456789abz" {
            out.extend(s.push(&[*byte]));
        }
        out.extend(s.finish());
        assert_eq!(out, b"ask_realz");
    }
}
