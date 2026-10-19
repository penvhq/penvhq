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
- penv crate tests refuse to run inside an agent session (env + parent-process detection). To run them locally: `env -u CLAUDECODE -u CLAUDE_CODE_CHILD_SESSION -u AI_AGENT setsid -f sh -c 'cargo test -p penv > /tmp/penv-test.out 2>&1; echo done >> /tmp/penv-test.out'` then wait for `done`. Never weaken the guard in code.
- json() quoted option values: `Call.quoted` holds option names written in quotes (T1 fix); T3 must honour it (Functions.md:74).

## Conventions
- `functions/<name>.rs` exposes `pub fn eval(call: &Call) -> Result<String, String>`; `functions::eval` dispatches.
- Errors: `"<fn>() <problem>; <fix>"`, naming option, part or path, never a value; resolve prefixes `KEY: `.
- Argument shape lives in vocabulary.json (`subject`, `address`, `labels`, `args`); `check_args` runs at parse.
- `editor::Vocabulary::section(name)` returns entries; `Entry::names` matches `query.NAME` patterns.
- Parser tests go in resolve.rs `mod tests`; per-function tests in `penv-schema/tests/<name>.rs`.
- `random(N)` already fails at parse (check_args); penv's `randoms()` still reads `random(N)` text (T5).

## Tasks
| # | Slug | Goal | Spec | Paths | Deps | Model | Group |
|---|---|---|---|---|---|---|---|
| T1 ✓ | args-core | Argument form, dispatch, Policy, vocabulary args, stubs | Functions.md:7-19,133-149 | penv-schema/src/{resolve.rs,functions/mod.rs,functions/args.rs,vocabulary.json,editor.rs (Vocabulary only)}, penv/src/source.rs (call sites), penv-dotenv/tests/dotenv.rs | - | opus | A |
| T2 ✓ | url | `url()` read/write, parts, multi-host, taint | Functions.md:21-49 | penv-schema/src/functions/url.rs, penv-schema/tests/url.rs | T1 | sonnet | B |
| T3 ✓ | json | `json()` read/write/build, paths, raw(), dup warn | Functions.md:51-75 | penv-schema/src/functions/json.rs, penv-schema/tests/json.rs | T1 | sonnet | B |
| T4 ✓ | hash | `hash()` sha/hmac, encodings, length | Functions.md:77-95 | penv-schema/src/functions/hash.rs, penv-schema/tests/hash.rs, Cargo.toml, penv-schema/Cargo.toml, penv-cloud/Cargo.toml | T1 | sonnet | B |
| T5 ✓ | random | `random(kind=,length=)`, kinds, `random(N)` fails | Functions.md:97-111 | penv-schema/src/functions/random.rs, penv/src/source.rs (randoms), penv/tests/{run.rs,encrypt.rs} | T1 | sonnet | B |
| T8 ✓ | editor | Completions, hover, did-you-mean for args | Functions.md:146-149 | penv-schema/src/editor.rs (complete/diagnostics), penv-schema/tests/editor.rs | T1 | sonnet | B |
| T6 ✓ | policy-config | `public` lists from config, check validation and notes | Functions.md:113-131; Design.md:120 | penv/src/config.rs, penv/src/source.rs (policy), penv/src/commands/check.rs, penv/tests/policy.rs | T1,T5 | opus | C |
| T7 ✓ | computed-hosts | Computed `@hosts`, `[hosts]` pins, `--pin-hosts`, sealed refusal | Functions.md:44 | penv-schema/src/{parse.rs,ir.rs}, penv/src/{cli.rs,commands/check.rs,sealed/run.rs,config.rs}, penv/tests/hosts.rs | T2,T6 | opus | D |
| T9 | docs-e2e | CHANGELOG, Design filter list, end-to-end fixture tests, manifest check | ORCHESTRATOR.md decisions 11,14 | CHANGELOG.md, docs/Design.md:110, penv/tests/functions.rs | T2-T8 | haiku | E |
| T10 ✓ | wire-up | fallback errors, json dup warning, url password at parse, penv deps | specs/T10-wire-up.md | see spec | T2-T5 | sonnet | C |

Groups run in order A, B, C, D, E. Tasks inside group B touch disjoint files and run in
parallel. T5 and T6 both touch source.rs (different functions), so T6 waits for T5.

## Log
T1 c63897c pass also touched penv-targets/src/view.rs (resolve_full call site); 19 penv tests fail before and after (agent-session env)
T2 40664f2 pass after 1 fix round (write-side host/path/port/scheme validation); check_password not yet wired into check_args (T10)
T3 4b41575 pass after 1 fix round; fallback() error catch and duplicate-key warning need resolve.rs (T10)
T4 b1d97e7 pass; sha2/hmac to workspace deps (penv crate still pins its own: T10)
T5 b92cb0b pass after 1 fix round; check_args calls random::spec; check.rs keeps validation errors out of cloud fallback
T8 8f341f3 pass after 1 fix round; added resolve::check_expression, did-you-mean in args.rs
T10 b69ee48 pass after 1 fix round (fallback only defaults missing/null json fields; other errors stay fatal)
T6 4a9af34 pass after 1 fix round (gen uses run-time policy; is_whole_call replaces text scanner; entries echoed only if key/part name)
T7 d0b29ac pass after 1 fix round; schema JSON has no hostsExpr (cloud refuses unknown fields) - cloud follow-up
