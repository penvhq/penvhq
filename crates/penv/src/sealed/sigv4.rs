//! AWS Signature Version 4, recomputed. The command signs with a placeholder
//! secret access key, so the signature it sends is wrong; the proxy computes it
//! again over the request as it leaves, with the real key. The key itself never
//! goes on the wire, only a signature valid for that one request.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

use super::http::{Head, raw};

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(key).expect("hmac takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether this request carries a SigV4 header signature.
pub fn is_signed(head: &Head) -> bool {
    head.get("authorization")
        .is_some_and(|v| v.starts_with("AWS4-HMAC-SHA256 "))
}

/// The payload hash the request declares in `x-amz-content-sha256` (S3 always
/// sends one): `Ok(Some)` to sign with, `Ok(None)` when there is none and the
/// body must be hashed. An upload whose chunks are signed one by one
/// (`STREAMING-AWS4-HMAC-SHA256-...`) cannot be re-signed here and is an error;
/// unsigned streaming (`STREAMING-UNSIGNED-PAYLOAD-TRAILER`, what current SDKs
/// send for S3 uploads) signs the header alone and passes.
pub fn declared_hash(head: &Head) -> Result<Option<String>, &'static str> {
    match head.get("x-amz-content-sha256").map(str::trim) {
        Some(v) if v.starts_with("STREAMING-AWS4-HMAC-SHA256") => {
            Err("a streaming AWS upload whose chunks are signed one by one")
        }
        Some(v) => Ok(Some(v.to_string())),
        None => Ok(None),
    }
}

/// Replace the signature in `head`'s Authorization with one made with `secret`.
/// Returns the new signature.
pub fn resign(head: &mut Head, payload_hash: &str, secret: &str) -> Result<String, &'static str> {
    let auth = head
        .get("authorization")
        .ok_or("no Authorization header")?
        .to_string();
    let rest = auth
        .strip_prefix("AWS4-HMAC-SHA256 ")
        .ok_or("not a SigV4 Authorization")?;
    let mut credential = None;
    let mut signed = None;
    for part in rest.split(',') {
        match part.trim().split_once('=') {
            Some(("Credential", v)) => credential = Some(v.to_string()),
            Some(("SignedHeaders", v)) => signed = Some(v.to_string()),
            _ => {}
        }
    }
    let credential = credential.ok_or("no Credential")?;
    let signed = signed.ok_or("no SignedHeaders")?;
    let scope_parts: Vec<&str> = credential.splitn(2, '/').collect();
    let scope = scope_parts
        .get(1)
        .ok_or("a Credential with no scope")?
        .to_string();
    let fields: Vec<&str> = scope.split('/').collect();
    let [date, region, service, "aws4_request"] = fields.as_slice() else {
        return Err("a credential scope that is not date/region/service/aws4_request");
    };
    let amz_date = head
        .get("x-amz-date")
        .or_else(|| head.get("date"))
        .ok_or("no X-Amz-Date")?
        .trim()
        .to_string();

    let mut parts = head.start.split_whitespace();
    let method = parts.next().ok_or("no method")?;
    let target = parts.next().ok_or("no target")?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let canonical_uri = if *service == "s3" {
        path.to_string()
    } else {
        encode(&raw(path), false)
    };

    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (encode(&decode(k), true), encode(&decode(v), true))
        })
        .collect();
    pairs.sort();
    let canonical_query: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();

    let mut canonical_headers = String::new();
    for name in signed.split(';') {
        let values: Vec<String> = head
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| {
                v.split([' ', '\t'])
                    .filter(|w| !w.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        if values.is_empty() {
            return Err("a signed header the request does not carry");
        }
        canonical_headers.push_str(&format!("{name}:{}\n", values.join(",")));
    }
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{}\n{canonical_headers}\n{signed}\n{payload_hash}",
        canonical_query.join("&")
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(&raw(&canonical_request))
    );
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex(&hmac(&k_signing, string_to_sign.as_bytes()));
    head.set(
        "Authorization",
        format!("AWS4-HMAC-SHA256 Credential={credential}, SignedHeaders={signed}, Signature={signature}"),
    );
    Ok(signature)
}

/// The signature a SigV4 Authorization header carries.
pub fn signature(head: &Head) -> Option<String> {
    let auth = head.get("authorization")?;
    let (_, rest) = auth.rsplit_once("Signature=")?;
    Some(rest.split(',').next().unwrap_or("").trim().to_string())
}

/// SigV4's URI encoding: unreserved characters as they are, everything else
/// `%XX` in upper case; `/` too when `slash` is set (query parts).
fn encode(bytes: &[u8], slash: bool) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) || (!slash && b == b'/') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn decode(text: &str) -> Vec<u8> {
    let bytes = raw(text);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(Ok(b)) = bytes
                .get(i + 1..i + 3)
                .map(|h| u8::from_str_radix(std::str::from_utf8(h).unwrap_or("zz"), 16))
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sealed::http::read_head;
    use std::io::Cursor;

    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    fn head(text: &str) -> Head {
        read_head(&mut Cursor::new(text.as_bytes().to_vec()))
            .unwrap()
            .unwrap()
    }

    fn signature(h: &Head) -> String {
        h.get("authorization")
            .unwrap()
            .rsplit("Signature=")
            .next()
            .unwrap()
            .to_string()
    }

    // AWS's published SigV4 test suite, "get-vanilla" and "get-vanilla-query-order-key-case".
    #[test]
    fn the_aws_test_suite_vectors_match() {
        let mut h = head(
            "GET / HTTP/1.1\r\nHost: example.amazonaws.com\r\nX-Amz-Date: 20150830T123600Z\r\nAuthorization: AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=placeholder\r\n\r\n",
        );
        assert_eq!(super::signature(&h).as_deref(), Some("placeholder"));
        let made = resign(&mut h, &sha256_hex(b""), SECRET).unwrap();
        assert_eq!(
            signature(&h),
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        assert_eq!(made, signature(&h));

        let mut h = head(
            "GET /?Param2=value2&Param1=value1 HTTP/1.1\r\nHost: example.amazonaws.com\r\nX-Amz-Date: 20150830T123600Z\r\nAuthorization: AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=placeholder\r\n\r\n",
        );
        resign(&mut h, &sha256_hex(b""), SECRET).unwrap();
        assert_eq!(
            signature(&h),
            "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
        );
    }

    #[test]
    fn a_chunk_signed_upload_is_not_resigned_and_a_missing_signed_header_is_an_error() {
        let h = head(
            "PUT /b/k HTTP/1.1\r\nx-amz-content-sha256: STREAMING-AWS4-HMAC-SHA256-PAYLOAD\r\n\r\n",
        );
        assert!(declared_hash(&h).is_err());
        let h = head(
            "PUT /b/k HTTP/1.1\r\nx-amz-content-sha256: STREAMING-UNSIGNED-PAYLOAD-TRAILER\r\n\r\n",
        );
        assert_eq!(
            declared_hash(&h).unwrap().as_deref(),
            Some("STREAMING-UNSIGNED-PAYLOAD-TRAILER")
        );
        assert_eq!(
            declared_hash(&head("POST / HTTP/1.1\r\n\r\n")).unwrap(),
            None
        );
        let mut h = head(
            "GET / HTTP/1.1\r\nHost: x\r\nX-Amz-Date: 20150830T123600Z\r\nAuthorization: AWS4-HMAC-SHA256 Credential=AK/20150830/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-meta, Signature=p\r\n\r\n",
        );
        assert!(resign(&mut h, "x", SECRET).is_err());
    }
}
