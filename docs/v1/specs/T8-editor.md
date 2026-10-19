# T8 editor (sonnet)

Spec: docs/Functions.md:133-149 (Vocabulary). Uses the vocabulary fields T1 added.

## Build (crates/penv-schema/src/editor.rs only: `complete`, `hover`, `diagnostics`)
- Completions: option names after `hash(`, `random(`, `json(`, `url(` and after `, `; values after
  `algo=`, `encoding=`, `kind=` from the `values_from` section; url parts as the address after
  `url($X, ` (Functions.md:148). Do not offer an option already given.
- Hover: an option, value or part shows its entry's `doc`.
- Diagnostics: unknown, repeated or missing option; unknown value; `hots` gets
  "did you mean host?" (closest name by edit distance <= 2). Reuse `functions::args::check_args`
  so `check` and the editor say the same thing; add the suggestion there if absent, without
  touching other function files.
- Drop-in (Functions.md:149): nothing in editor.rs names an algorithm, encoding or part.

## Tests (crates/penv-schema/tests/editor.rs)
- Completion lists at each position above; already-given option not offered.
- Hover on `sha256` shows its doc; on `host` shows its doc.
- `url($X, hots)` diagnostic says "did you mean host?"; `hash($K)` says `algo` is missing.
- Existing editor tests still pass; every vocabulary entry with `values_from` names an existing
  section (a data test).
