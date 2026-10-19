# T7 computed-hosts (opus)

Spec: docs/Functions.md:44 (read it whole); `[hosts]` at :125-126, :131.

## Build
- crates/penv-schema/src/parse.rs (hosts arm ~line 599) and ir.rs: a `@hosts=` value that is a
  call (`url(...)`) is stored as `Key.hosts_expr` (CONTRACT.md) instead of a host list.
- crates/penv/src/source.rs or check.rs: compute each `hosts_expr` the way asserts are computed
  (source.rs:595-597 names them `\0assert{i}`); result is a host list (comma-split).
- crates/penv/src/config.rs: `[hosts]` accessor and setter, written with toml_edit so comments
  and order survive (decision 13).
- `check`: fail when a computed `@hosts` key has no pin or its hosts differ, naming the key and
  both lists (hosts are public). `check --pin-hosts` (cli.rs Check args) writes pins and prints
  only the hosts that changed. No other command writes `[hosts]`.
- crates/penv/src/sealed/run.rs (~line 38): a sealed run refuses the same mismatch, same message.
  With a matching pin, the computed hosts are what the proxy allows.
- `penv help --json` publishes `--pin-hosts`; tests/manifest.rs and docs/manifest.schema.json
  still validate (update snapshots under tests/snapshots if they list flags).

## Tests (crates/penv/tests/hosts.rs; add a case to sealed.rs)
- No pin -> check fails naming the key; `--pin-hosts` writes it, keeps an existing comment,
  prints the host; second `--pin-hosts` prints nothing.
- Changed DATABASE_URL host -> check fails listing pinned and computed hosts.
- Sealed run refuses on mismatch; runs with a pin.
- Literal `@hosts=api.stripe.com` behaves exactly as before.
