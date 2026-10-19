//! `hash()`, docs/Functions.md.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256, Sha512};

use super::Call;

type Run = fn(data: &[u8], key: Option<&[u8]>) -> Vec<u8>;
type Encode = fn(&[u8]) -> String;

fn sha256(data: &[u8], _: Option<&[u8]>) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

fn sha512(data: &[u8], _: Option<&[u8]>) -> Vec<u8> {
    Sha512::digest(data).to_vec()
}

fn hmac_sha256(data: &[u8], key: Option<&[u8]>) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.unwrap_or_default()).expect("any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hmac_sha512(data: &[u8], key: Option<&[u8]>) -> Vec<u8> {
    let mut mac = Hmac::<Sha512>::new_from_slice(key.unwrap_or_default()).expect("any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Vocabulary `algorithms` name, whether it signs with a key, the function.
const ALGORITHMS: &[(&str, bool, Run)] = &[
    ("sha256", false, sha256),
    ("sha512", false, sha512),
    ("hmac-sha256", true, hmac_sha256),
    ("hmac-sha512", true, hmac_sha512),
];

/// Vocabulary `encodings` name and the writer.
const ENCODINGS: &[(&str, Encode)] = &[
    ("hex", hex),
    ("base64", |b| base64(b, b"+/", true)),
    ("base64url", |b| base64(b, b"-_", false)),
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn base64(bytes: &[u8], extra: &[u8; 2], pad: bool) -> String {
    let table: Vec<u8> = (b'A'..=b'Z')
        .chain(b'a'..=b'z')
        .chain(b'0'..=b'9')
        .chain(extra.iter().copied())
        .collect();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(table[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
        if pad {
            out.extend(std::iter::repeat_n('=', 3 - chunk.len()));
        }
    }
    out
}

pub fn eval(call: &Call) -> Result<String, String> {
    let option = |name: &str| {
        call.options
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    };
    let algo = option("algo").ok_or("hash() needs algo=; write algo=sha256")?;
    let &(_, signs, run) = ALGORITHMS
        .iter()
        .find(|(n, _, _)| *n == algo)
        .ok_or("hash() algo is not offered; use sha256, sha512, hmac-sha256 or hmac-sha512")?;
    let key = option("key");
    match (signs, key) {
        (true, None) => return Err(format!("hash() algo {algo} needs key=; add key=$SECRET")),
        (false, Some(_)) => {
            return Err(format!(
                "hash() key= is for hmac-* only; drop it for {algo}"
            ));
        }
        _ => {}
    }
    let encoding = option("encoding").unwrap_or("hex");
    let &(_, encode) = ENCODINGS
        .iter()
        .find(|(n, _)| *n == encoding)
        .ok_or("hash() encoding is not offered; use hex, base64 or base64url")?;
    let length = match option("length") {
        None => None,
        Some(n) => match n.parse::<usize>() {
            Ok(v) if v >= 1 && n.bytes().all(|b| b.is_ascii_digit()) => Some(v),
            _ => {
                return Err(
                    "hash() length must be a whole number of at least 1; write length=8".into(),
                );
            }
        },
    };
    let data = call
        .subject
        .as_deref()
        .ok_or("hash() needs a subject first; write hash($KEY, ...)")?;
    let text = encode(&run(data.as_bytes(), key.map(str::as_bytes)));
    Ok(match length {
        Some(n) => text.chars().take(n).collect(),
        None => text,
    })
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
        ];
        for (input, want) in cases {
            assert_eq!(base64(input.as_bytes(), b"+/", true), want, "{input:?}");
        }
    }

    #[test]
    fn base64url_drops_padding() {
        assert_eq!(base64(b"foob", b"-_", false), "Zm9vYg");
        assert_eq!(base64(b"", b"-_", false), "");
    }
}
