# v1 handoff

## Rules
- Read AGENTS.md (short) and your spec in docs/v1/specs/. The design docs are the contract.
- Never read a whole large file: search first (`rg -n`, `rg -l`, `git log --stat`), then read line
  ranges with `sed -n 'a,bp'`. Grep headings of docs/Design.md; read only needed sections.
  Cap command output with `| head -40`.
- Branch `v1/T<n>-<slug>` cut from `claude/v1-integration-ygsja5`; merge back when green.
- Before done, for touched crates only: `cargo check -p C`, `cargo test -p C`,
  `cargo clippy -p C -- -D warnings`, `cargo fmt --all -- --check`. Never `cargo build --release`.
- Commits carry no attribution lines. Never print a value; tests use obviously fake values.
- Report <=10 lines. Append one line per task to Log: `T<n> <sha> <pass|fail> <note>`.

## Conventions
(T1 fills: <=6 lines, e.g. module layout, error text shape, test file naming.)

## Tasks
| # | Slug | Goal | Spec | Paths | Deps | Model | Group |
|---|---|---|---|---|---|---|---|
| T1 | args-core | Argument form, dispatch, Policy, vocabulary args, stubs | Functions.md:7-19,133-149 | penv-schema/src/{resolve.rs,functions/mod.rs,functions/args.rs,vocabulary.json,editor.rs (Vocabulary only)}, penv/src/source.rs (call sites), penv-dotenv/tests/dotenv.rs | - | opus | A |
| T2 | url | `url()` read/write, parts, multi-host, taint | Functions.md:21-49 | penv-schema/src/functions/url.rs, penv-schema/tests/url.rs | T1 | sonnet | B |
| T3 | json | `json()` read/write/build, paths, raw(), dup warn | Functions.md:51-75 | penv-schema/src/functions/json.rs, penv-schema/tests/json.rs | T1 | sonnet | B |
| T4 | hash | `hash()` sha/hmac, encodings, length | Functions.md:77-95 | penv-schema/src/functions/hash.rs, penv-schema/tests/hash.rs, Cargo.toml, penv-schema/Cargo.toml, penv-cloud/Cargo.toml | T1 | sonnet | B |
| T5 | random | `random(kind=,length=)`, kinds, `random(N)` fails | Functions.md:97-111 | penv-schema/src/functions/random.rs, penv/src/source.rs (randoms), penv/tests/{run.rs,encrypt.rs} | T1 | sonnet | B |
| T8 | editor | Completions, hover, did-you-mean for args | Functions.md:146-149 | penv-schema/src/editor.rs (complete/diagnostics), penv-schema/tests/editor.rs | T1 | sonnet | B |
| T6 | policy-config | `public` lists from config, check validation and notes | Functions.md:113-131; Design.md:120 | penv/src/config.rs, penv/src/source.rs (policy), penv/src/commands/check.rs, penv/tests/policy.rs | T1,T5 | opus | C |
| T7 | computed-hosts | Computed `@hosts`, `[hosts]` pins, `--pin-hosts`, sealed refusal | Functions.md:44 | penv-schema/src/{parse.rs,ir.rs}, penv/src/{cli.rs,commands/check.rs,sealed/run.rs,config.rs}, penv/tests/hosts.rs | T2,T6 | opus | D |
| T9 | docs-e2e | CHANGELOG, Design filter list, end-to-end fixture tests, manifest check | ORCHESTRATOR.md decisions 11,14 | CHANGELOG.md, docs/Design.md:110, penv/tests/functions.rs | T2-T8 | haiku | E |

Groups run in order A, B, C, D, E. Tasks inside group B touch disjoint files and run in
parallel. T5 and T6 both touch source.rs (different functions), so T6 waits for T5.

## Log
