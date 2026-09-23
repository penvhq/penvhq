# Changelog

## 1.0.0-beta.2

### Added
- **Sealed runs.**
  - `@hosts` names where a value may go. Under an AI agent, or with `penv run --sealed`, the command holds a placeholder shaped by the key's type.
  - A proxy inside `penv run` puts the value into request headers and the request line for those hosts only. Bodies keep the placeholder. Echoed values, raw or encoded, come back as the placeholder.
    - WebSocket through an allowed host: the handshake gets the value, and server messages come back with it swapped for the placeholder.
    - `AWS_SECRET_ACCESS_KEY` can be sealed: the SDK signs with a placeholder and the proxy signs each request again (SigV4) with the real key, which never goes on the wire.
  - `postgres://` and `redis://` URLs go through local proxies. penv logs in with SCRAM-SHA-256, MD5 or cleartext; for Redis it uses `AUTH` and `HELLO … AUTH`.
- **Local-first values.**
  - Value files layer in the order `.env`, `.env.local`, `.env.<env>`, `.env.<env>.local`.
  - The environment comes from `@currentEnv`.
  - Computed values: `${KEY}`, filters (`urlencode`, `base64`, …), `match`, `if`, `fallback`, `random(N)`.
  - `penv()` reads another environment, project or org, and `penv()` with no argument reads the key it sits on.
  - `@assert` and `@rotate`.
- **Masking by default** in every `penv run`, and inside Node, Bun, Deno and Python processes through a preload. The file `penv gen ts` writes now masks `console` and `Response` bodies in deployed code (Node, Bun, Deno, edge, Workers) with no wrapper process.
- **Browser safety.** A public key built from a secret fails `check` and `run`. After a build, the client output (`.next/static`, `dist`, `build`, `out`, React Native bundles) is scanned.
- **More typed code.** `penv gen` writes go, rust, php, java and csharp as well as ts and py. A secret field prints `[redacted]` in each; CI compiles and runs every one.
- **`penv check` reads the code.** It names each variable the code reads and `.env.schema` doesn't declare, and each declared key nothing mentions. `--strict` fails on undeclared reads.
- **`penv why KEY`**: where a value comes from, what it's built on, and whether it's masked, public or sealed. It never prints the value.
- **Encryption at rest**, opt-in: `penv encrypt` and `penv decrypt`, `[local] encrypt`, and a key in the OS keychain.
- **`penv bundle`**: one environment encrypted into `.penv/<env>.bundle`, which `penv run` reads with `PENV_BUNDLE_KEY` where it would otherwise read penv.cloud.
- **Settings file.** `.penv/config.toml` lists every setting with its default and what the other value does. Writes keep comments.
- **Providers.** `@penv=<provider>:org/project`, `--provider`, and `[providers.<slug>] url`. A root that only the settings file names never receives a token, OIDC or AWS credential.
- **Deployment.** ECS task roles, EKS IRSA and Pod Identity, an AWS Lambda layer, and a signed `scratch` Docker image. `SSL_CERT_FILE` names a CA bundle; in an agent session, one the user can write is refused.
- **`@penvhq/varlock-plugin`**: penv.cloud from varlock, published from `packages/varlock-plugin`.
- **Git exposure.** `penv check` fails when a value file holding a secret is tracked or not ignored.

### Changed
- `gen ts` exports one `env` for server and client code. Secrets are read by computed name, so no bundler inlines them.
- Target settings moved into `[targets.<name>]` in `.penv/config.toml`; old `.penv/targets/` folders migrate on the next `gen`.
- The schema version moved from `.env.schema` to `[schema] version`.
- `penv guard` also denies penv's local key file, `~/.config/penv/local.key`.

### Fixed
- A `${KEY:-fallback}` whose key was unset masked the fallback.
- The sealed proxy refuses requests whose length headers disagree, or that combine a length with chunked encoding.
