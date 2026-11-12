# T6 policy-config (opus)

Wire the `public` lists into resolution and `check`. Spec: docs/Functions.md:113-131, :19;
Design.md:120 (penv-only note).

## Build
- crates/penv/src/config.rs: read `[url] public`, `[json.KEY] public`, `[hash] public` into
  `penv_schema::functions::Policy` (absent section = default). Follow the `[rotation]` accessor
  style at config.rs:314-330. Add commented examples to the template/defaults if config.rs
  renders defaults for every setting (check `with_defaults`).
- crates/penv/src/source.rs: pass the config's Policy to both `resolve_full` calls (T1 left
  `Policy::default()`).
- crates/penv/src/commands/check.rs:
  - fail when a `[url] public` entry is not a url part or `query.NAME`, or names `password`;
    when a `[hash] public` entry is not a key whose value is a `hash()` call (:130);
    when a `[json.KEY]` names no schema key (decision 12). Errors name section and entry.
  - note each `[hash] public` key (:130).
  - `penv_only` (check.rs:496): add `url(`, `json(`, `hash(` (Design.md:120).
- Never print a value in any message.

## Tests (crates/penv/tests/policy.rs, use tests/common helpers)
- Default policy: `DB_HOST=url($DATABASE_URL, host)` with a sensitive DATABASE_URL shows
  unmasked in `ls`; `DB_USER=url(..., user)` shows sensitive.
- `[url] public = [..., "user"]` declassifies user; `password` in the list fails check.
- `[hash] public = ["NOT_A_HASH"]` fails check naming the entry; a valid one is noted.
- `[json.GCP_SA_JSON] public = ["client_email"]` declassifies that read only.
- check notes url()/json()/hash() as penv-only.
