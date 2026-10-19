//! `url()`, docs/Functions.md. A hand-written parser: the schema crate takes no URL dependency.

use super::{Arg, Call};
use crate::resolve::Expr;

#[derive(Debug, Default)]
struct Url {
    scheme: String,
    user: Option<String>,
    password: Option<String>,
    /// Each `host[:port]` as written; several for a replica set.
    hosts: Vec<(String, Option<String>)>,
    path: String,
    /// Raw `name=value` pairs, still encoded.
    query: Vec<(String, Option<String>)>,
    fragment: Option<String>,
}

/// `password=` takes a `$KEY` reference. Called by `check_args` on the url arguments.
pub fn check_password(args: &[Arg]) -> Result<(), String> {
    for arg in args {
        if let Arg::Named(label, value) = arg
            && label == "password"
            && matches!(value, Expr::Text(_))
        {
            return Err(
                "url() password= takes a $KEY reference, never a literal; store the password in a key and write password=$KEY"
                    .into(),
            );
        }
    }
    Ok(())
}

pub fn eval(call: &Call) -> Result<String, String> {
    let Some(subject) = call.subject.as_deref() else {
        return Err("url() needs a subject; write url($KEY, part)".into());
    };
    match (call.address, call.options.is_empty()) {
        (Some(part), true) => read(subject, part),
        (None, false) => write(subject, &call.options),
        (Some(_), false) => Err("url() reads or writes, never both; call it once for each".into()),
        (None, true) => Err("url() needs a part to read or a part=value to write".into()),
    }
}

fn parse(text: &str, part: &str) -> Result<Url, String> {
    let fail = |why: &str| {
        format!("url() cannot use part {part}: {why}; the subject must look like scheme://host")
    };
    let (scheme, rest) = text
        .split_once("://")
        .ok_or_else(|| fail("the subject is not a URL"))?;
    let mut chars = scheme.chars();
    let ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !ok {
        return Err(fail("the subject is not a URL"));
    }
    let (rest, fragment) = match rest.split_once('#') {
        Some((r, f)) => (r, Some(f.to_string())),
        None => (rest, None),
    };
    let (rest, query) = match rest.split_once('?') {
        Some((r, q)) => (r, Some(q)),
        None => (rest, None),
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let (userinfo, hosts) = match authority.rfind('@') {
        Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
        None => (None, authority),
    };
    let (user, password) = match userinfo {
        None => (None, None),
        Some(u) => match u.split_once(':') {
            Some((a, b)) => (Some(a.to_string()), Some(b.to_string())),
            None => (Some(u.to_string()), None),
        },
    };
    let hosts = hosts.split(',').map(split_host).collect();
    let query = query
        .map(|q| {
            q.split('&')
                .filter(|p| !p.is_empty())
                .map(|p| match p.split_once('=') {
                    Some((n, v)) => (n.to_string(), Some(v.to_string())),
                    None => (p.to_string(), None),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Url {
        scheme: scheme.to_string(),
        user,
        password,
        hosts,
        path: path.to_string(),
        query,
        fragment,
    })
}

fn split_host(text: &str) -> (String, Option<String>) {
    let tail = text.rfind(']').map_or(0, |i| i + 1);
    match text[tail..].rfind(':') {
        Some(i) => (
            text[..tail + i].to_string(),
            Some(text[tail + i + 1..].to_string()),
        ),
        None => (text.to_string(), None),
    }
}

impl Url {
    fn single(&self, part: &str) -> Result<&(String, Option<String>), String> {
        match self.hosts.as_slice() {
            [one] => Ok(one),
            _ => Err(format!(
                "url() cannot use part {part}: the URL names several hosts; read scheme, path or a query part instead"
            )),
        }
    }

    fn render(&self) -> String {
        let mut out = format!("{}://", self.scheme);
        if self.user.is_some() || self.password.is_some() {
            out.push_str(self.user.as_deref().unwrap_or(""));
            if let Some(p) = &self.password {
                out.push(':');
                out.push_str(p);
            }
            out.push('@');
        }
        let hosts: Vec<String> = self
            .hosts
            .iter()
            .map(|(h, p)| match p {
                Some(p) => format!("{h}:{p}"),
                None => h.clone(),
            })
            .collect();
        out.push_str(&hosts.join(","));
        out.push_str(&self.path);
        if !self.query.is_empty() {
            let pairs: Vec<String> = self
                .query
                .iter()
                .map(|(n, v)| match v {
                    Some(v) => format!("{n}={v}"),
                    None => n.clone(),
                })
                .collect();
            out.push('?');
            out.push_str(&pairs.join("&"));
        }
        if let Some(f) = &self.fragment {
            out.push('#');
            out.push_str(f);
        }
        out
    }
}

fn read(subject: &str, part: &str) -> Result<String, String> {
    let url = parse(subject, part)?;
    Ok(match part {
        "scheme" => url.scheme,
        "host" => url.single(part)?.0.clone(),
        "port" => url.single(part)?.1.clone().unwrap_or_default(),
        "origin" => {
            let (h, p) = url.single(part)?;
            match p {
                Some(p) => format!("{}://{h}:{p}", url.scheme),
                None => format!("{}://{h}", url.scheme),
            }
        }
        "path" => url.path,
        "user" => url.user.as_deref().map(decode).unwrap_or_default(),
        "password" => url.password.as_deref().map(decode).unwrap_or_default(),
        _ => match part.strip_prefix("query.") {
            Some(name) => url
                .query
                .iter()
                .find(|(n, _)| n == name)
                .and_then(|(_, v)| v.as_deref().map(decode))
                .unwrap_or_default(),
            None => return Err(format!("url() has no part {part}")),
        },
    })
}

fn write(subject: &str, options: &[(&str, String)]) -> Result<String, String> {
    let mut url = parse(subject, options[0].0)?;
    for (part, value) in options {
        match *part {
            "scheme" => url.scheme = value.clone(),
            "host" | "port" => {
                if url.hosts.len() != 1 {
                    url.single(part)?;
                }
                if *part == "host" {
                    url.hosts[0].0 = value.clone();
                } else if value.is_empty() {
                    url.hosts[0].1 = None;
                } else if value.parse::<u16>().is_ok() {
                    url.hosts[0].1 = Some(value.clone());
                } else {
                    return Err("url() port takes a whole number from 0 to 65535".into());
                }
            }
            "path" => {
                url.path = if value.is_empty() || value.starts_with('/') {
                    value.clone()
                } else {
                    format!("/{value}")
                }
            }
            "user" => url.user = Some(encode(value)),
            "password" => url.password = Some(encode(value)),
            _ => {
                let Some(name) = part.strip_prefix("query.") else {
                    return Err(format!("url() cannot write part {part}"));
                };
                let value = Some(encode(value));
                match url.query.iter_mut().find(|(n, _)| n == name) {
                    Some(pair) => pair.1 = value,
                    None => url.query.push((name.to_string(), value)),
                }
            }
        }
    }
    Ok(url.render())
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
