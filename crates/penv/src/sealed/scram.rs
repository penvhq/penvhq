//! The client side of SCRAM-SHA-256 (RFC 5802, RFC 7677) and Postgres's MD5
//! password hash: the two ways a Postgres server asks for a password that are
//! not plain text. Pure functions over bytes.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(key).expect("hmac takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// PBKDF2-HMAC-SHA256 for one 32-byte block: SCRAM's `Hi`.
fn hi(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut first = salt.to_vec();
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac(password, &first);
    let mut out = u;
    for _ in 1..iterations {
        u = hmac(password, &u);
        for (o, b) in out.iter_mut().zip(u.iter()) {
            *o ^= b;
        }
    }
    out
}

pub struct Scram {
    password: String,
    client_first_bare: String,
    nonce: String,
    auth_message: String,
    server_signature: [u8; 32],
}

impl Scram {
    /// `nonce` is printable random text the caller makes.
    pub fn new(password: &str, nonce: &str) -> Scram {
        Scram {
            password: password.to_string(),
            client_first_bare: format!("n=,r={nonce}"),
            nonce: nonce.to_string(),
            auth_message: String::new(),
            server_signature: [0; 32],
        }
    }

    /// The client-first message, gs2 header included.
    pub fn first(&self) -> String {
        format!("n,,{}", self.client_first_bare)
    }

    /// The client-final message for the server-first one.
    pub fn respond(&mut self, server_first: &str) -> Result<String, &'static str> {
        let mut nonce = None;
        let mut salt = None;
        let mut iterations = None;
        for part in server_first.split(',') {
            match part.split_once('=') {
                Some(("r", v)) => nonce = Some(v.to_string()),
                Some(("s", v)) => salt = penv_cloud::b64::decode(v),
                Some(("i", v)) => iterations = v.parse::<u32>().ok(),
                _ => {}
            }
        }
        let (Some(nonce), Some(salt), Some(iterations)) = (nonce, salt, iterations) else {
            return Err("a SCRAM server-first message penv cannot read");
        };
        if !nonce.starts_with(&self.nonce) || nonce.len() <= self.nonce.len() {
            return Err("a SCRAM nonce that does not extend penv's");
        }
        if !(1..=10_000_000).contains(&iterations) {
            return Err("a SCRAM iteration count out of range");
        }
        let salted = hi(self.password.as_bytes(), &salt, iterations);
        let client_key = hmac(&salted, b"Client Key");
        let stored_key: [u8; 32] = Sha256::digest(client_key).into();
        let without_proof = format!("c=biws,r={nonce}");
        self.auth_message = format!("{},{server_first},{without_proof}", self.client_first_bare);
        let signature = hmac(&stored_key, self.auth_message.as_bytes());
        let proof: Vec<u8> = client_key
            .iter()
            .zip(signature.iter())
            .map(|(a, b)| a ^ b)
            .collect();
        let server_key = hmac(&salted, b"Server Key");
        self.server_signature = hmac(&server_key, self.auth_message.as_bytes());
        Ok(format!(
            "{without_proof},p={}",
            penv_cloud::b64::encode(&proof)
        ))
    }

    /// Whether the server-final message proves the server knew the password.
    pub fn verify(&self, server_final: &str) -> bool {
        server_final
            .strip_prefix("v=")
            .and_then(penv_cloud::b64::decode)
            .is_some_and(|v| v == self.server_signature)
    }
}

/// Postgres's MD5 answer: `md5` + md5hex(md5hex(password + user) + salt).
pub fn md5_password(password: &str, user: &str, salt: &[u8; 4]) -> String {
    let inner = hex(&md5(format!("{password}{user}").as_bytes()));
    let mut outer = inner.into_bytes();
    outer.extend_from_slice(salt);
    format!("md5{}", hex(&md5(&outer)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// MD5 (RFC 1321). Only for Postgres's legacy password exchange.
pub fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64)
        .map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32)
        .collect();
    let (mut a0, mut b0, mut c0, mut d0) =
        (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = input.to_vec();
    let bits = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_le_bytes());
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk
            .chunks(4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
            .collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, v) in [a0, b0, c0, d0].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_matches_the_rfc_vectors() {
        assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(&md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex(&md5(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            )),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    #[test]
    fn scram_matches_rfc_7677() {
        // RFC 7677 section 3: user "user", password "pencil".
        let mut scram = Scram::new("pencil", "rOprNGfwEbeRWgbNEkqO");
        scram.client_first_bare = "n=user,r=rOprNGfwEbeRWgbNEkqO".into();
        let server_first = "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
        let final_message = scram.respond(server_first).unwrap();
        assert_eq!(
            final_message,
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );
        assert!(scram.verify("v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4="));
        assert!(!scram.verify("v=AAAA"));
    }

    #[test]
    fn a_server_nonce_that_does_not_extend_ours_is_refused() {
        let mut scram = Scram::new("pw", "abc");
        assert!(scram.respond("r=xyz123,s=AAAA,i=4096").is_err());
        assert!(scram.respond("r=abc,s=AAAA,i=4096").is_err());
    }
}
