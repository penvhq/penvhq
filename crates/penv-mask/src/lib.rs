//! Streaming scrubber that masks sensitive values in a child process's output.
//! A secret is caught however it was split across reads, and in the forms a
//! program is likely to have re-encoded it into on the way out.

/// Shorter than this and the mask would swallow ordinary text.
pub const MIN_SECRET_LEN: usize = 4;

/// What replaces a match, after the first two characters of the secret.
pub const BLOCKS: &str = "\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}";

const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";
const UPPER_HEX: &[u8; 16] = b"0123456789ABCDEF";

/// What a matched secret is replaced with: enough to recognise which one it was,
/// and nothing that helps use it.
pub fn redaction(secret: &str) -> String {
    let head: String = secret.chars().take(2).collect();
    format!("{head}{BLOCKS}")
}

struct Pattern {
    bytes: Vec<u8>,
    replacement: Vec<u8>,
    /// Base64 arrives wrapped, so the line breaks inside a match are skipped.
    wrapped: bool,
}

enum Hit {
    /// A whole pattern matched, consuming this many bytes of the buffer.
    Full(usize),
    /// The buffer ran out part way through a pattern.
    Partial,
    None,
}

/// Masks a byte stream fed to it in arbitrary chunks.
pub struct Masker {
    patterns: Vec<Pattern>,
    /// Pattern indices by first byte, longest first, so a scan step is a lookup.
    by_first_byte: Vec<Vec<u32>>,
    longest: usize,
    held: Vec<u8>,
}

impl Masker {
    pub fn new(secrets: Vec<String>) -> Masker {
        let mut patterns: Vec<Pattern> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for secret in &secrets {
            if secret.len() < MIN_SECRET_LEN {
                continue;
            }
            let replacement = redaction(secret).into_bytes();
            for (form, wrapped) in forms(secret) {
                if !form.is_empty() && seen.insert(form.clone()) {
                    patterns.push(Pattern {
                        bytes: form.into_bytes(),
                        replacement: replacement.clone(),
                        wrapped,
                    });
                }
            }
        }
        patterns.sort_by_key(|p| std::cmp::Reverse(p.bytes.len()));

        let longest = patterns.first().map_or(0, |p| p.bytes.len());
        let mut by_first_byte = vec![Vec::new(); 256];
        for (index, pattern) in patterns.iter().enumerate() {
            by_first_byte[pattern.bytes[0] as usize].push(index as u32);
        }

        Masker {
            patterns,
            by_first_byte,
            longest,
            held: Vec::new(),
        }
    }

    /// True when nothing is worth scanning for, so the stream can be copied.
    pub fn is_pass_through(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Scrub a chunk. Only bytes that could still grow into a secret are held
    /// back, so a prompt with no newline reaches the terminal at once.
    pub fn feed(&mut self, chunk: &[u8], out: &mut Vec<u8>) {
        if self.is_pass_through() {
            out.extend_from_slice(chunk);
            return;
        }
        self.held.extend_from_slice(chunk);
        self.scan(false, out);
    }

    /// Scrub what is held back and release it. The stream ends here.
    pub fn finish(&mut self, out: &mut Vec<u8>) {
        if self.is_pass_through() {
            return;
        }
        self.scan(true, out);
    }

    fn scan(&mut self, ending: bool, out: &mut Vec<u8>) {
        // A wrapped match can span at most twice its pattern, so nothing before
        // that window can still be waiting for more bytes.
        let tail = self.held.len().saturating_sub(self.longest * 2);
        let mut index = 0;
        let mut run = 0;
        while index < self.held.len() {
            // A longer pattern still waiting for bytes outranks a shorter one that
            // already fits, so a partial stops the scan before any match is taken.
            if !ending && index >= tail && self.partial_at(index) {
                break;
            }
            match self.match_at(index) {
                (Some(pattern), consumed) => {
                    out.extend_from_slice(&self.held[run..index]);
                    out.extend_from_slice(&self.patterns[pattern].replacement);
                    index += consumed;
                    run = index;
                }
                (None, _) => index += 1,
            }
        }
        out.extend_from_slice(&self.held[run..index]);
        self.held.drain(..index);
    }

    /// The longest pattern that fits whole at this position, and what it ate.
    fn match_at(&self, index: usize) -> (Option<usize>, usize) {
        let rest = &self.held[index..];
        for i in self.by_first_byte[rest[0] as usize]
            .iter()
            .map(|i| *i as usize)
        {
            if let Hit::Full(consumed) = compare(rest, &self.patterns[i]) {
                return (Some(i), consumed);
            }
        }
        (None, 0)
    }

    /// True when what is left could still be the start of a pattern.
    fn partial_at(&self, index: usize) -> bool {
        let rest = &self.held[index..];
        self.by_first_byte[rest[0] as usize]
            .iter()
            .any(|i| matches!(compare(rest, &self.patterns[*i as usize]), Hit::Partial))
    }
}

/// Match one pattern at the head of `hay`, stepping over the line breaks base64
/// picks up in transit. A run of breaks longer than the pattern is not wrapping.
fn compare(hay: &[u8], pattern: &Pattern) -> Hit {
    let mut i = 0;
    let mut matched = 0;
    let mut skipped = 0;
    while matched < pattern.bytes.len() {
        if i >= hay.len() {
            return Hit::Partial;
        }
        let byte = hay[i];
        if pattern.wrapped && matched > 0 && (byte == b'\r' || byte == b'\n') {
            skipped += 1;
            if skipped > pattern.bytes.len() {
                return Hit::None;
            }
            i += 1;
            continue;
        }
        if byte != pattern.bytes[matched] {
            return Hit::None;
        }
        i += 1;
        matched += 1;
    }
    Hit::Full(i)
}

/// The shapes one secret can leave a process in: as written, base64 in both
/// alphabets at every phase a prefix can push it to, hex, percent-encoded and
/// escaped as a JSON string body. The flag marks the forms that arrive wrapped.
fn forms(secret: &str) -> Vec<(String, bool)> {
    let raw = secret.as_bytes();
    let mut out = vec![
        (secret.to_string(), false),
        (base64(raw, STANDARD, true), true),
        (base64(raw, STANDARD, false), true),
        (base64(raw, URL_SAFE, true), true),
        (base64(raw, URL_SAFE, false), true),
    ];
    for phase in 0..3 {
        out.push((phased_base64(raw, STANDARD, phase), true));
        out.push((phased_base64(raw, URL_SAFE, phase), true));
    }
    out.push((hex(raw, LOWER_HEX), false));
    out.push((hex(raw, UPPER_HEX), false));
    out.push((percent(secret, UPPER_HEX), false));
    out.push((percent(secret, LOWER_HEX), false));
    out.push((json_escaped(secret), false));
    out
}

fn base64(input: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for group in input.chunks(3) {
        let high = group[0] as u32;
        let mid = *group.get(1).unwrap_or(&0) as u32;
        let low = *group.get(2).unwrap_or(&0) as u32;
        let n = (high << 16) | (mid << 8) | low;
        let digits = [
            alphabet[((n >> 18) & 63) as usize],
            alphabet[((n >> 12) & 63) as usize],
            alphabet[((n >> 6) & 63) as usize],
            alphabet[(n & 63) as usize],
        ];
        let kept = group.len() + 1;
        for digit in digits.iter().take(kept) {
            out.push(*digit as char);
        }
        if pad {
            for _ in kept..4 {
                out.push('=');
            }
        }
    }
    out
}

/// The secret as it appears inside a larger base64 body, where a prefix has
/// pushed it off the group boundary: the groups the neighbouring bytes cannot
/// reach, so both the partial leading and the partial trailing group are dropped.
fn phased_base64(input: &[u8], alphabet: &[u8; 64], phase: usize) -> String {
    let mut padded = vec![b'X'; phase];
    padded.extend_from_slice(input);
    let whole = base64(&padded, alphabet, false);
    let start = if phase == 0 { 0 } else { 4 };
    let complete = (padded.len() / 3) * 4;
    whole.get(start..complete).unwrap_or_default().to_string()
}

fn hex(input: &[u8], digits: &[u8; 16]) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for byte in input {
        out.push(digits[(byte >> 4) as usize] as char);
        out.push(digits[(byte & 15) as usize] as char);
    }
    out
}

/// `encodeURIComponent`, which leaves the unreserved set and `!'()*~` alone.
fn percent(value: &str, digits: &[u8; 16]) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(digits[(byte >> 4) as usize] as char);
            out.push(digits[(byte & 15) as usize] as char);
        }
    }
    out
}

/// The body of a JSON string, without the quotes around it.
fn json_escaped(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
