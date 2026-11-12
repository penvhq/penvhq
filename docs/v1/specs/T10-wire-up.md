# T10 wire-up (follow-ups from group B reviews)

Touch only: crates/penv-schema/src/{resolve.rs,functions/args.rs}, crates/penv-schema/tests/{json.rs,url.rs}, crates/penv/Cargo.toml (Cargo.lock as side effect).

1. `fallback()` in resolve.rs propagates function errors with `?` and only skips empty values.
   Make a function error inside a fallback arm fall through to the next arm (docs/Functions.md: grep "fallback" for the exact rule; if the doc is silent, an error falls through and the last arm's error is reported).
   Un-ignore `fallback_supplies_a_default` in tests/json.rs.
2. json duplicate keys: `functions::json::duplicate_keys(text) -> Vec<String>` exists. When resolve evaluates `json()` on a subject, push a soft (warning, non-fatal) ResolveError naming the key, never the value (Functions.md:51-75, grep "duplicate").
3. args.rs `check_args`: call `functions::url::check_password(args)` when the function is `url`, so a literal `password=...` is refused at parse. Add a test in tests/url.rs.
4. crates/penv/Cargo.toml: `hmac` and `sha2` use `.workspace = true` (versions are identical).

Acceptance: cargo check --workspace --tests, cargo test -p penv-schema, clippy -D warnings, fmt --check all pass.
