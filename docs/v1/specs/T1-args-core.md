# T1 args-core (opus)

Build the seam every other task fills. Contract: docs/v1/CONTRACT.md (implement it exactly).

## Build
- `crates/penv-schema/src/functions/{mod,args,url,json,hash,random}.rs`; `lib.rs` exports `functions`.
- Parser (resolve.rs `parse_at`/`arg`): calls to `url`, `json`, `hash`, `random` parse args into
  `Arg` per docs/Functions.md:11-16. A `name=value` arg is `Named` (split on the first `=` outside
  quotes/brackets; label may be dotted/indexed, `["a.b"]`, Functions.md:72). A bare first arg that
  is a `$KEY`, quoted text or call is `Subject`; a bare word in url/json after the subject is
  `Address`. Other functions keep positional args (`Arg::Subject`) and `match` cases unchanged.
- Filters with arguments: `${KEY | json(path) | url(host)}` (Functions.md:17), subject implied.
  `b64d` joins FILTERS (decision 11); an invalid base64 input fails naming the key.
- `check_args` reads `args`/`address` from vocabulary.json: repeated or unknown option fails
  naming function and option (Functions.md:15); a failure never names the value (:18).
- `raw()` is accepted only as a `json()` option value (decision 10).
- `resolve_full`/`resolve` take `&Policy`; update callers: crates/penv/src/source.rs:610,630
  (pass `Policy::default()` for now; T6 wires config), crates/penv-dotenv/tests/dotenv.rs:153,
  and resolve.rs unit tests.
- Taint: before evaluating url/json/hash, call `functions::taint`; `Public` evaluates args as
  control (no deps), `Always` adds dep `ALWAYS_SENSITIVE`; `tainted()` treats it as sensitive.
- Stubs in url/json/hash return `Err("<name>() is not built yet")`; T2-T4 replace the bodies.
- vocabulary.json: replace `random(N)` entry; add url/json/hash/raw entries with `args`/`address`
  and the sections `algorithms`, `encodings`, `url-parts`, `random-kinds` (Functions.md:137-146).
  `editor::Vocabulary` loads them (fields only; completions are T8).
- Fill the Conventions section of docs/v1/HANDOFF.md (<=6 lines).

## Acceptance
- `url($X, host)`, `url($X, host=h, port=5433)`, `json(a.b=1, c=raw($I))`, `hash($K, algo=sha256)`,
  `${K | json(a) | url(host)}` parse to the expected `Arg`s (unit tests on the parser).
- `hash($K, algo=sha256, algo=sha512)` and `hash($K, algoo=sha256)` fail naming option and key.
- `tainted()` marks a key whose deps hold `ALWAYS_SENSITIVE`.
- Existing penv-schema, penv-dotenv and penv tests pass unchanged except call sites.
