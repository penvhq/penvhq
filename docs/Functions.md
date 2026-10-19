# Functions

Version 1. The contract for the functions a `.env.schema` value can call: `url`, `json`, `hash` and `random`, and the argument form every function follows. [Design.md](./Design.md) holds the rest of the grammar.

A change to this page adds a row to [Changes](#changes) and edits the section it changes. It does not add a section beside it.

## Argument form

```dotenv
name($SUBJECT, address, option=value, ...)
```

- **Subject first.** A function with no subject (`random`, and `json` when it builds) starts at its options.
- **One bare address, only in `url` and `json`**, naming a part or a path. `url($X, host)` reads it, `url($X, host=replica.internal.test)` writes it. A call reads or writes, never both.
- **Every other argument is `name=value`.** Order never matters. A repeated or unknown option is a validation failure naming the function and the option.
- **Lists and cases are unchanged:** `forEnv(a, b)` and `match(subject, label: value)`.
- **Filter spelling.** `${KEY | name(args)}` is the function with its subject implied, and filters chain left to right. A default cannot hold a `|`.
- **Failures** name the key and the part, path or option, never the value.
- **Taint.** A result is tainted when any input is sensitive: the subject, a written value or a `key=`. A key read only to decide (`eq`, `startsWith`, ...) taints nothing. Only the `public` lists under [Config](#config) declassify, and each default is the safe side.

## url

```dotenv
DATABASE_URL=postgres://app:fakepass@db.internal.test:5432/orders?sslmode=require
DB_HOST=url($DATABASE_URL, host)                              # db.internal.test
REPLICA_URL=url($DATABASE_URL, host=replica.internal.test, port=5433)
DB_HOST=${DB_SECRET | json(database_url) | url(host)}
```

| Part | Result for the URL above | Default |
|---|---|---|
| `scheme` | `postgres` | public |
| `host` | `db.internal.test` | public |
| `port` | `5432` | public |
| `origin` | `postgres://db.internal.test:5432` | public |
| `path` | `/orders` | public |
| `user` | `app` | sensitive |
| `password` | `fakepass` | sensitive, always |
| `query.NAME` | `query.sslmode` is `require` | sensitive |

- A part the URL does not carry is empty. `fallback(url($X, port), 5432)` supplies a default.
- A URL that names several hosts fails `host` and `port`. A value that is not a URL fails every part.
- Writable parts: `scheme`, `host`, `port`, `path`, `user`, `password` and `query.NAME`. A `password=` value is a reference to another key, never a literal. A written URL is sensitive when its source is.
- **Computed `@hosts`.** `@hosts=url($DATABASE_URL, host)` is allowed. Its hosts are pinned in `[hosts]`: `check` fails when a computed `@hosts` key has no pin or its hosts differ from the pin, naming the key and both lists (hosts are public), and a sealed run refuses the same way. `penv check --pin-hosts` writes the pins, keeps comments and order, and prints only the hosts that changed. No other command writes `[hosts]`.
- **Cross-env assert**, with no new feature:

```dotenv
# @assert(if(forEnv(production), true, not(eq(url($DATABASE_URL, host), url(penv(production/DATABASE_URL), host)))), "DATABASE_URL points at the production host")
```

## json

| Call | Does |
|---|---|
| `json($X)` | the whole document, compact; invalid JSON fails |
| `json($X, db.hosts[0])` | one field |
| `json($X, project_id=acme-sandbox)` | writes a field, creating it when missing |
| `json(username=$U, password=$P)` | builds an object; no subject starts from `{}` |
| `json(db.host=h, db.port=5432)` | a dotted label creates nested objects |
| `json(hosts[0]=a, hosts[1]=b)` | an indexed label creates an array |
| `json(db=raw($INNER))` | embeds JSON that already exists |

```dotenv
DB_SECRET='{"username":"app","db":{"hosts":["db1.internal.test","db2.internal.test"],"port":5432},"tls":true}'
DB_HOST=json($DB_SECRET, db.hosts[0])          # db1.internal.test
DB_HOSTS=json($DB_SECRET, db.hosts)            # ["db1.internal.test","db2.internal.test"]
DB_TLS=json($DB_SECRET, tls)                   # true
FEATURES=json(beta=eq($APP_ENV, staging), maxUploadMb=25, region=eu-1)
# {"beta":true,"maxUploadMb":25,"region":"eu-1"}
```

- **Paths** are `a.b[0]`; `["a.b"]` names a key that contains a dot. A field called `get` is `json(get=1)`; no word is reserved.
- **Reading.** A string comes back as it is, a number or boolean as text, an array or object as compact JSON. A missing or null field fails; `fallback()` supplies a default. A duplicate key in the document keeps the last value and warns, naming the key.
- **Writing.** A bare number, `true` or `false` is typed; anything else is a string, and quotes force a string (`maxUploadMb='25'`). A string is escaped, so a value with quotes or newlines stays valid JSON. Key order is kept.
- **Taint.** A result is sensitive when any input is; one built only from public inputs is public.

## hash

```dotenv
API_KEY_FP=hash($API_KEY, algo=sha256, length=8)                       # 604d6582 for the fake value fake_api_key_123
TEST_SIG=hash('{"id":1}', algo=hmac-sha256, key=$WEBHOOK_SECRET)
SESSION_KEY=hash(session, algo=hmac-sha256, key=$ROOT)                 # a sub-key of $ROOT
```

| Option | Values | Default |
|---|---|---|
| `algo` | `sha256`, `sha512`, `hmac-sha256`, `hmac-sha512` | required |
| `encoding` | `hex`, `base64`, `base64url` | `hex` |
| `length` | at least 1; keeps the first N characters of the encoded result | all |
| `key` | the secret an `hmac-*` algorithm signs with | none |

- `hash` is pure: the same inputs give the same result, and it reads and writes nothing.
- An `hmac-*` algorithm needs `key=`, and `key=` is refused for the others. For HMAC the subject is the data.
- `md5` and `sha1` are not offered. Reversible encodings stay filters: `base64`, `b64d`, `urlencode`.
- A result is sensitive when the subject or the key is.

## random

```dotenv
SESSION_SECRET=random(length=32)               # letters and digits
COOKIE_KEY=random(kind=hex, length=32)
INSTANCE_UUID=random(kind=uuid)                # a version-4 UUID
```

| Option | Values | Default |
|---|---|---|
| `kind` | `alnum`, `hex`, `base64url`, `uuid` | `alnum` |
| `length` | 8 to 512; required, except that `uuid` takes none | none |

- Characters come from the operating system's generator. `run` generates the value once and keeps it in `.env.local` (`.env.test.local` for test); later runs read it back. `check`, `ls` and `scan` never write it, and `check` notes it as pending. `push` never sends it. Each machine has its own.
- `random(32)` is a validation failure that says to write `length=32`.

## Config

```toml
[url]
public = ["scheme", "host", "port", "origin", "path"]   # add "user" or "query.sslmode" to opt in

[json.GCP_SA_JSON]
public = ["client_email", "project_id"]                 # read paths that are public for this key

[hash]
public = ["API_KEY_FP"]                                  # keys whose hash result may be shown

[hosts]
DATABASE_URL = ["db.internal.test"]                      # pin for a computed @hosts
```

- Every list is named `public`. A dotted name puts `query.sslmode` in the same list as the URL parts.
- A `[hash] public` entry must be a key whose value is a `hash()` call, or `check` fails. `check` notes each listed key.
- `[hosts]` pairs with `@hosts` the way `[rotation]` pairs with `@rotate`.

## Vocabulary

`vocabulary.json` describes arguments, and the language server and `check` read the same entries.

```json
{ "name": "hash", "origin": "penv", "form": "hash(subject, algo=sha256, encoding=hex, length=N, key=$K)",
  "args": [ { "name": "algo", "required": true, "values_from": "algorithms" },
            { "name": "encoding", "values_from": "encodings", "default": "hex" },
            { "name": "length", "type": "integer" },
            { "name": "key", "type": "value" } ] }
{ "name": "url", "address": { "values_from": "url-parts" } }
{ "name": "hmac-sha256", "section": "algorithms", "needs": ["key"] }
{ "name": "host", "section": "url-parts", "public": true }
```

- **Editor:** option names after `(`, values after `=`, parts after `url(`, hover from each entry's `doc`, and diagnostics for an unknown, repeated or missing option (`hots` gets "did you mean host?").
- **Drop-in:** a new algorithm, encoding or part is a new entry. `hash` and `url` are not edited.

## Not read

varlock's `domainFromUrl()`, `randomUuid()`, `randomHex()`, `randomString()` and `randomNum()` are validation failures naming the function (Design §2). `url($X, host)` and `random(kind=...)` cover the same ground under penv's names.

## Changes

| Version | Change |
|---|---|
| 1 | First specification: the argument form, `url`, `json`, `hash`, `random` with named options, the `public` and `[hosts]` config keys, and argument entries in the vocabulary. `random(N)` is removed. |
