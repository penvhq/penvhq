# v1 orchestration

## Scope
v1 = shipped 1.0.0-rc.3 plus everything docs/Functions.md (PR #107) specifies. Phases 1-5 of
docs/Design.md §12 have landed (commands, cloud, sealed runs, bundles, encrypt, lsp); the gap is
Functions.md, which no code implements yet:
- the argument form (`name($S, address, opt=v)`), filters with arguments, `b64d`
- `url`, `json` (+ `raw()`), `hash`, `random(kind=, length=)`; `random(N)` becomes a failure
- part-level taint and the `public` lists in `.penv/config.toml` (`[url]`, `[json.KEY]`, `[hash]`)
- computed `@hosts` pinned in `[hosts]`, `penv check --pin-hosts`, sealed-run refusal
- vocabulary argument entries and editor completions/diagnostics over them
- `check` noting `url()`/`json()`/`hash()` as penv-only (Design.md:120)

## Out of scope
The penvhq-cloud repo; anything not written in docs/; version bumps and releases; new targets/guards.

## Execution
- Integration branch: `claude/v1-integration-ygsja5`. Tasks branch `v1/T<n>-<slug>` from it, merge back.
- Final PR: integration branch -> `development`.
- Model ceiling: opus at medium effort at most; prefer haiku/sonnet (per task in HANDOFF.md).
- Baseline: `cargo check --workspace` passes on eb490a8 (1m07s, no warnings shown).

## Decisions
1. Functions live in `crates/penv-schema/src/functions/` (one file each): they are pure cores.
2. T1 owns the parser, dispatch, `Policy` and stubs, so T2-T5/T8 touch disjoint files.
3. Taint keeps the existing deps/`control` mechanism: a public read evaluates its subject as
   control; `password` adds the dep `ALWAYS_SENSITIVE`, which `tainted()` treats as sensitive.
4. `resolve_full` gains a `&Policy` argument (no second entry point: one concept, one name).
5. JSON uses a small hand-written order-keeping parser: serde_json `preserve_order` would unify
   across the workspace and reorder every JSON output penv prints.
6. URL parsing is hand-written: no new crate, and multi-host (`a,b`) must be detected.
7. `hash` is pure, so `sha2`/`hmac` move to `[workspace.dependencies]` and penv-schema uses them.
8. `random` options are parsed in penv-schema (pure); bytes stay in the binary (OS generator).
9. `[json.KEY]` names the subject key (`$GCP_SA_JSON`), paths matched exactly as written.
10. `raw()` is valid only as a `json()` option value; elsewhere a validation failure.
11. `b64d` is added as a filter (Functions.md:94 names it); Design.md:110 comment gains it.
12. Unknown names in a `public` list fail `check`, naming the section and the entry.
13. `--pin-hosts` writes with toml_edit so comments and order survive (Functions.md:43).
14. CHANGELOG gets an `Unreleased` section; no version bump (the release is the user's call).
15. Existing tests using `random(32)` are rewritten to `random(length=32)`; no compat path.
