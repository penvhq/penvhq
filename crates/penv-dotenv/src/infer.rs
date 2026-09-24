use penv_schema::{
    BaseType, Key, Schema, Type, is_absolute_url, is_email, is_public_prefixed, parse_boolean,
};

use crate::read::{Dotenv, is_plausible_key};

/// Draft a schema from a `.env`. Every key is sensitive and required unless a
/// bundler prefix or a value too dull to be a secret says otherwise. A copied
/// value sits in the committed schema, so it is no secret.
pub fn infer(env: &Dotenv) -> Schema {
    let mut schema = Schema::default();
    for entry in env.entries.iter().filter(|e| is_plausible_key(&e.key)) {
        let prefixed = is_public_prefixed(&entry.key);
        let ty = infer_type(&entry.key, &entry.value);
        // A bundler prefix says who may read the key, never that the value is
        // dull: NEXT_PUBLIC_SUPABASE_ANON_KEY is still a key.
        let credential = names_a_credential(&entry.key);
        let copied = !entry.value.is_empty() && !credential && (prefixed || is_dull(&entry.value));
        let default = copied.then(|| entry.value.clone());
        // The prefix decides sensitivity: the value reaches the browser either way.
        let sensitive = !prefixed && !copied;
        schema.keys.push(Key {
            name: entry.key.clone(),
            ty,
            required: default.is_none(),
            sensitive,
            // Written only where the prefix rule alone would not reach the same answer.
            sensitive_decorator: (!prefixed && !sensitive).then_some(false),
            default,
            ..Key::default()
        });
    }
    schema
}

/// A key that says what it holds keeps its value out of the schema however dull
/// the value looks: `STRIPE_SECRET_KEY=sk_test_0000` reads as a slug otherwise.
fn names_a_credential(key: &str) -> bool {
    const NAMES: [&str; 16] = [
        "AUTH",
        "KEY",
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "PASSPHRASE",
        "PASS",
        "PWD",
        "PW",
        "CREDENTIAL",
        "CRED",
        "DSN",
        "SALT",
        "SEED",
        "SIGNATURE",
    ];
    // The word itself or its plural, in both spellings: PASSES is PASS.
    key.split('_').any(|part| {
        NAMES.contains(&part)
            || ["ES", "S"].iter().any(|plural| {
                part.strip_suffix(plural)
                    .is_some_and(|single| NAMES.contains(&single))
            })
    })
}

/// A value the committed schema may carry: nothing here can be a credential.
fn is_dull(value: &str) -> bool {
    parse_boolean(value).is_some()
        || value.parse::<i64>().is_ok()
        || is_lowercase_word(value)
        || is_lowercase_slug(value)
        || is_loopback_url(value)
}

fn is_lowercase_word(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_lowercase())
}

/// `us-east-1`, `gpt-4o`, `api.internal`. One segment is letters only and a
/// segment carrying a digit stays short, so `a3f9c2d4e5b6` falls out here.
fn is_lowercase_slug(value: &str) -> bool {
    if value.len() > 32 || !value.starts_with(|c: char| c.is_ascii_lowercase()) {
        return false;
    }
    let segments: Vec<&str> = value.split(['-', '_', '.']).collect();
    let shaped = segments.iter().all(|segment| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && (segment.len() <= 4 || !segment.chars().any(|c| c.is_ascii_digit()))
    });
    shaped
        && segments
            .iter()
            .any(|segment| segment.chars().all(|c| c.is_ascii_lowercase()))
}

/// `http://localhost:3000` and nothing that could carry a credential in it.
fn is_loopback_url(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https") || rest.contains('?') {
        return false;
    }
    let authority = rest.split(['/', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return false;
    }
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    matches!(host, "localhost" | "127.0.0.1")
}

/// The type `init` reads out of one pair. `set` uses it for a key the schema
/// does not list yet, without ever copying the value.
pub fn infer_type(key: &str, value: &str) -> Type {
    // TRANSPORT and SUPPORT end in PORT too.
    let port = key == "PORT" || key.ends_with("_PORT");
    if value.is_empty() {
        return Type::new(if port {
            BaseType::Port
        } else {
            BaseType::String
        });
    }
    if is_absolute_url(value) {
        return Type::new(BaseType::Url);
    }
    if port && matches!(value.parse::<u32>(), Ok(n) if (1..=65535).contains(&n)) {
        return Type::new(BaseType::Port);
    }
    // Before the boolean check, which reads 0 and 1 as false and true.
    if value.parse::<i64>().is_ok() {
        let mut ty = Type::new(BaseType::Number);
        ty.constraints.push(("isInt".into(), "true".into()));
        return ty;
    }
    if parse_boolean(value).is_some() {
        return Type::new(BaseType::Boolean);
    }
    if matches!(value.parse::<f64>(), Ok(n) if n.is_finite()) {
        return Type::new(BaseType::Number);
    }
    if is_email(value) {
        return Type::new(BaseType::Email);
    }
    Type::new(BaseType::String)
}
