# T3 json (sonnet)

Fill `crates/penv-schema/src/functions/json.rs` (stub from T1). Spec: docs/Functions.md:51-75.

## Build
- Order-keeping JSON parser and compact writer (decision 5; do not enable serde_json
  `preserve_order`). Detect duplicate keys: keep the last, return a warning naming the key
  (surface as a soft `ResolveError`).
- Paths `a.b[0]`, `["a.b"]` (:72). Read rules (:73): string as is, number/bool as text,
  array/object compact; missing or null fails naming the path.
- Write with subject: set a path, creating it when missing (:57). Build without subject from
  `{}` (:58-60): dotted labels nest, indexed labels make arrays. `raw($X)` embeds parsed JSON (:61);
  invalid raw JSON fails.
- Typing on write (:74): bare number, `true`, `false` typed; quoted forces string; strings escaped.
- `json($X)` alone: whole document compact; invalid JSON fails (:55).
- Taint (`functions::taint` for json): a read whose path is in `policy.json_public[subject_key]`
  is `Public` (decision 9); everything else `Inputs` (:75).

## Tests (crates/penv-schema/tests/json.rs)
- Each example at Functions.md:64-70 gives the commented result.
- `json(get=1)` is `{"get":1}`; `maxUploadMb='25'` is a string; a value with quotes/newline stays
  valid JSON; key order kept; duplicate key warns with its name only.
- Missing/null field fails; `fallback(json($X, nope), d)` yields `d`.
- Taint: listed path from a sensitive subject is public; unlisted path is tainted.
