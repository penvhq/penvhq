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
| Error body | `{ "error": "<code>" }`, with a `message` or `detail` where a route below says so; `retry-after` header (or `retryAfter` in the body) on 429 |
| Retry | a 5xx is sent once more after 1 s; a machine exchange's `503` up to three more times (below) |
| Server text | a `message` or `detail` the CLI shows loses control, bidi and zero-width characters and stops at 500 characters |

## Credentials

| Prefix | Who | Obtained by | Lifetime on penv.cloud |
|---|---|---|---|
| `pcu_` | a person | [`penv login`](https://penv.cloud/docs/cli/login) (device code) | 30 days from last use, never past 90 days from issue; revoked by [`penv logout`](https://penv.cloud/docs/cli/logout), a password reset, "Sign out everywhere" and account recovery |
| `pck_` | a machine | a console token (`PENV_TOKEN`), or an OIDC, AWS or keypair exchange | exchanges: 15 minutes, 5 under an agent (`ttlSeconds`) |

## Device-code login

| Route | Body | Answers |
|---|---|---|
| `POST /auth/device` (unauthenticated) | `{ device }`: the host name the approval page shows | `201 { deviceCode, userCode, verificationUri, expiresIn, interval }` |
| `POST /auth/device/token` (unauthenticated) | `{ deviceCode }` | `201 { credential, expiresAt, user: { email }, orgs: [{ slug, name }] }`; `428 authorization_pending`; `429` (any code); `410 expired`; `403 denied` |
| `POST /auth/revoke` | none; revokes the bearer itself | `200` or `204` |

The CLI polls every `interval` seconds; on any 429 it switches to the `retry-after` the answer names. `user.email` may be null. On penv.cloud, `expiresIn` is 600, `interval` is 5 and `userCode` is `XXXX-XXXX`, case-insensitive.

| Rule | The CLI |
|---|---|
| The approval page does not fill the code in from the URL | opens `verificationUri` and prints `Enter this code in the browser: ABCD-EFGH. Only approve it if you started this login.` |
| `device` is kept as 48 plain characters (letters, digits, space, `._-()'@`) | sends the host name already reduced to them |
| The approver picks one workspace (the default) or all | lists `orgs`, the workspaces the login reaches, after sign-in; a `404` says the login may have been approved for a different workspace and to run `penv login` again and choose it |
| `403 { "error": "forbidden", "message" }`: a workspace's sign-in policy (single sign-on, two-factor) | prints `message` as written; a `forbidden` without one keeps its own copy |
| `401 expired` or `401 unauthorized` from a stored login | says the login ended and to run `penv login` again; no retry, no loop |

## Machine exchanges

All unauthenticated. Each answers `201 { credential: "pck_...", expiresAt }` (200 accepted), where `expiresAt` is the real expiry.

The workspace a machine login names is its id, a UUID: the Organization ID under Settings → Organization in the console. A slug is refused before any lookup with `400 { "error": "audience_not_workspace_id", "message" }`, and the CLI prints `message` as written. The CLI takes the workspace from `@penv=<org>/<project>`: where a login asks for an audience (GitHub Actions) or signs the header (AWS), it validates the org as a UUID and refuses a slug before any request, saying where the id is. There is no slug-to-id lookup. A held token (`ID_TOKEN`, `PENV_OIDC_TOKEN`) carries whatever audience it was minted with. Routes a person's `penv login` reaches by slug are unchanged.

Every route that mints a `pck_` (`/auth/oidc`, `/auth/aws`, `/auth/keypair`) also takes an optional integer `ttlSeconds` in its body: the CLI sends 300 under a detected or declared agent and 900 otherwise. The server mints for the least of `ttlSeconds`, 900 and the trust's own cap; absent means 15 minutes; a non-integer or anything below 60 is `400`. The CLI never reuses a minted credential past its session's `ttlSeconds`, whatever `expiresAt` says.

| Route | Body |
|---|---|
| `POST /auth/oidc` | `{ token, ttlSeconds }`; the CLI asks the CI platform for a token whose audience is the workspace id |
| `POST /auth/aws` | `{ method, url, body, headers, ttlSeconds }`: a SigV4-signed STS `GetCallerIdentity`, with `x-penv-cloud-org`, the workspace id, among the signed headers. Accepted once: every try, retries included, is signed afresh with a later `x-amz-date` |
| `POST /auth/keypair/enroll` | `{ secret: "pce_...", publicKey }` (Ed25519 SPKI DER, base64) answers `{ credentialId, generation }` |
| `POST /auth/keypair/challenge` | `{ credentialId }` answers `{ nonce }` |
| `POST /auth/keypair` | `{ credentialId, nonce, generation, signature, ttlSeconds }` answers `{ credential, expiresAt, generation }` |

| Refusal | Meaning | The CLI |
|---|---|---|
| `403 refused` | the CI trigger is refused; `pull_request_target` always | names the trigger (`GITHUB_EVENT_NAME`) and says to run on `push`, `pull_request` or `workflow_dispatch` |
| `401 expired` | the token or signed request expired | says so; a CI job is not told to `penv login` |
| `401 unauthorized` | invalid or untrusted, including a GitHub token without `event_name` | points at the trust under Connect a Platform |
| `503 unavailable` | retryable | three more tries, 1, 2 and 4 s apart, then says the token could not be verified right now |

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

| Write limit | The CLI, before sending |
|---|---|
| name `^[A-Za-z0-9_][A-Za-z0-9_.-]*$`, at most 255 characters, no `/` | refuses as `name_invalid`, naming the key |
| path empty or `/`-joined segments: none empty, `.` or `..`; each at most 255 characters, the whole at most 1024; no control, whitespace, bidi or zero-width character, none of ``\ $ ` " ' = # ; & \| < > ( ) { } * ? !`` | refuses as `name_invalid`, naming the key |
| value at most 256 KiB of UTF-8 | refuses as `value_too_large`, naming the key and never the value |
| at most 1000 keys per `PUT` | splits a push into batches after checking every value's size; refuses `--prune` for more than one batch |

A console export of an environment quotes each value in single quotes, or in double quotes with `$` and the backtick escaped; the CLI's `.env` reader takes both.

### Per-key schema

`schema` is the key's object from [`penv schema --json`](https://penv.cloud/docs/cli/schema) minus `name`.

| Refusal | Meaning | CLI exit |
|---|---|---|
| `400 schema_invalid` | a schema field is refused; `field` names it | 1 |
| `400 name_invalid` | a name or path outside the write limits; `detail` names the address | 3 |
| `413 value_too_large` | a value over 256 KiB; `detail` names the address | 3 |
| `413 too_many_keys`, `413 body_too_large` | a request over the limits; the CLI batches, so it names this a bug in penv | 1 |
| `409 machine_change_requires_approval` | the environment requires a second approver, so a machine credential cannot write, delete or change its schema; `detail` says more | 2; a person makes the change in the console, where it becomes a change request |
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
| `409 org_ambiguous` | 2 |
| `400 audience_not_workspace_id` | 2, with the server's `message` |
| `409 cloned` | 2 |
| `409 redacted` | 6 |
| `409 quota_exceeded`, `live_leases`, `exists` | 1, 1, 3 |
| `429` | 1, naming `retry-after` |
| `5xx` twice | 1, naming the status |
| a 3xx | 1 (`cloud_failed`) |
| no answer | 5 (`offline`) |
