//! What penv.cloud accepts on a write, checked before anything is sent so a
//! refusal arrives before the upload rather than halfway through it.

use std::fmt;

use crate::api::CloudKey;
use crate::error::{is_format, printable};

pub const MAX_NAME: usize = 255;
pub const MAX_SEGMENT: usize = 255;
pub const MAX_PATH: usize = 1024;
/// 256 KiB, counted in UTF-8 bytes.
pub const MAX_VALUE: usize = 256 * 1024;
/// Keys in one `PUT`; a larger push goes in several.
pub const MAX_BATCH: usize = 1000;

/// Characters no path segment may hold, besides control, whitespace, bidi and
/// zero-width characters.
const PATH_FORBIDDEN: &str = "\\$`\"'=#;&|<>(){}*?!";

/// One key penv.cloud would refuse. It names the key and never its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Name { address: String, why: &'static str },
    Path { address: String, why: &'static str },
    TooLarge { address: String, bytes: usize },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Name { address, why } => write!(f, "the name of {address} {why}"),
            Refusal::Path { address, why } => write!(f, "the path of {address} {why}"),
            Refusal::TooLarge { address, bytes } => write!(
                f,
                "the value of {address} is {bytes} bytes, and penv.cloud stores at most {MAX_VALUE} (256 KiB)"
            ),
        }
    }
}

/// `^[A-Za-z0-9_][A-Za-z0-9_.-]*$`, at most [`MAX_NAME`] characters.
pub fn check_name(name: &str) -> Result<(), &'static str> {
    let mut chars = name.chars();
    match chars.next() {
        None => return Err("is empty"),
        Some(c) if !(c.is_ascii_alphanumeric() || c == '_') => {
            return Err("must start with a letter, a digit or _");
        }
        _ => {}
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')) {
        return Err("may hold only letters, digits, _, . and - (no /)");
    }
    if name.len() > MAX_NAME {
        return Err("is longer than 255 characters");
    }
    Ok(())
}

/// Empty, or `/`-joined segments: none empty, `.` or `..`, each at most
/// [`MAX_SEGMENT`] characters and the whole at most [`MAX_PATH`].
pub fn check_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() {
        return Ok(());
    }
    if path.chars().count() > MAX_PATH {
        return Err("is longer than 1024 characters");
    }
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err("has an empty segment (a leading, trailing or doubled /)");
        }
        if segment == "." || segment == ".." {
            return Err("has a . or .. segment");
        }
        if segment.chars().count() > MAX_SEGMENT {
            return Err("has a segment longer than 255 characters");
        }
        if segment.chars().any(|c| {
            c.is_control() || c.is_whitespace() || is_format(c) || PATH_FORBIDDEN.contains(c)
        }) {
            return Err(
                "holds a space, a control or invisible character, or one of \\ $ ` \" ' = # ; & | < > ( ) { } * ? !",
            );
        }
    }
    Ok(())
}

/// One key as penv.cloud would judge it.
pub fn check(key: &CloudKey) -> Result<(), Refusal> {
    let address = || printable(&key.address());
    check_path(&key.path).map_err(|why| Refusal::Path {
        address: address(),
        why,
    })?;
    check_name(&key.name).map_err(|why| Refusal::Name {
        address: address(),
        why,
    })?;
    match &key.value {
        Some(value) if value.len() > MAX_VALUE => Err(Refusal::TooLarge {
            address: address(),
            bytes: value.len(),
        }),
        _ => Ok(()),
    }
}

/// Every key, so one refusal lists them all.
pub fn check_all(keys: &[CloudKey]) -> Vec<Refusal> {
    keys.iter().filter_map(|key| check(key).err()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(path: &str, name: &str, value: Option<String>) -> CloudKey {
        CloudKey {
            path: path.into(),
            name: name.into(),
            value,
            ..CloudKey::default()
        }
    }

    #[test]
    fn a_name_is_letters_digits_underscore_dot_and_dash() {
        for name in [
            "DATABASE_URL",
            "stripe.key",
            "1PASSWORD",
            "_x",
            "a-b",
            &"A".repeat(255),
        ] {
            assert_eq!(check_name(name), Ok(()), "{name}");
        }
        for name in [
            "",
            ".env",
            "-x",
            "a/b",
            "a b",
            "a=b",
            "é",
            "a\u{200B}",
            &"A".repeat(256),
        ] {
            assert!(check_name(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn a_path_is_segments_with_nothing_a_shell_or_a_reader_would_trip_on() {
        for path in ["", "db", "db/primary", "a.b/c-d/e_f", &"p".repeat(255)] {
            assert_eq!(check_path(path), Ok(()), "{path}");
        }
        let long = vec!["p".repeat(200); 6].join("/");
        for path in [
            "/db",
            "db/",
            "db//x",
            ".",
            "db/..",
            "a b",
            "a\tb",
            "a\u{202E}b",
            "a\u{200B}b",
            "a\u{7}b",
            &"p".repeat(256),
            &long,
        ] {
            assert!(check_path(path).is_err(), "{path:?}");
        }
        for c in PATH_FORBIDDEN.chars() {
            assert!(check_path(&format!("a{c}b")).is_err(), "{c}");
        }
    }

    #[test]
    fn a_value_is_measured_in_utf8_bytes() {
        assert_eq!(check(&key("", "K", Some("a".repeat(MAX_VALUE)))), Ok(()));
        // 'é' is two bytes, so this is one byte over with half the characters.
        let over = "é".repeat(MAX_VALUE / 2) + "a";
        assert_eq!(
            check(&key("db", "K", Some(over))),
            Err(Refusal::TooLarge {
                address: "db/K".into(),
                bytes: MAX_VALUE + 1
            })
        );
    }

    #[test]
    fn a_refusal_names_the_key_and_never_its_value() {
        let refused = check_all(&[
            key("", "OK", Some("fine".into())),
            key("", "BIG", Some("FAKE".repeat(MAX_VALUE))),
            key("a b", "K", None),
            key("", "a/b", None),
        ]);
        assert_eq!(refused.len(), 3);
        let text: Vec<String> = refused.iter().map(ToString::to_string).collect();
        assert!(
            text[0].starts_with("the value of BIG is 1048576 bytes"),
            "{}",
            text[0]
        );
        assert!(text[1].starts_with("the path of a b/K"), "{}", text[1]);
        assert!(text[2].starts_with("the name of a/b"), "{}", text[2]);
        assert!(text.iter().all(|t| !t.contains("FAKE")));
    }
}
