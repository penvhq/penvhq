# T2 url (sonnet)

Fill `crates/penv-schema/src/functions/url.rs` (stub from T1). Spec: docs/Functions.md:21-49.

## Build
- Hand-written URL parser (decision 6): scheme, userinfo (user, password), host(s), port, path,
  query; percent-decoding of user/password/query values on read.
- Read: parts per the table at Functions.md:30-39; `origin` is `scheme://host[:port]`;
  `query.NAME` reads one query parameter. A part the URL lacks is empty (:41).
- Multi-host (`a:1,b:2`) fails `host` and `port` only; a non-URL fails every part (:42).
- Write: `scheme host port path user password query.NAME` (:43); one call reads or writes,
  never both (Functions.md:14). Re-encode written user/password/query values.
- `password=` must be a `$KEY` reference, never a literal: the arg check fails otherwise.
- Taint (`functions::taint` for url): read of a part in `policy.url_public` is `Public`, except
  `password`, which is `Always`; any write is `Inputs` (:43).
- Errors name the key and the part, never the value (Functions.md:18).

## Tests (crates/penv-schema/tests/url.rs, fake values only)
- Every row of Functions.md:32-39 against the example URL at :25.
- Missing port is empty; `fallback(url($X, port), 5432)` yields 5432.
- Multi-host fails host/port but reads scheme; a non-URL fails each part.
- Write host+port round-trips; written password from `$KEY`; literal `password=x` fails.
- Taint: host of a sensitive URL is not tainted; user and password are; `password` is tainted
  even from a non-sensitive URL; adding `user` to `url_public` declassifies it.
- Read+write in one call fails.
