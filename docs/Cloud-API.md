# Cloud API for the CLI

The HTTP contract `penv` speaks to provider `penv`: our hosted [penv.cloud](https://penv.cloud), or any server you point `PENV_URL` or `[providers.penv] url` at ([providers](./PROVIDERS.md)). The client is `crates/penv-cloud/src/api.rs`, one method per route.

| Rule | Value |
|---|---|
| Base path | `{root}/api/v1` |
| Transport | HTTPS; plain HTTP only to `127.0.0.1`, `localhost`, `[::1]` |
| Redirects | never followed; any 3xx but 304 fails the request |
| Timeout | 30 s per request |
| Auth | `Authorization: Bearer <credential>`, except the unauthenticated routes marked below |
| Every request | `User-Agent: penv/<version>`; `X-Penv-Agent` and `X-Penv-Session` when an agent is detected |
| Path segments | percent-encoded to RFC 3986 unreserved; a segment made only of dots is refused before sending |
| Error body | `{ "error": "<code>" }`; `retry-after` header (or `retryAfter` in the body) on 429 |
| Retry | a 5xx is sent once more after 1 s |

## Credentials

| Prefix | Who | Obtained by | Lifetime on penv.cloud |
|---|---|---|---|
| `pcu_` | a person | [`penv login`](https://penv.cloud/docs/cli/login) (device code) | 30 days, extended on each use; revoked by [`penv logout`](https://penv.cloud/docs/cli/logout) |
| `pck_` | a machine | a console token (`PENV_TOKEN`), or an OIDC, AWS or keypair exchange | exchanges: 15 minutes |

## Device-code login

| Route | Body | Answers |
|---|---|---|
| `POST /auth/device` (unauthenticated) | `{ device }`: the host name the approval page shows | `201 { deviceCode, userCode, verificationUri, expiresIn, interval }` |
| `POST /auth/device/token` (unauthenticated) | `{ deviceCode }` | `201 { credential, expiresAt, user: { email }, orgs: [{ slug, name }] }`; `428 authorization_pending`; `429` (any code); `410 expired`; `403 denied` |
| `POST /auth/revoke` | none; revokes the bearer itself | `200` or `204` |

The CLI polls every `interval` seconds; on any 429 it switches to the `retry-after` the answer names. `user.email` may be null. On penv.cloud, `expiresIn` is 600, `interval` is 5 and `userCode` is `XXXX-XXXX`, case-insensitive.

## Machine exchanges

All unauthenticated. Each answers `201 { credential: "pck_...", expiresAt }` (200 accepted).

| Route | Body |
|---|---|
| `POST /auth/oidc` | `{ token }`; the CLI asks the CI platform for a token whose audience is the org slug |
| `POST /auth/aws` | `{ method, url, body, headers }`: a SigV4-signed STS `GetCallerIdentity`, with `x-penv-cloud-org` among the signed headers |
| `POST /auth/keypair/enroll` | `{ secret: "pce_...", publicKey }` (Ed25519 SPKI DER, base64) answers `{ credentialId, generation }` |
| `POST /auth/keypair/challenge` | `{ credentialId }` answers `{ nonce }` |
| `POST /auth/keypair` | `{ credentialId, nonce, generation, signature }` answers `{ credential, expiresAt, generation }` |

The keypair signs the UTF-8 bytes of `penv-cloud:keypair:v1\n{credentialId}\n{nonce}\n{generation}` (base64 signature) and persists the returned `generation` before using the credential. `409 cloned` means a second machine used the keypair; we revoke it.

## Environments

An address is `{org}/{project}/{environment}`: org and project slugs, and the environment's free-form name.

| Route | Command | Request | Answers |
|---|---|---|---|
| `HEAD /envs/{org}/{project}/{environment}` | cached reads | `If-None-Match` | `304`, or `200` with `ETag` |
| `GET /envs/{org}/{project}/{environment}` | [`penv run`](https://penv.cloud/docs/cli/run), [`penv reveal`](https://penv.cloud/docs/cli/reveal) and every read | `If-None-Match` | `304`, or `200` with `ETag` and the body below |
| `PUT /envs/{org}/{project}/{environment}` | [`penv push`](https://penv.cloud/docs/cli/push) | `{ keys: [{ path, name, schema?, value? }], prune }` | `200 { written, unchanged, pruned, etag }` |
| `PATCH /envs/.../keys/{path...}/{name}` | [`penv set`](https://penv.cloud/docs/cli/set) | `{ value?, schema? }` | `200 { version, etag }` |
| `DELETE /envs/.../keys/{path...}/{name}` | [`penv unset`](https://penv.cloud/docs/cli/unset) | none | `200 { etag }` |

```json
{
  "keys": [
    { "path": "", "name": "DATABASE_URL", "kind": "static", "version": 3,
      "updatedAt": "2026-09-22T14:05:00Z", "schema": { "sensitive": true }, "value": "..." },
    { "path": "", "name": "DB_PASSWORD", "kind": "static", "version": 4, "redacted": true }
  ],
  "skipped": ["path/name"],
  "writeOnly": true
}
```

| Field | Meaning |
|---|---|
| `updatedAt` | last value write; `@rotate` counts from it |
| `skipped` | keys with no value to give (dynamic, or never written); absent when empty |
| `redacted` | present and withheld from this identity |
| `writeOnly` | the environment is write-only; absent otherwise |

On `PUT`, a key with no `value` updates its schema only, and `prune: true` deletes the keys the body leaves out, except dynamic ones. The `ETag` changes when any value, schema or the write-only flag changes.

### Per-key schema

`schema` is the key's object from [`penv schema --json`](https://penv.cloud/docs/cli/schema) minus `name`.

| Refusal | Meaning | CLI exit |
|---|---|---|
| `400 schema_invalid` | a schema field is refused; `field` names it | 1 |
| `400 name_invalid` | a key name outside `A-Z`, `0-9`, `_` | 3 |
| `409 dynamic` | the key's value is generated; the CLI cannot write it | 3 |

### Write-only environments

In a write-only environment, only workload identities get plaintext: a `pck_` exchanged from OIDC, AWS or a bound keypair. A `pcu_` and a console `pck_` get each key with `"redacted": true` and no `value`; the other fields still arrive.

| Route | Identity not a workload |
|---|---|
| `GET /envs/...` | `200`, `"writeOnly": true`, each key `"redacted": true` |
| `POST /approvals` | `409 redacted`: an approval cannot release a write-only value |

A redacted key has a value: the CLI never reports it missing and exits 6 where it needs one. A value in `.env.<env>.local` wins over redaction on that machine.

### hosts

`hosts` is the list `@hosts` declares: the hosts a value may be sent to. We store it and hand it back unchanged; it grants and enforces nothing on our side.

```json
{ "type": { "name": "string" }, "sensitive": true, "hosts": ["api.stripe.com", "*.stripe.com"] }
```

| Rule | Accept | Refuse (`400 schema_invalid`, `"field": "hosts"`) |
|---|---|---|
| An array of 1 to 32 strings; `[]` is absent | `["api.stripe.com"]` | `"api.stripe.com"`, 33 entries |
| Host names or IPv4: lowercase labels of `a-z`, `0-9`, `-`, 1 to 63 characters, no leading or trailing `-`, 253 in all | `db-1.internal`, `10.0.0.5`, `localhost` | `API.stripe.com`, `-a.com`, `a..com` |
| A wildcard only as the whole first label, over at least two labels, never over a shared suffix (`SHARED_SUFFIXES` in `crates/penv-schema/src/placeholder.rs`) | `*.stripe.com`, `*.acme.vercel.app` | `*`, `*.com`, `api.*.com`, `*.co.uk`, `*.vercel.app` |
| No scheme, port, path, query or user | | `https://a.com`, `a.com:443`, `a.com/v1`, `u@a.com` |
| No duplicates | | `["a.com", "a.com"]` |

A `PUT` or `PATCH` whose schema has no `hosts` removes it. When a server answers `400 schema_invalid` to a write that carried `hosts`, the CLI retries once without it and warns that the cloud copy lacks it.

## Reveal approvals

An agent may ask for a value; only a person releases one. The CLI side is on the [`penv reveal`](https://penv.cloud/docs/cli/reveal) page.

| Route | Body | Answers |
|---|---|---|
| `POST /approvals` | `{ org, project, environment, key, device }` | `201 { id, url, expiresAt }`; `409 { error: "approval_pending", id, url }` when one is open for the same key and session |
| `GET /approvals/{id}` | none | `200 { id, status, key, url, expiresAt }` |
| `POST /approvals/{id}/redeem` | none | `200 { key, value }`, once; `409` `approval_pending`, `approval_denied`, `approval_expired` or `approval_redeemed` |

The harness and session reach the approval from the `X-Penv-Agent` and `X-Penv-Session` headers, not the body. `status` is `pending`, `approved`, `denied`, `expired` or `redeemed`. On penv.cloud, only a `pcu_` with `secret:reveal` can ask, and the request expires after 10 minutes.

## Projects and environments

The routes behind [`penv project`](https://penv.cloud/docs/cli/project) and [`penv env`](https://penv.cloud/docs/cli/env).

| Route | Command | Body | Answers |
|---|---|---|---|
| `GET /orgs` | `penv push`, linking a schema | none | `{ orgs: [{ slug, name }] }` |
| `GET /orgs/{org}/projects` | [`penv project ls`](https://penv.cloud/docs/cli/project-ls) | none | `{ projects: [{ slug, name, environments: [name] }] }` |
| `POST /orgs/{org}/projects` | [`penv project new`](https://penv.cloud/docs/cli/project-new), `penv push` | `{ name, environments }` | `201 { slug, name, environments }` |
| `PATCH /orgs/{org}/projects/{project}` | [`penv project rename`](https://penv.cloud/docs/cli/project-rename) | `{ name }` | `200 { slug, name }` |
| `DELETE /orgs/{org}/projects/{project}` | [`penv project rm`](https://penv.cloud/docs/cli/project-rm) | none | `200 { name, environments, parameters }` |
| `POST /orgs/{org}/projects/{project}/environments` | [`penv env new`](https://penv.cloud/docs/cli/env-new), [`penv env copy`](https://penv.cloud/docs/cli/env-copy) | `{ name, from? }` | `201 { name, copied }` |
| `PATCH .../environments/{environment}` | [`penv env rename`](https://penv.cloud/docs/cli/env-rename) | `{ name }` | `200` |
| `DELETE .../environments/{environment}` | [`penv env rm`](https://penv.cloud/docs/cli/env-rm) | none | `200 { name, parameters }` |

The new slug comes back in the answer; `penv push` writes it into the `@penv=` header. `from` copies another environment's keys and their decorators, never its values.

## Errors

The CLI turns each refusal into an [exit code](https://penv.cloud/docs/reference/errors):

| Status and code | CLI exit |
|---|---|
| `401 expired`, `401` any other | 2; run `penv login` again |
| `403` on an environment | 6 (`environment_refused`) |
| `403` elsewhere | 2 (`forbidden`) |
| `404` | 1 (`not_found`) |
| `409 ambiguous` from project create | 3 (`project_taken`) |
| `409 ambiguous` from an OIDC or AWS exchange | 2 (`org_ambiguous`) |
| `409 cloned` | 2 |
| `409 redacted` | 6 |
| `409 quota_exceeded`, `live_leases`, `exists` | 1, 1, 3 |
| `429` | 1, naming `retry-after` |
| `5xx` twice | 1, naming the status |
| a 3xx | 1 (`cloud_failed`) |
| no answer | 5 (`offline`) |
