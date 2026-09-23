//! Placeholders for values an agent's process must not hold. The shape comes
//! from the schema, never from the value: a key typed
//! `string(startsWith=sk_live_, minLength=32)` gets a placeholder that passes the
//! same checks an SDK makes, and nothing in it was read from the secret.

use crate::ir::{BaseType, Type};

/// The prefix of a placeholder for a key whose type says nothing about its shape.
pub const FALLBACK_PREFIX: &str = "penvph_";

/// Random characters in every placeholder: enough that two never collide and
/// that one never occurs in ordinary traffic by chance.
pub const RANDOM_LEN: usize = 24;

const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// A placeholder that satisfies `ty`'s own rules. `bytes(n)` supplies `n` random
/// bytes; the caller passes the OS generator, tests pass a fixed sequence.
/// `None` when the rules leave no room for the random part, or ask for a shape
/// penv does not generate (`matches`, a non-string type).
pub fn placeholder(ty: &Type, bytes: &mut dyn FnMut(usize) -> Vec<u8>) -> Option<String> {
    if ty.base != BaseType::String || !ty.members.is_empty() || constraint(ty, "matches").is_some()
    {
        return None;
    }
    let starts = constraint(ty, "startsWith").unwrap_or_default();
    let ends = constraint(ty, "endsWith").unwrap_or_default();
    let (prefix, suffix) = if starts.is_empty() && ends.is_empty() {
        (FALLBACK_PREFIX.to_string(), String::new())
    } else {
        (starts, ends)
    };
    let min = number(ty, "minLength").unwrap_or(0);
    let max = number(ty, "maxLength");
    let fixed = prefix.chars().count() + suffix.chars().count();
    let mut random = RANDOM_LEN.max(min.saturating_sub(fixed));
    if let Some(max) = max {
        random = random.min(max.saturating_sub(fixed));
    }
    if random < RANDOM_LEN.min(16) {
        return None;
    }
    let middle: String = bytes(random)
        .into_iter()
        .map(|b| ALPHABET[usize::from(b) % ALPHABET.len()] as char)
        .collect();
    Some(format!("{prefix}{middle}{suffix}"))
}

fn constraint(ty: &Type, name: &str) -> Option<String> {
    ty.constraints
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
}

fn number(ty: &Type, name: &str) -> Option<usize> {
    constraint(ty, name).and_then(|v| v.trim().parse().ok())
}

/// Domains under which anyone can get a name: country second levels and
/// hosting platforms. `*.vercel.app` would send a value to every stranger's
/// deploy, so a wildcard directly over one of these is refused.
pub const SHARED_SUFFIXES: &[&str] = &[
    "co.uk",
    "org.uk",
    "ac.uk",
    "gov.uk",
    "me.uk",
    "ltd.uk",
    "plc.uk",
    "com.au",
    "net.au",
    "org.au",
    "co.nz",
    "co.jp",
    "ne.jp",
    "or.jp",
    "co.za",
    "co.in",
    "co.kr",
    "com.br",
    "com.cn",
    "com.mx",
    "com.ng",
    "com.sg",
    "com.tr",
    "com.ar",
    "com.hk",
    "com.tw",
    "github.io",
    "gitlab.io",
    "vercel.app",
    "netlify.app",
    "pages.dev",
    "workers.dev",
    "herokuapp.com",
    "fly.dev",
    "onrender.com",
    "web.app",
    "firebaseapp.com",
    "appspot.com",
    "azurewebsites.net",
    "cloudfront.net",
    "amazonaws.com",
    "blob.core.windows.net",
    "ngrok.io",
    "ngrok-free.app",
    "trycloudflare.com",
    "deno.dev",
    "replit.app",
    "glitch.me",
    "surge.sh",
    "railway.app",
    "up.railway.app",
    "supabase.co",
    "repl.co",
];

/// `api.example.com`, `localhost`, `10.0.0.5` or `*.example.com`: lowercase
/// labels, a wildcard only as the whole first label, with two labels after it
/// and never directly over a shared suffix, and no scheme, port, path or user.
pub fn is_host_pattern(host: &str) -> bool {
    let (wild, rest) = match host.strip_prefix("*.") {
        Some(rest) => (true, rest),
        None => (false, host),
    };
    if wild && SHARED_SUFFIXES.contains(&rest) {
        return false;
    }
    let labels: Vec<&str> = rest.split('.').collect();
    if rest.is_empty() || rest.len() > 253 || (wild && labels.len() < 2) {
        return false;
    }
    labels.iter().all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    })
}

/// Whether `host` is one `patterns` names. `*.example.com` matches any name
/// under example.com, not example.com itself.
pub fn host_allowed(patterns: &[String], host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    patterns.iter().any(|p| match p.strip_prefix("*.") {
        Some(domain) => host.len() > domain.len() + 1 && host.ends_with(&format!(".{domain}")),
        None => host == *p,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter() -> impl FnMut(usize) -> Vec<u8> {
        let mut next = 0u8;
        move |n| {
            (0..n)
                .map(|_| {
                    next = next.wrapping_add(7);
                    next
                })
                .collect()
        }
    }

    fn string(constraints: &[(&str, &str)]) -> Type {
        let mut ty = Type::new(BaseType::String);
        ty.constraints = constraints
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ty
    }

    #[test]
    fn the_shape_comes_from_the_type() {
        let mut random = counter();
        let ty = string(&[("minLength", "40"), ("startsWith", "sk_live_")]);
        let p = placeholder(&ty, &mut random).unwrap();
        assert!(p.starts_with("sk_live_"), "{p}");
        assert!(p.len() >= 40, "{p}");
        assert!(p[8..].chars().all(|c| c.is_ascii_alphanumeric()));

        let ends = placeholder(&string(&[("endsWith", "==")]), &mut random).unwrap();
        assert!(
            ends.ends_with("==") && ends.len() == RANDOM_LEN + 2,
            "{ends}"
        );

        let plain = placeholder(&string(&[]), &mut random).unwrap();
        assert!(
            plain.starts_with(FALLBACK_PREFIX) && plain.len() == FALLBACK_PREFIX.len() + RANDOM_LEN
        );
    }

    #[test]
    fn a_shape_it_cannot_honour_is_none() {
        let mut random = counter();
        assert!(placeholder(&string(&[("matches", "^a+$")]), &mut random).is_none());
        assert!(
            placeholder(
                &string(&[("startsWith", "x"), ("maxLength", "10")]),
                &mut random
            )
            .is_none()
        );
        assert!(placeholder(&Type::new(BaseType::Url), &mut random).is_none());
        let fits = placeholder(
            &string(&[("startsWith", "x"), ("maxLength", "20")]),
            &mut random,
        )
        .unwrap();
        assert_eq!(fits.len(), 20, "{fits}");
    }

    #[test]
    fn two_placeholders_differ() {
        let mut random = counter();
        let ty = string(&[("startsWith", "sk_")]);
        assert_ne!(placeholder(&ty, &mut random), placeholder(&ty, &mut random));
    }

    #[test]
    fn hosts_are_names_and_a_wildcard_covers_subdomains_only() {
        for ok in [
            "api.stripe.com",
            "localhost",
            "10.0.0.5",
            "*.stripe.com",
            "db-1.internal",
        ] {
            assert!(is_host_pattern(ok), "{ok}");
        }
        for bad in [
            "*",
            "*.com",
            "*.co.uk",
            "*.vercel.app",
            "*.amazonaws.com",
            "https://a.com",
            "a.com:443",
            "a.com/x",
            "A.com",
            "u@a.com",
            "",
            "-a.com",
            "a..com",
        ] {
            assert!(!is_host_pattern(bad), "{bad}");
        }
        let allowed = vec!["api.stripe.com".to_string(), "*.acme.dev".to_string()];
        assert!(host_allowed(&allowed, "api.stripe.com"));
        assert!(host_allowed(&allowed, "API.Stripe.com."));
        assert!(host_allowed(&allowed, "eu.api.acme.dev"));
        assert!(!host_allowed(&allowed, "acme.dev"));
        assert!(!host_allowed(&allowed, "evilacme.dev"));
        assert!(!host_allowed(&allowed, "api.stripe.com.evil.test"));
        assert!(!host_allowed(&allowed, "stripe.com"));
    }
}
