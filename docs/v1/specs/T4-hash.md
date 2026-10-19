# T4 hash (sonnet)

Fill `crates/penv-schema/src/functions/hash.rs` (stub from T1). Spec: docs/Functions.md:77-95.

## Build
- Move `sha2` and `hmac` to `[workspace.dependencies]` in /Cargo.toml; penv-cloud and
  penv-schema use `.workspace = true` (decision 7). No other dependency change.
- Algorithms and encodings dispatch from a table keyed by the vocabulary names
  (`algorithms`, `encodings`, Functions.md:149): adding one is a table row plus a vocabulary entry.
- `algo` required; `encoding` default hex; `length` >= 1 truncates the encoded text; `key`
  required for `hmac-*` and refused otherwise (:93). Subject is the data for HMAC.
- `md5`/`sha1` rejected as unknown values (:94). base64 is standard padded; base64url unpadded.
- Taint: `Inputs` (subject or key sensitive -> sensitive, :95); a key listed in
  `policy.hash_public` whose value is a hash() call is `Public` (Functions.md:121, :130).

## Tests (crates/penv-schema/tests/hash.rs)
- `hash($API_KEY, algo=sha256, length=8)` with `fake_api_key_123` gives `604d6582` (:80).
- Known vectors (RFC 4231 case 1 for hmac-sha256/512; sha256("abc")) in each encoding.
- `hash($K, algo=hmac-sha256)` without key fails; `key=` with sha256 fails; `algo=md5` fails.
- `length=0` fails; length longer than the result returns all of it.
- Taint: key sensitive -> result tainted; hash_public declassifies.
