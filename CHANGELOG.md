# Changelog

## 1.0.0-beta.4

### Changed
- Every OIDC, AWS and keypair exchange asks for its credential's lifetime (`ttlSeconds`): 5 minutes under an agent, 15 otherwise, and an agent never reuses one past 5 minutes.
- [`penv hook`](https://penv.cloud/docs/cli/hook) lets [`penv reveal`](https://penv.cloud/docs/cli/reveal) through: under an agent, reveal asks a person to approve before it prints anything. [`penv pull`](https://penv.cloud/docs/cli/pull) is still refused.
- penv refuses a command the chosen provider does not declare (`unsupported`), naming the provider and the capability.
- `install.ps1` verifies the release signature with OpenSSL 1.1.1 or newer from PATH, as `install.sh` does.
- A penv installed by npm is told `npm i -g @penvhq/cli@<version>`, since npm's `latest` tag holds no prerelease.
- `penv --help`, [`penv help`](https://penv.cloud/docs/cli/help) `--json` and the shell completions read one summary per command.
- The `guard --check` claim for cloud mode says what we do: we record every value we hand out as a read.
- Messages name penv.cloud, not penv-cloud.
- Offline, only `development` falls back to the local value files; any other environment stops with `offline` (exit 5), as the offline message already said.
- A target, guard or credential kind is added as a folder or file; a guard's rank and a credential kind's place in the lookup order are data in it.

## 1.0.0-beta.3

### Added
- **Editor support.** [`penv lsp`](https://penv.cloud/docs/cli/lsp) serves `.env.schema` over the Language Server Protocol: the problems [`penv check`](https://penv.cloud/docs/cli/check) reports for the schema, completion and hover for every decorator, function and filter (marked penv, [@env-spec](https://varlock.dev/env-spec/overview/) or varlock-only), go-to-definition for key references, and an outline. It reads no value file and sends no request.
- **penv for VS Code**, in `packages/vscode`. It runs `penv lsp`, finds the platform binary behind an npm install, and runs beside [varlock](https://varlock.dev)'s @env-spec extension.
- [`penv upgrade`](https://penv.cloud/docs/cli/upgrade) `[latest|next|<version>]`: `next` follows prereleases, a version pins or steps back. `latest` stays the default.
- **Write-only keys.** A key we withhold on penv.cloud from a person's login or a static token is `redacted`: present, never missing. `run` and `bundle` refuse it (`redacted`, exit 6) naming every key and the environment, unless a local layer such as `.env.<env>.local` supplies it. `pull` writes `# penv:redacted KEY` in place of the value, and a local-mode `run` refuses on that marker too. `check` notes it, `ls` shows `redacted`, `why` says it is withheld, and `reveal` refuses it.

### Changed
- Once a plain release exists, `/install` and `penv upgrade` no longer give a prerelease.

### Fixed
- An AWS login (keys, web identity or a container role) signs the workspace in `x-penv-cloud-org`, which we require on penv.cloud; without it we refused every AWS exchange.
- A taken project name and an org slug two workspaces share each get their own message (`project_taken`, `org_ambiguous`) instead of "matches more than one project or environment"; a value we cannot decrypt (`undecryptable`) and a lowercase key name on push (`name_invalid`) are named too.
- `gen ts` declared `inlined` when no public key used it (`noUnusedLocals`), and `Response.json` lacked `override` (`noImplicitOverride`). The targets job now type-checks the output with both on.
- [`@penvhq/varlock-plugin`](./packages/varlock-plugin/README.md) 0.1.1 ([varlock.dev](https://varlock.dev)) refuses an org, project or environment made only of dots; URL parsing would have read another path.

## 1.0.0-beta.2

### Added
- **Sealed runs.**
  - `@hosts` names where a value may go. Under an AI agent, or with [`penv run`](https://penv.cloud/docs/cli/run) `--sealed`, the command holds a placeholder shaped by the key's type.
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
- **Masking by default** in every `penv run`, and inside Node, Bun, Deno and Python processes through a preload. The file [`penv gen`](https://penv.cloud/docs/cli/gen) `ts` writes now masks `console` and `Response` bodies in deployed code (Node, Bun, Deno, edge, Workers) with no wrapper process.
- **Browser safety.** A public key built from a secret fails `check` and `run`. After a build, the client output (`.next/static`, `dist`, `build`, `out`, React Native bundles) is scanned.
- **More typed code.** `penv gen` writes go, rust, php, java and csharp as well as ts and py. A secret field prints `[redacted]` in each; CI compiles and runs every one.
- **`check` reads the code.** It names each variable the code reads and `.env.schema` doesn't declare, and each declared key nothing mentions. `--strict` fails on undeclared reads.
- **[`penv why`](https://penv.cloud/docs/cli/why) `KEY`**: where a value comes from, what it's built on, and whether it's masked, public or sealed. It never prints the value.
- **Encryption at rest**, opt-in: [`penv encrypt`](https://penv.cloud/docs/cli/encrypt) and [`penv decrypt`](https://penv.cloud/docs/cli/decrypt), `[local] encrypt`, and a key in the OS keychain.
- **[`penv bundle`](https://penv.cloud/docs/cli/bundle)**: one environment encrypted into `.penv/<env>.bundle`, which `run` reads with `PENV_BUNDLE_KEY` where it would otherwise read from us on penv.cloud.
- **Settings file.** `.penv/config.toml` lists every setting with its default and what the other value does. Writes keep comments.
- **Providers.** `@penv=<provider>:org/project`, `--provider`, and `[providers.<slug>] url`. A root that only the settings file names never receives a token, OIDC or AWS credential.
- **Deployment.** ECS task roles, EKS IRSA and Pod Identity, an AWS Lambda layer, and a signed `scratch` Docker image. `SSL_CERT_FILE` names a CA bundle; in an agent session, penv refuses one you can write.
- **`@penvhq/varlock-plugin`**: penv.cloud from [varlock](https://varlock.dev), published from `packages/varlock-plugin`.
- **Git exposure.** `penv check` fails when a value file holding a secret is tracked or not ignored.

### Changed
- `gen ts` exports one `env` for server and client code. Secrets are read by computed name, so no bundler inlines them.
- Target settings moved into `[targets.<name>]` in `.penv/config.toml`; old `.penv/targets/` folders migrate on the next `gen`.
- The schema version moved from `.env.schema` to `[schema] version`.
- [`penv guard`](https://penv.cloud/docs/cli/guard) also denies penv's local key file, `~/.config/penv/local.key`.

### Fixed
- A `${KEY:-fallback}` whose key was unset masked the fallback.
- The sealed proxy refuses requests whose length headers disagree, or that combine a length with chunked encoding.
