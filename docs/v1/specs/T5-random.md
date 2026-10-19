# T5 random (sonnet)

Spec: docs/Functions.md:97-111; Design.md:117. Contract: `random::spec` in CONTRACT.md.

## Build
- `crates/penv-schema/src/functions/random.rs`: pure `spec(text)` parsing `random(kind=, length=)`.
  `kind` default `alnum`; `length` 8..=512 required except `uuid`, which refuses it (:107-108).
  `random(32)` fails with a message telling to write `random(length=32)` (:111).
- `crates/penv/src/source.rs` `randoms`/`random_text`/`random_len` (~lines 750-850): use
  `spec`; generate per kind from `penv_cloud::random_bytes`: alnum (existing rejection sampling),
  hex, base64url (unpadded, exactly `length` chars), uuid v4 (RFC 9562 version/variant bits).
  Keep the `.env.local` / `.env.test.local` storage and pending behaviour unchanged (:110).
- Update tests that write `random(32)` to `random(length=32)`: crates/penv/tests/run.rs:700,881,
  crates/penv/tests/encrypt.rs:212,223 (and any other `rg -n 'random\(' crates`).
- push.rs filter (`starts_with("random(")`) needs no change; confirm.

## Tests
- Unit tests in random.rs: defaults, each kind, bounds 7/8/512/513, uuid with length fails,
  unknown kind fails, `random(32)` message names `length=32`.
- Binary: generated hex is lowercase hex of the asked length; uuid matches the v4 shape;
  second run reads back the same value; `check` lists it pending and writes nothing.
