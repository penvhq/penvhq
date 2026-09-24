# Cloud API for the CLI

The surface `penv` speaks to penv.cloud. It is new in v1 and lives beside the existing machine-only `/api/v1/secrets` routes, which the retired TypeScript provider used and which nothing else needs. Every route is under `/api/v1`, answers JSON, and is authenticated with `Authorization: Bearer <credential>` unless stated. Error bodies are `{ "error": "<code>" }` with the status codes below.

## Principals and credentials

| Prefix | Who | How obtained | Lifetime |
|---|---|---|---|
| `pcu_` | a person | device-code login | 30 days from last use (every authenticated request extends `expiresAt`), revoked by `logout` |
| `pck_` | a machine identity | console-issued token, or exchanged from OIDC, AWS SigV4 or a bound keypair | as today; exchanges mint 15 minutes for the CLI |

Both resolve through one verifier to the same claims shape and one RBAC evaluator. A user credential carries the user's own role assignments and no fixed scope. A machine credential is bound to one project and environment; a request whose address is another environment is `403 forbidden`.

Claims: `{ principal: "user" | "machine", principalId, credentialId, orgId, projectId?, environmentId?, grants, plan }`.

## Device-code login

```
POST /api/v1/auth/device            unauthenticated, IP limited
  -> 201 { deviceCode, userCode, verificationUri, expiresIn: 600, interval: 5 }

POST /api/v1/auth/device/token      body { deviceCode }
  -> 428 { error: "authorization_pending" }
  -> 429 { error: "slow_down" }
  -> 410 { error: "expired" }
  -> 403 { error: "denied" }
  -> 201 { credential: "pcu_...", expiresAt, user: { email }, orgs: [{ slug, name }] }

console page /device                signed-in person enters userCode, sees the device name and IP, approves or denies; step-up MFA applies
POST /api/v1/auth/revoke            Bearer pcu_ or pck_, revokes itself; idempotent
```

`POST /auth/device` accepts `{ "device": "<host name>" }`, shown on the approval page. The user code is eight characters in two groups, `XXXX-XXXX`, case-insensitive, normalised server-side. Approval marks the row; the credential is minted by the first successful poll after approval, once. While polling, any 429 (the IP ceiling's `rate_limited` as well as `slow_down`) means back off, honouring `retry-after`. `user.email` may be null.

## Machine exchanges

Unauthenticated, IP limited. Each returns `201 { credential: "pck_...", expiresAt }` with a 15 minute lifetime capped by the trust's own expiry, or `401 { error: "expired" | "unauthorized" }`, or `503 { error: "unavailable" }` which means retry once.

```
POST /api/v1/auth/oidc               { token }                       audience is the org's slug or id
POST /api/v1/auth/aws                { method, url, body, headers }  a SigV4-signed STS GetCallerIdentity request; x-penv-cloud-org (slug or id) must be among the signed headers
POST /api/v1/auth/keypair/enroll     { secret: "pce_...", publicKey }  SPKI DER base64, Ed25519 -> 201 { credentialId, generation: 1 }
POST /api/v1/auth/keypair/challenge  { credentialId } -> 200 { nonce }   valid 120 s
POST /api/v1/auth/keypair            { credentialId, nonce, generation, signature } -> 201 { credential, expiresAt, generation }
```

The keypair signs the UTF-8 bytes of `penv-cloud:keypair:v1\n{credentialId}\n{nonce}\n{generation}` (four lines joined by newline); the signature is base64. The client persists the returned `generation` before using the credential. A generation mismatch that is not a replay answers `409 { error: "cloned" }` and revokes the keypair and everything it minted.

The CLI stores the credential in the OS keychain under the API base URL. `PENV_TOKEN` in the environment takes precedence over the keychain and is how CI and servers pass a `pck_`.

## Environments

An address is `{org}/{project}/{environment}`; org and project are slugs, environment is the free-form name the console knows.

```
HEAD /api/v1/envs/{org}/{project}/{environment}     If-None-Match honoured
  -> 304
  -> 200 with ETag: "<hash>"    hash over every (parameter id, latest version, meta) in the environment; changes when any value or decorator changes
  -> 404 not_found | 403 forbidden

GET  /api/v1/envs/{org}/{project}/{environment}     If-None-Match honoured
  -> 304
  -> 200 ETag + {
       "keys": [ { "path": "", "name": "DATABASE_URL", "kind": "static", "version": 3,
                   "updatedAt": "2026-09-22T14:05:00Z",     not yet served; the CLI reads it for @rotate (Design section 8)
                   "schema": { <one key of the .env.schema JSON IR> },
                   "value": "..." },
                 { "path": "", "name": "DB_PASSWORD", "kind": "static", "version": 4,
                   "redacted": true } ],    write-only environment, identity not a workload: present, no "value"
       "skipped": [ "path/name" ],     dynamic keys and keys with no version; absent when empty
       "writeOnly": true               the environment is write-only; absent otherwise
     }
  requires secret:reveal; `?values=false` lists with schema only and requires secret:read

PUT  /api/v1/envs/{org}/{project}/{environment}     push
  body { "keys": [ { "path", "name", "schema", "value"? } ], "prune": false }
  -> 200 { "written": n, "unchanged": n, "pruned": n, "etag": "..." }
  a key with no value updates the schema only; prune=true deletes keys not in the body; requires secret:write (and secret:delete when pruning)

PATCH  /api/v1/envs/{org}/{project}/{environment}/keys/{path...}/{name}   set
  body { "value"?: string, "schema"?: {...} }
  -> 200 { "version": n, "etag": "..." }

DELETE /api/v1/envs/{org}/{project}/{environment}/keys/{path...}/{name}   unset
  -> 200 { "etag": "..." }
```

Every address and key segment is percent-encoded by the client; environment names are free-form. The per-key schema is stored in `parameters.meta` as the same JSON object the CLI emits for that key in `penv schema --json`, minus `name`: `type {name, raw, members, constraints}`, `required`, `sensitive`, `default`, `description`, `example`, `docs`, `since`, `deprecated` (a string note), `rotate`, `dynamic` (boolean), `dynamicFrom`, `hosts` (below). The client omits absent fields; the server treats `null` as absent. Anything else is `400 schema_invalid`. Writes to a dynamic key answer `409 dynamic`. The console renders and edits it. `must_encrypt` follows `sensitive`.

### Write-only environments

An environment the console marks write-only (Pro plan and above) hands plaintext to workload identities only: a `pck_` exchanged from OIDC, AWS IAM or a bound keypair. A person's `pcu_` and a static `pck_` token get each key with `"redacted": true` and no `value`; `path`, `name`, `kind`, `version`, `updatedAt` and `schema` are still sent. The `ETag` changes when the flag flips.

| Route | Write-only, identity not a workload |
|---|---|
| `GET /envs/...` | `200`, `"writeOnly": true`, each key `"redacted": true` with no `value` |
| `GET /secrets/...` | `409 { "error": "redacted" }` |
| `POST /approvals` | `409 { "error": "redacted" }`: an approval cannot release a write-only value |

A redacted key has a value. The CLI never reports it as missing and never tells anyone to `penv set` it.

| Command | A redacted key no local layer supplies |
|---|---|
| `run`, `bundle` | refused: `redacted`, exit 6, naming every such key and the environment |
| `pull` | writes the comment `# penv:redacted KEY` in place of a value line; JSON `"redacted": [names]` |
| `check` | a note, not a failure; JSON `"redacted": [names]` |
| `ls` | value `redacted` |
| `why KEY` | state `redacted`, set in the cloud environment and withheld |
| `reveal KEY` | refused: `redacted`, exit 6 |

A value in `.env.<env>.local` (or any value file, or the process environment) wins over redaction on that machine.

### `hosts`

The hosts a value may be sent to (`@hosts` in `.env.schema`). When a key has them, an agent's process gets a placeholder and penv puts the real value into requests to these hosts only. The server stores the list and hands it back; it enforces nothing with it.

```json
{ "type": { "name": "string", "raw": "string(startsWith=sk_live_)", "constraints": { "startsWith": "sk_live_" } },
  "sensitive": true, "hosts": ["api.stripe.com", "*.stripe.com"] }
```

Accept it when every rule holds; otherwise `400 schema_invalid` with `"field": "hosts"`:

| Rule | Accept | Refuse |
|---|---|---|
| An array of 1 to 32 strings; an empty array is absent | `["api.stripe.com"]` | `"api.stripe.com"`, `[]` kept as a value, 33 entries |
| Each is a host name or IPv4 address: lowercase labels of `a-z`, `0-9`, `-`, 1 to 63 characters, not starting or ending with `-`, 253 characters in all | `db-1.internal`, `10.0.0.5`, `localhost` | `API.stripe.com`, `-a.com`, `a..com` |
| A wildcard only as the whole first label, followed by at least two labels, and never directly over a domain where anyone can get a name (the CLI's list: `SHARED_SUFFIXES` in `crates/penv-schema/src/placeholder.rs`) | `*.stripe.com`, `*.acme.vercel.app` | `*`, `*.com`, `api.*.com`, `*.co.uk`, `*.vercel.app` |
| No scheme, port, path, query or user | | `https://a.com`, `a.com:443`, `a.com/v1`, `u@a.com` |
| No duplicates | | `["a.com", "a.com"]` |

Storage and round trip:

1. Store the array as given, in the key's `parameters.meta`, in the order sent.
2. Return it unchanged wherever the per-key schema is returned: `GET /envs` (`keys[].schema.hosts`) and `?values=false`.
3. A `PUT` or `PATCH` whose schema has no `hosts` removes it, like any other schema field.
4. `hosts` changes no permission and no audit row. It is a schema change: bump the environment `ETag`.
5. Console: list the hosts on the key, editable with the same rules, and mark the key as "sent only to these hosts".

The CLI sends `hosts` on `push` and `set`. Until the server accepts it, a `400 schema_invalid` on a write that carried `hosts` is retried once without it, and the CLI warns that the cloud copy lacks it. So the server can ship this at any time, with no CLI release.

## Reveal approvals

An agent session may ask for a value; only a person may release one. This is phase 2b, and it exists.

```
POST /api/v1/approvals            bearer: the user credential (pcu_), agent session headers as on every request
  body { org, project, environment, key, device }
  201 { id, url, expiresAt }      requires secret:reveal for the caller; 10 minute expiry
  409 { error: "approval_pending", id, url } when an unexpired request for the same key and session exists
GET  /api/v1/approvals/{id}       200 { id, status: "pending"|"approved"|"denied"|"expired"|"redeemed", key, url, expiresAt }
POST /api/v1/approvals/{id}/redeem
  200 { key, value }              once; status becomes "redeemed"; audited as secret.reveal with the approver, the agent harness and session id
  409 { error: "approval_pending" | "approval_denied" | "approval_expired" | "approval_redeemed" }
```

`device` is the machine name the console page shows. The harness and the session are not body members: they reach the row from the `X-Penv-Agent` and `X-Penv-Session` headers the CLI stamps on every request, and the server reads them there. The 409 reuse answer carries the id and the url and no `expiresAt`, so the CLI passes none on.

The CLI side: `penv reveal KEY` under an agent creates the request and exits 4 with `{ "error": "approval_required", "message", "approval": id, "url", "expiresAt", "fix": "A person approves at <url>, then run penv reveal KEY --approval <id>." }`; a 409 answers in the same shape with no `expiresAt` and a message saying the key already has an open approval. `penv reveal KEY --approval <id>` reads `GET /approvals/{id}` first and refuses with its own `approval_mismatch` at exit 4, fix `Run penv reveal KEY for its own approval.`, when that approval names another key, so an id is never spent on a key nobody released. Otherwise it redeems: the value on 200, and on a 409 either exit 4 again (`approval_pending`, whose page and expiry come from that first read; `approval_expired` and `approval_redeemed`, whose fix is a fresh `penv reveal KEY`) or exit 2 (`approval_denied`). For a person at a terminal nothing changes: `reveal` is a plain read gated by `secret:reveal`, and `--approval` redeems an id they were handed the same way.

## Projects

```
GET  /api/v1/orgs                                   -> { orgs: [{ slug, name }] }
GET  /api/v1/orgs/{org}/projects                    -> { projects: [{ slug, name, environments: [name] }] }
POST /api/v1/orgs/{org}/projects                    body { name, environments: ["development"] } -> 201 ; requires project:create
```

Slugs are derived from names server-side; an ambiguous address is refused, never guessed. `penv push` on a schema with no `@penv=` header creates the project from the directory name after printing what it will do, and writes the `slug` the 201 body returns into the header. Project creation over the plan limit answers `409 quota_exceeded`, and a name another project in the workspace already answers to `409 ambiguous`. An OIDC or AWS exchange also answers `409 ambiguous` when two workspaces share the slug and both trust the identity. The CLI names the two by the call that got them: `project_taken` and `org_ambiguous`.

## Errors

| Status | Codes |
|---|---|
| 400 | `schema_invalid`, `name_required`, `keys_required`, `value_must_be_a_string`, `token_required` |
| 401 | `expired` (say so: run `penv login` again), `unauthorized` |
| 403 | `forbidden`, `denied` |
| 404 | `not_found` |
| 409 | `dynamic`, `cloned`, `quota_exceeded`, `ambiguous`, `approval_pending`, `approval_denied`, `approval_expired`, `approval_redeemed`, `redacted` (the CLI exits 6) |
| 429 | `rate_limited`, `slow_down`, both with `retry-after` seconds |
| 503 | `unavailable`, retry once |
| other 5xx | one retry after one second, then exit 1 naming the status |

## Rate limits and audit

As today: IP ceiling, then per-principal per-op plan limits. A user credential shares the identity bucket keyed by `principalId`. Every request may carry `X-Penv-Agent: <name>` and `X-Penv-Session: <id>` from the CLI's agent detection; every route that writes an audit row, including orgs, projects and the exchanges, stamps both into the row's metadata, truncated to 128 characters each and treated as data, so the console can answer "what did the agent session touch". An approval is three such rows: the request, the console's approve or deny, and the redemption, whose `secret.reveal` row names the approver beside the session that asked.
