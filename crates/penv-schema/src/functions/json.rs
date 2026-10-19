//! `json()`, docs/Functions.md. Order-keeping values, so output keeps document order.

use super::Call;

#[derive(Debug, Clone, PartialEq)]
enum J {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Key(String),
    Idx(usize),
}

const MAX_DEPTH: usize = 128;

struct Parser<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
    dups: Vec<String>,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.b.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn fail<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{what} at byte {}", self.i))
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        let hit = self.b.get(self.i) == Some(&c);
        if hit {
            self.i += 1;
        }
        hit
    }

    fn value(&mut self, depth: usize) -> Result<J, String> {
        if depth > MAX_DEPTH {
            return self.fail("nesting too deep");
        }
        self.ws();
        match self.b.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut obj: Vec<(String, J)> = Vec::new();
                if self.eat(b'}') {
                    return Ok(J::Obj(obj));
                }
                loop {
                    self.ws();
                    if self.b.get(self.i) != Some(&b'"') {
                        return self.fail("expected a key");
                    }
                    let k = self.string()?;
                    if !self.eat(b':') {
                        return self.fail("expected ':'");
                    }
                    let v = self.value(depth + 1)?;
                    match obj.iter_mut().find(|(n, _)| *n == k) {
                        Some(slot) => {
                            slot.1 = v;
                            if !self.dups.contains(&k) {
                                self.dups.push(k);
                            }
                        }
                        None => obj.push((k, v)),
                    }
                    if self.eat(b',') {
                        continue;
                    }
                    if self.eat(b'}') {
                        return Ok(J::Obj(obj));
                    }
                    return self.fail("expected ',' or '}'");
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut arr = Vec::new();
                if self.eat(b']') {
                    return Ok(J::Arr(arr));
                }
                loop {
                    arr.push(self.value(depth + 1)?);
                    if self.eat(b',') {
                        continue;
                    }
                    if self.eat(b']') {
                        return Ok(J::Arr(arr));
                    }
                    return self.fail("expected ',' or ']'");
                }
            }
            Some(b'"') => self.string().map(J::Str),
            Some(b't') => self.word("true", J::Bool(true)),
            Some(b'f') => self.word("false", J::Bool(false)),
            Some(b'n') => self.word("null", J::Null),
            Some(c) if *c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                while self.b.get(self.i).is_some_and(|c| {
                    matches!(c, b'-' | b'+' | b'.' | b'e' | b'E') || c.is_ascii_digit()
                }) {
                    self.i += 1;
                }
                let n = &self.s[start..self.i];
                if is_number(n) {
                    Ok(J::Num(n.to_string()))
                } else {
                    self.i = start;
                    self.fail("bad number")
                }
            }
            _ => self.fail("unexpected input"),
        }
    }

    fn word(&mut self, w: &str, v: J) -> Result<J, String> {
        if self.b[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            Ok(v)
        } else {
            self.fail("unexpected input")
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self
            .s
            .get(self.i..self.i + 4)
            .and_then(|h| u32::from_str_radix(h, 16).ok());
        match h {
            Some(n) => {
                self.i += 4;
                Ok(n)
            }
            None => self.fail("bad \\u escape"),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let Some(&c) = self.b.get(self.i) else {
                return self.fail("unterminated string");
            };
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.i += 1;
                    let Some(&e) = self.b.get(self.i) else {
                        return self.fail("unterminated string");
                    };
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) {
                                if !self.b[self.i..].starts_with(b"\\u") {
                                    return self.fail("lone surrogate");
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return self.fail("lone surrogate");
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            match char::from_u32(cp) {
                                Some(ch) => out.push(ch),
                                None => return self.fail("lone surrogate"),
                            }
                        }
                        _ => return self.fail("bad escape"),
                    }
                }
                c if c < 0x20 => return self.fail("control character in string"),
                _ => {
                    let ch = self.s[self.i..].chars().next().unwrap_or('\u{fffd}');
                    out.push(ch);
                    self.i += ch.len_utf8();
                }
            }
        }
    }
}

/// JSON number grammar.
fn is_number(n: &str) -> bool {
    let b = n.as_bytes();
    let mut i = 0;
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    match b.get(i) {
        Some(b'0') => i += 1,
        Some(c) if c.is_ascii_digit() => {
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
        _ => return false,
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        let from = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == from {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let from = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == from {
            return false;
        }
    }
    i == b.len()
}

fn parse_doc(text: &str) -> Result<(J, Vec<String>), String> {
    let mut p = Parser {
        b: text.as_bytes(),
        s: text,
        i: 0,
        dups: Vec::new(),
    };
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() {
        return p.fail("text after the document");
    }
    Ok((v, p.dups))
}

/// Keys repeated within one object of `text`; the last value wins. Empty when the
/// text is not JSON. The resolver reports each as a soft error naming the key.
pub fn duplicate_keys(text: &str) -> Vec<String> {
    parse_doc(text).map(|(_, d)| d).unwrap_or_default()
}

fn write(v: &J, out: &mut String) {
    match v {
        J::Null => out.push_str("null"),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Num(n) => out.push_str(n),
        J::Str(s) => quote(s, out),
        J::Arr(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(x, out);
            }
            out.push(']');
        }
        J::Obj(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                quote(k, out);
                out.push(':');
                write(x, out);
            }
            out.push('}');
        }
    }
}

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
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
    out.push('"');
}

fn compact(v: &J) -> String {
    let mut s = String::new();
    write(v, &mut s);
    s
}

/// `a.b[0]`, `["a.b"]`, `a["x.y"].c`.
fn parse_path(path: &str) -> Result<Vec<Seg>, String> {
    let b = path.as_bytes();
    let mut segs = Vec::new();
    let mut i = 0;
    let bad = || format!("path `{path}` is not a.b[0] or [\"a.b\"]");
    while i < b.len() {
        match b[i] {
            b'[' => {
                i += 1;
                if b.get(i) == Some(&b'"') {
                    let start = i + 1;
                    let end = path[start..].find('"').ok_or_else(bad)? + start;
                    if b.get(end + 1) != Some(&b']') {
                        return Err(bad());
                    }
                    segs.push(Seg::Key(path[start..end].to_string()));
                    i = end + 2;
                } else {
                    let end = path[i..].find(']').ok_or_else(bad)? + i;
                    let n = path[i..end].parse::<usize>().map_err(|_| bad())?;
                    segs.push(Seg::Idx(n));
                    i = end + 1;
                }
            }
            b'.' if !segs.is_empty() && i + 1 < b.len() && b[i + 1] != b'.' && b[i + 1] != b'[' => {
                i += 1;
            }
            _ => {
                let start = i;
                while i < b.len() && !matches!(b[i], b'.' | b'[') {
                    i += 1;
                }
                if i == start {
                    return Err(bad());
                }
                segs.push(Seg::Key(path[start..i].to_string()));
            }
        }
    }
    if segs.is_empty() {
        return Err(bad());
    }
    Ok(segs)
}

fn get<'a>(mut v: &'a J, segs: &[Seg]) -> Option<&'a J> {
    for s in segs {
        v = match (v, s) {
            (J::Obj(o), Seg::Key(k)) => &o.iter().find(|(n, _)| n == k)?.1,
            (J::Arr(a), Seg::Idx(i)) => a.get(*i)?,
            _ => return None,
        };
    }
    Some(v)
}

fn set(v: &mut J, segs: &[Seg], new: J, path: &str) -> Result<(), String> {
    let Some((head, rest)) = segs.split_first() else {
        *v = new;
        return Ok(());
    };
    if matches!(v, J::Null) {
        *v = match head {
            Seg::Key(_) => J::Obj(Vec::new()),
            Seg::Idx(_) => J::Arr(Vec::new()),
        };
    }
    let slot = match (v, head) {
        (J::Obj(o), Seg::Key(k)) => {
            let at = match o.iter().position(|(n, _)| n == k) {
                Some(at) => at,
                None => {
                    o.push((k.clone(), J::Null));
                    o.len() - 1
                }
            };
            &mut o[at].1
        }
        (J::Arr(a), Seg::Idx(i)) => {
            if *i > 100_000 {
                return Err(format!(
                    "json() index in `{path}` is too large; use a smaller index"
                ));
            }
            while a.len() <= *i {
                a.push(J::Null);
            }
            &mut a[*i]
        }
        _ => {
            return Err(format!(
                "json() path `{path}` crosses a value of the wrong kind; write the path to match the document"
            ));
        }
    };
    set(slot, rest, new, path)
}

/// Bare number, `true` or `false` are typed; anything else, or a quoted value, is a string.
fn typed(text: &str, quoted: bool) -> J {
    if quoted {
        return J::Str(text.to_string());
    }
    match text {
        "true" => J::Bool(true),
        "false" => J::Bool(false),
        t if is_number(t) => J::Num(t.to_string()),
        t => J::Str(t.to_string()),
    }
}

pub fn eval(call: &Call) -> Result<String, String> {
    let doc = call
        .subject
        .as_deref()
        .map(|s| {
            parse_doc(s)
                .map(|(v, _)| v)
                .map_err(|e| format!("json() subject is not valid JSON ({e}); fix the document"))
        })
        .transpose()?;

    if let Some(path) = call.address {
        if !call.options.is_empty() {
            return Err("json() reads or writes, never both; split it into two calls".into());
        }
        let Some(doc) = doc else {
            return Err(format!(
                "json() reads `{path}` from a document; pass one, such as json($KEY, {path})"
            ));
        };
        let segs = parse_path(path)?;
        return match get(&doc, &segs) {
            None | Some(J::Null) => Err(format!(
                "json() path `{path}` is missing or null; check the path or wrap in fallback()"
            )),
            Some(J::Str(s)) => Ok(s.clone()),
            Some(other) => Ok(compact(other)),
        };
    }

    let Some(mut doc) = doc.or_else(|| (!call.options.is_empty()).then(|| J::Obj(Vec::new())))
    else {
        return Err(
            "json() needs a subject or name=value options; write json($KEY) or json(name=value)"
                .into(),
        );
    };
    for (label, value) in &call.options {
        let segs = parse_path(label)?;
        let new = if call.raw.contains(label) {
            parse_doc(value).map(|(v, _)| v).map_err(|e| {
                format!("json() raw `{label}` is not valid JSON ({e}); fix the embedded document")
            })?
        } else {
            typed(value, call.quoted.contains(label))
        };
        set(&mut doc, &segs, new, label)?;
    }
    Ok(compact(&doc))
}
