//! Database URLs, only as far as a sealed run needs them: find the password and
//! the host, and write the URL the command gets instead.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbUrl {
    pub scheme: String,
    pub user: String,
    /// Decoded.
    pub password: String,
    pub host: String,
    pub port: Option<u16>,
    /// Everything after the authority: `/app?sslmode=require`.
    pub rest: String,
}

pub fn parse(url: &str) -> Option<DbUrl> {
    let (scheme, after) = url.split_once("://")?;
    let (authority, rest) = match after.find(['/', '?']) {
        Some(i) => (&after[..i], &after[i..]),
        None => (after, ""),
    };
    let (userinfo, hostport) = authority.rsplit_once('@')?;
    let (user, password) = userinfo.split_once(':')?;
    let (host, port) = if let Some(v6) = hostport.strip_prefix('[') {
        let (h, tail) = v6.split_once(']')?;
        (
            h.to_string(),
            match tail.strip_prefix(':') {
                Some(p) => Some(p.parse().ok()?),
                None if tail.is_empty() => None,
                None => return None,
            },
        )
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p.parse().ok()?)),
            None => (hostport.to_string(), None),
        }
    };
    if host.is_empty() || host.contains(',') {
        // A multi-host URL (host1,host2) is not something one proxy stands in for.
        return None;
    }
    Some(DbUrl {
        scheme: scheme.to_ascii_lowercase(),
        user: decode(user)?,
        password: decode(password)?,
        host: host.to_ascii_lowercase(),
        port,
        rest: rest.to_string(),
    })
}

fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// A query parameter's value, as written.
pub fn query(rest: &str, name: &str) -> Option<String> {
    let q = rest.split_once('?')?.1;
    q.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| decode(v).unwrap_or_else(|| v.to_string()))
    })
}

/// `rest` with `name` set to `value`, added when missing.
pub fn with_query(rest: &str, name: &str, value: &str) -> String {
    let (path, q) = rest.split_once('?').unwrap_or((rest, ""));
    let mut pairs: Vec<String> = q
        .split('&')
        .filter(|p| !p.is_empty() && p.split('=').next() != Some(name))
        .map(str::to_string)
        .collect();
    pairs.push(format!("{name}={value}"));
    format!("{path}?{}", pairs.join("&"))
}

/// `rest` without the query parameters whose names `drop` picks.
pub fn without_query(rest: &str, drop: impl Fn(&str) -> bool) -> String {
    let Some((path, q)) = rest.split_once('?') else {
        return rest.to_string();
    };
    let kept: Vec<&str> = q
        .split('&')
        .filter(|p| !p.is_empty() && !drop(p.split('=').next().unwrap_or("")))
        .collect();
    if kept.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_postgres_url_is_split_and_its_password_decoded() {
        let u =
            parse("postgres://app:p%40ss%3Aw%2Fd@DB.acme.com:5432/app?sslmode=require").unwrap();
        assert_eq!(
            (
                u.user.as_str(),
                u.password.as_str(),
                u.host.as_str(),
                u.port
            ),
            ("app", "p@ss:w/d", "db.acme.com", Some(5432))
        );
        assert_eq!(u.rest, "/app?sslmode=require");
        assert_eq!(query(&u.rest, "sslmode").as_deref(), Some("require"));
        assert_eq!(
            with_query(&u.rest, "sslmode", "disable"),
            "/app?sslmode=disable"
        );
        assert_eq!(
            with_query("/app", "sslmode", "disable"),
            "/app?sslmode=disable"
        );
        let r = parse("rediss://:secret@cache.acme.com/0").unwrap();
        assert_eq!(
            (r.user.as_str(), r.password.as_str(), r.port),
            ("", "secret", None)
        );
        assert!(
            parse("postgres://app@db/app").is_none(),
            "no password, nothing to seal"
        );
        assert!(parse("postgres://a:b@h1,h2/app").is_none());
        assert_eq!(parse("postgres://a:b@[::1]:6/x").unwrap().host, "::1");
    }

    #[test]
    fn query_parameters_are_dropped_by_name() {
        let ssl = |k: &str| k.starts_with("ssl");
        assert_eq!(
            without_query("/app?sslmode=require&sslrootcert=system&x=1", ssl),
            "/app?x=1"
        );
        assert_eq!(without_query("/app?sslmode=require", ssl), "/app");
        assert_eq!(without_query("/app", ssl), "/app");
    }
}
