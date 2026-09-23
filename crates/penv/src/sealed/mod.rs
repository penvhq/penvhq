//! A sealed run: the child holds placeholders for keys with `@hosts`, and the
//! values go into its requests on the way out, through a proxy in this process.

pub mod ca;
pub mod http;
pub mod proxy;
pub mod run;

/// Why a key looks like it signs requests rather than sending itself, from its
/// name. A placeholder cannot stand in for such a key: the signature is made
/// in the command, with whatever value it holds.
pub fn signing_secret(name: &str) -> Option<&'static str> {
    let upper = name.to_ascii_uppercase();
    if upper == "AWS_SECRET_ACCESS_KEY" || upper.ends_with("_AWS_SECRET_ACCESS_KEY") {
        return Some("AWS signs every request with its secret access key (SigV4)");
    }
    for (word, why) in [
        ("SIGNING", "its name says it signs"),
        ("HMAC", "its name says it computes HMACs"),
        (
            "WEBHOOK_SECRET",
            "a webhook secret verifies or signs payloads",
        ),
        ("JWT_SECRET", "a JWT secret signs tokens"),
        ("JWT_PRIVATE_KEY", "a JWT key signs tokens"),
    ] {
        if upper.contains(word) {
            return Some(why);
        }
    }
    None
}
