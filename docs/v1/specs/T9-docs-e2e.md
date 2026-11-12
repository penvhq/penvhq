# T9 docs-e2e (haiku)

Close v1 out. Read docs/v1/ORCHESTRATOR.md decisions and the Log in HANDOFF.md first.

## Build
- CHANGELOG.md: add `## Unreleased` above 1.0.0-rc.3 (decision 14) with `### Added` (url, json,
  hash, random options, public lists, computed @hosts, --pin-hosts, b64d, editor arguments) and
  `### Breaking` (`random(N)` is a validation failure; write `random(length=N)`). Match the
  existing entry style: bold lead sentence, one short paragraph.
- docs/Design.md:110: the filters comment gains `b64d` (decision 11). No other Design edit
  unless a merged task contradicts it; then report, do not rewrite.
- crates/penv/tests/functions.rs: one end-to-end schema using every example in Functions.md
  (fake values only), run through `penv check` and `penv ls --json`: results and sensitivity
  match the doc comments. Assert no sensitive fake value appears in any output.

## Acceptance
- `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --all -- --check`
  all pass (this task runs the workspace-wide set once).
- The fixture schema still renders through every target (`rg -n fixture crates/penv/tests | head`
  to find the existing test) and `penv help --json` validates against docs/manifest.schema.json.
