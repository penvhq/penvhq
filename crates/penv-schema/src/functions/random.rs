//! `random()`, docs/Functions.md. `penv run` generates the value; this reads the call.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RandomKind {
    Alnum,
    Hex,
    Base64Url,
    Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomSpec {
    pub kind: RandomKind,
    pub length: Option<usize>,
}

/// `None` when the text is not a `random()` call.
pub fn spec(text: &str) -> Result<Option<RandomSpec>, String> {
    let text = text.trim();
    let Some(inner) = text
        .strip_prefix("random(")
        .and_then(|t| t.strip_suffix(')'))
    else {
        return Ok(None);
    };
    let mut kind = RandomKind::Alnum;
    let mut length: Option<usize> = None;
    for part in inner.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let Some((name, value)) = part.split_once('=') else {
            return Err(if part.chars().all(|c| c.is_ascii_digit()) {
                format!("random() takes options as name=value; write random(length={part})")
            } else {
                "random() takes options as name=value; write random(kind=hex, length=32)".into()
            });
        };
        let (name, value) = (name.trim(), value.trim());
        match name {
            "kind" => {
                kind = match value {
                    "alnum" => RandomKind::Alnum,
                    "hex" => RandomKind::Hex,
                    "base64url" => RandomKind::Base64Url,
                    "uuid" => RandomKind::Uuid,
                    _ => {
                        return Err(
                            "random() option `kind` is alnum, hex, base64url or uuid".into()
                        );
                    }
                }
            }
            "length" => {
                length = Some(value.parse().map_err(|_| {
                    "random() option `length` is a whole number; write length=32".to_string()
                })?);
            }
            _ => {
                return Err(format!(
                    "random() has no option `{name}`; use kind or length"
                ));
            }
        }
    }
    match (kind, length) {
        (RandomKind::Uuid, Some(_)) => {
            Err("random() option `length` does not apply to kind=uuid; remove it".into())
        }
        (RandomKind::Uuid, None) => Ok(Some(RandomSpec { kind, length })),
        (_, Some(n)) if (8..=512).contains(&n) => Ok(Some(RandomSpec { kind, length })),
        (_, Some(_)) => Err("random() option `length` is 8 to 512; write length=32".into()),
        (_, None) => Err("random() needs a length; write random(length=32)".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(t: &str) -> RandomSpec {
        spec(t).unwrap().unwrap()
    }

    #[test]
    fn defaults_and_kinds() {
        assert_eq!(ok("random(length=32)").kind, RandomKind::Alnum);
        assert_eq!(ok("random(kind=hex, length=16)").kind, RandomKind::Hex);
        assert_eq!(
            ok("random(length=9, kind=base64url)").kind,
            RandomKind::Base64Url
        );
        assert_eq!(ok("random(kind=uuid)").length, None);
        assert_eq!(spec("hello").unwrap(), None);
    }

    #[test]
    fn length_bounds() {
        assert!(spec("random(length=7)").is_err());
        assert_eq!(ok("random(length=8)").length, Some(8));
        assert_eq!(ok("random(length=512)").length, Some(512));
        assert!(spec("random(length=513)").is_err());
        assert!(spec("random()").is_err());
    }

    #[test]
    fn rejections() {
        assert!(spec("random(kind=uuid, length=32)").is_err());
        assert!(spec("random(kind=nope, length=32)").is_err());
        assert!(spec("random(size=32)").is_err());
        assert!(spec("random(32)").unwrap_err().contains("length=32"));
    }
}
