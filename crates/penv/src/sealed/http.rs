//! HTTP/1.1 messages through the sealed proxy: read one message, swap bytes in
//! its head and body, write it back framed so the other side can read it. Pure
//! over `Read` and `Write`, so the tests need no network.

use std::io::{self, BufRead, Read, Write};

use super::read_len;

/// A head bigger than this is refused rather than buffered.
pub const MAX_HEAD: usize = 64 * 1024;
/// A body that must be held whole (to hash it for a signature, or for a client
/// that cannot take chunked encoding) is refused over this.
pub const MAX_BODY: usize = 64 * 1024 * 1024;
/// A body with a length and swaps to apply is held whole up to this; a longer
/// one streams, re-framed as chunked.
pub const BUFFER: usize = 1024 * 1024;

/// Byte strings to replace, found in one pass: at each position the longest
/// `from` that matches wins, and what a swap writes is never swapped again.
#[derive(Debug, Clone)]
pub struct Swaps {
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
    /// Indices into `pairs` by the first byte of `from`, longest first.
    by_first: Vec<Vec<usize>>,
}

enum Match<'a> {
    Found(&'a [u8], &'a [u8]),
    /// A `from` could still match once more bytes arrive.
    Maybe,
    None,
}

impl Default for Swaps {
    fn default() -> Swaps {
        Swaps::new(Vec::new())
    }
}

impl Swaps {
    pub fn new(pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Swaps {
        let mut kept: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (from, to) in pairs {
            if !from.is_empty() && !kept.iter().any(|(f, _)| *f == from) {
                kept.push((from, to));
            }
        }
        kept.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
        let mut by_first = vec![Vec::new(); 256];
        for (i, (from, _)) in kept.iter().enumerate() {
            by_first[from[0] as usize].push(i);
        }
        Swaps {
            pairs: kept,
            by_first,
        }
    }

    /// These swaps and `more`; a `from` already here keeps its `to`.
    pub fn with(&self, more: Vec<(Vec<u8>, Vec<u8>)>) -> Swaps {
        let mut pairs = self.pairs.clone();
        pairs.extend(more);
        Swaps::new(pairs)
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    fn at<'a>(&'a self, bytes: &[u8], more_coming: bool) -> Match<'a> {
        let Some(first) = bytes.first() else {
            return Match::None;
        };
        for &i in &self.by_first[*first as usize] {
            let (from, to) = &self.pairs[i];
            if bytes.starts_with(from) {
                return Match::Found(from, to);
            }
            if more_coming && bytes.len() < from.len() && from.starts_with(bytes) {
                return Match::Maybe;
            }
        }
        Match::None
    }

    pub fn apply(&self, input: &[u8]) -> Vec<u8> {
        if self.is_empty() {
            return input.to_vec();
        }
        let mut out = Vec::with_capacity(input.len());
        let mut i = 0;
        while i < input.len() {
            match self.at(&input[i..], false) {
                Match::Found(from, to) => {
                    out.extend_from_slice(to);
                    i += from.len();
                }
                _ => {
                    out.push(input[i]);
                    i += 1;
                }
            }
        }
        out
    }
}

/// A message head. Each char of `start` and of a header value stands for one
/// byte as it came (Latin-1), so a value that is not UTF-8 passes unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// `GET /v1/charges HTTP/1.1` or `HTTP/1.1 200 OK`.
    pub start: String,
    pub headers: Vec<(String, String)>,
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// The bytes a head's text stands for: one per char, as `read_head` made it.
pub fn raw(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        match u8::try_from(u32::from(c)) {
            Ok(b) => out.push(b),
            Err(_) => out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
        }
    }
    out
}

impl Head {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.headers
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
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
        for v in self.all("content-length") {
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
        let encodings: Vec<&str> = self.all("transfer-encoding").collect();
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

    /// This head with every swap applied, so what is signed is what is sent.
    pub fn swapped(&self, swaps: &Swaps) -> Head {
        let text = |t: &str| latin1(&swaps.apply(&raw(t)));
        Head {
            start: text(&self.start),
            headers: self
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), text(v)))
                .collect(),
        }
    }

    /// Head bytes with every swap applied to the start line and header values.
    pub fn to_bytes(&self, swaps: &Swaps) -> Vec<u8> {
        let mut out = swaps.apply(&raw(&self.start));
        out.extend_from_slice(b"\r\n");
        for (name, value) in &self.headers {
            out.extend_from_slice(&raw(name));
            out.extend_from_slice(b": ");
            out.extend_from_slice(&swaps.apply(&raw(value)));
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
        if line.pop() != Some(b'\n') {
            return Err(bad("a head cut short"));
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        // A bare CR or other control byte is read one way here and another
        // upstream: the start of a smuggled request.
        if line.iter().any(|&b| (b < 0x20 && b != b'\t') || b == 0x7f) {
            return Err(bad("a control character in a head"));
        }
        if line.is_empty() {
            if lines.is_empty() {
                continue; // stray blank line between messages
            }
            break;
        }
        lines.push(latin1(&line));
    }
    let start = lines.remove(0);
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| bad("a header line with no colon"))?;
        if name.is_empty() || name.contains([' ', '\t']) {
            return Err(bad("a header name with spaces"));
        }
        headers.push((
            name.to_string(),
            value.trim_matches([' ', '\t']).to_string(),
        ));
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

/// A body that must be read for values but is encoded (gzip, br) is refused:
/// relayed as it is, a value inside would reach the command.
pub fn readable(head: &Head, framing: &Framing) -> io::Result<()> {
    if *framing == Framing::None {
        return Ok(());
    }
    for value in head.all("content-encoding") {
        for coding in value.split(',').map(|c| c.trim().to_ascii_lowercase()) {
            if !coding.is_empty() && coding != "identity" {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "the proxy refused a response in Content-Encoding {coding}, which it cannot read for values to swap back; the request asked for identity"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Copy a body from `reader` to `writer` with `swaps` applied (the head gets
/// `head_swaps`), and write the head first with framing that matches what is
/// sent. A body with a length and nothing to swap streams as it is; with swaps
/// it is held whole up to `BUFFER`, and past that streams re-framed as chunked
/// when `chunked_ok` (the reader of `writer` speaks HTTP/1.1). Chunked bodies
/// stream, holding back only a tail that could still become a swap, so a
/// server-sent event stream still arrives as it happens.
pub fn relay(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    mut head: Head,
    framing: Framing,
    head_swaps: &Swaps,
    swaps: &Swaps,
    chunked_ok: bool,
) -> io::Result<()> {
    match framing {
        Framing::None => {
            writer.write_all(&head.to_bytes(head_swaps))?;
        }
        Framing::Length(n) if swaps.is_empty() => {
            writer.write_all(&head.to_bytes(head_swaps))?;
            if io::copy(&mut reader.take(n as u64), writer)? < n as u64 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }
        Framing::Length(n) if n > BUFFER && chunked_ok => {
            head.remove("content-length");
            head.set("Transfer-Encoding", "chunked".to_string());
            writer.write_all(&head.to_bytes(head_swaps))?;
            let mut stream = Streamer::new(swaps.clone());
            let mut body = reader.take(n as u64);
            let mut buf = [0u8; 16 * 1024];
            let mut seen = 0;
            loop {
                let k = body.read(&mut buf)?;
                if k == 0 {
                    break;
                }
                seen += k;
                write_chunk(writer, &stream.push(&buf[..k]))?;
            }
            if seen < n {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            write_chunk(writer, &stream.finish())?;
            writer.write_all(b"0\r\n\r\n")?;
        }
        Framing::Length(n) => {
            if n > MAX_BODY {
                return Err(bad(
                    "a body over 64 MiB for a client without chunked encoding",
                ));
            }
            let body = swaps.apply(&read_len(reader, n)?);
            head.set("Content-Length", body.len().to_string());
            writer.write_all(&head.to_bytes(head_swaps))?;
            writer.write_all(&body)?;
        }
        Framing::Chunked => {
            writer.write_all(&head.to_bytes(head_swaps))?;
            let mut stream = Streamer::new(swaps.clone());
            let mut buf = [0u8; 16 * 1024];
            loop {
                let mut size_line = Vec::new();
                reader.take(1024).read_until(b'\n', &mut size_line)?;
                if !size_line.ends_with(b"\n") {
                    return Err(bad("a chunk size line cut short"));
                }
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
                let mut chunk = reader.take(size as u64);
                let mut seen = 0;
                loop {
                    let k = chunk.read(&mut buf)?;
                    if k == 0 {
                        break;
                    }
                    seen += k;
                    write_chunk(writer, &stream.push(&buf[..k]))?;
                }
                if seen < size {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                let mut crlf = [0u8; 2];
                reader.read_exact(&mut crlf)?;
                if &crlf != b"\r\n" {
                    return Err(bad("a chunk that does not end in CRLF"));
                }
                writer.flush()?;
            }
        }
        Framing::Close => {
            writer.write_all(&head.to_bytes(head_swaps))?;
            let mut stream = Streamer::new(swaps.clone());
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

/// Swaps over a stream. Everything is final as it arrives except a tail that
/// is the start of some `from`, held until the next bytes settle it.
pub struct Streamer {
    swaps: Swaps,
    pending: Vec<u8>,
}

impl Streamer {
    pub fn new(swaps: Swaps) -> Streamer {
        Streamer {
            swaps,
            pending: Vec::new(),
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        if self.swaps.is_empty() {
            return bytes.to_vec();
        }
        self.pending.extend_from_slice(bytes);
        let mut out = Vec::with_capacity(self.pending.len());
        let mut i = 0;
        while i < self.pending.len() {
            match self.swaps.at(&self.pending[i..], true) {
                Match::Found(from, to) => {
                    out.extend_from_slice(to);
                    i += from.len();
                }
                Match::Maybe => break,
                Match::None => {
                    out.push(self.pending[i]);
                    i += 1;
                }
            }
        }
        self.pending.drain(..i);
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
        Swaps::new(vec![(
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
        relay(
            &mut reader,
            &mut out,
            head,
            framing,
            &swaps(),
            &swaps(),
            true,
        )
        .unwrap();
        String::from_utf8(out).unwrap()
    }

    fn unchunk(body: &str) -> String {
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
        joined
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
        assert_eq!(unchunk(body), "data: sk_real end");
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
    fn a_bare_cr_or_control_byte_in_a_head_is_refused_and_a_tab_is_not() {
        for head in [
            &b"GET / HTTP/1.1\r\nX-A: a\rX-B: b\r\n\r\n"[..],
            b"GET / HTTP/1.1\rHost: x\r\n\r\n",
            b"GET / HTTP/1.1\r\nX-A: a\x00b\r\n\r\n",
            b"GET / HTTP/1.1\r\nX-A: a\x7fb\r\n\r\n",
            b"GET /\x0b HTTP/1.1\r\n\r\n",
        ] {
            assert!(
                read_head(&mut Cursor::new(head.to_vec())).is_err(),
                "{head:?}"
            );
        }
        let head = read_head(&mut Cursor::new(
            b"GET / HTTP/1.1\r\nX-A: a\tb\r\n\r\n".to_vec(),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(head.get("x-a"), Some("a\tb"));
    }

    #[test]
    fn a_header_that_is_not_utf8_passes_byte_for_byte() {
        let input = b"HTTP/1.1 200 OK\r\nContent-Disposition: attachment; filename=\"caf\xe9.txt\"\r\nContent-Length: 2\r\n\r\nok";
        let mut reader = Cursor::new(input.to_vec());
        let head = read_head(&mut reader).unwrap().unwrap();
        let framing = response_framing(&head, "GET").unwrap();
        let mut out = Vec::new();
        relay(
            &mut reader,
            &mut out,
            head,
            framing,
            &swaps(),
            &swaps(),
            true,
        )
        .unwrap();
        assert_eq!(out, input);
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
        let mut reader = Cursor::new(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nokXX0\r\n\r\n".to_vec(),
        );
        let head = read_head(&mut reader).unwrap().unwrap();
        let framing = response_framing(&head, "GET").unwrap();
        let mut out = Vec::new();
        assert!(
            relay(
                &mut reader,
                &mut out,
                head,
                framing,
                &swaps(),
                &swaps(),
                true
            )
            .is_err()
        );
    }

    #[test]
    fn the_streamer_never_emits_half_a_match() {
        let mut s = Streamer::new(swaps());
        let mut out = Vec::new();
        for byte in b"aPLACEHOLDER_0123456789abz" {
            out.extend(s.push(&[*byte]));
        }
        out.extend(s.finish());
        assert_eq!(out, b"ask_realz");
    }

    #[test]
    fn the_streamer_holds_back_only_what_could_still_be_a_match() {
        let mut s = Streamer::new(swaps());
        assert_eq!(s.push(b"+OK\r\n"), b"+OK\r\n");
        assert_eq!(s.push(b"x PLACEHOLDER_0"), b"x ");
        assert_eq!(s.push(b"123456789ab\r\n"), b"sk_real\r\n");
    }

    #[test]
    fn swaps_are_one_pass_and_the_longest_match_wins_whatever_the_order() {
        let s = Swaps::new(vec![
            (b"abc".to_vec(), b"1".to_vec()),
            (b"abcdef".to_vec(), b"2".to_vec()),
        ]);
        assert_eq!(s.apply(b"xabcdefabcx"), b"x21x");
        let chained = Swaps::new(vec![
            (b"AAAA".to_vec(), b"BBBB".to_vec()),
            (b"BBBB".to_vec(), b"CCCC".to_vec()),
        ]);
        assert_eq!(chained.apply(b"AAAA BBBB"), b"BBBB CCCC");
        let mut stream = Streamer::new(s.clone());
        let mut out = stream.push(b"xabc");
        out.extend(stream.push(b"def abc"));
        out.extend(stream.finish());
        assert_eq!(out, b"x2 1");
    }

    #[test]
    fn content_encodings_penv_cannot_read_are_refused() {
        let head = |text: &str| {
            read_head(&mut Cursor::new(text.as_bytes().to_vec()))
                .unwrap()
                .unwrap()
        };
        let gzip = head("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 3\r\n\r\n");
        assert!(readable(&gzip, &Framing::Length(3)).is_err());
        assert!(readable(&gzip, &Framing::None).is_ok());
        let plain = head("HTTP/1.1 200 OK\r\nContent-Encoding: identity\r\n\r\n");
        assert!(readable(&plain, &Framing::Close).is_ok());
        let listed = head("HTTP/1.1 200 OK\r\nContent-Encoding: identity, br\r\n\r\n");
        assert!(readable(&listed, &Framing::Chunked).is_err());
    }

    /// Counts what is written without keeping it.
    struct Count(usize);

    impl Write for Count {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_body_over_the_buffer_limit_streams_instead_of_being_refused() {
        let n = MAX_BODY + 1;
        let head_text = format!("PUT /big HTTP/1.1\r\nContent-Length: {n}\r\n\r\n");
        let mut reader = io::BufReader::new(
            Cursor::new(head_text.clone().into_bytes()).chain(io::repeat(b'a').take(n as u64)),
        );
        let head = read_head(&mut reader).unwrap().unwrap();
        let framing = request_framing(&head).unwrap();
        let mut out = Count(0);
        relay(
            &mut reader,
            &mut out,
            head,
            framing,
            &swaps(),
            &Swaps::default(),
            true,
        )
        .unwrap();
        assert_eq!(out.0, head_text.len() + n);

        // With swaps to apply, a long response is re-framed as chunked.
        let mut body = vec![b'a'; BUFFER + 10];
        body.extend_from_slice(b"PLACEHOLDER_0123456789ab.");
        let mut input =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        input.extend_from_slice(&body);
        let out = relay_all(&input, false);
        let (head, chunked) = out.split_once("\r\n\r\n").unwrap();
        assert!(head.contains("Transfer-Encoding: chunked"), "{head}");
        assert!(!head.contains("Content-Length"), "{head}");
        let joined = unchunk(chunked);
        assert_eq!(joined.len(), BUFFER + 10 + "sk_real.".len());
        assert!(joined.ends_with("aask_real."));
    }
}
