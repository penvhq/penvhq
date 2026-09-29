# Changelog

## Unreleased

### Fixed
- **Masking no longer breaks Next.js middleware.** The generated `env.ts` (`mask = true`) and the `penv run` preload replace `Response` with a masking subclass; a `NextResponse` built on the original class then failed Next's `instanceof Response` check, so every request answered 500 ("Expected an instance of Response to be returned"). Responses built on the original class pass `instanceof Response` again.

### Changed
- Comments in generated files are at most two lines each, and every setting in `.penv/config.toml`, target options (`[targets.<name>.options]`) included, says what it does and the values it takes.

## 1.0.0-rc.2

The second release candidate for 1.0.0. penv.cloud has tightened how machines sign in and what it accepts on a write; this release speaks that contract, and fixes arrow keys in choice prompts.

### Breaking
- **A CI or AWS login names its workspace by id.** penv.cloud now takes only the workspace id as the OIDC audience and in the signed AWS `x-penv-cloud-org` header. penv asks GitHub Actions for a token with that audience, and refuses a slug in `@penv=` before any request when it would become the audience or the signed header. **Migration:** replace the workspace slug in `@penv=` with the workspace id from Settings → Organization (the Organization ID) in the penv.cloud console. A person's [`penv login`](https://penv.cloud/docs/cli/login) is unchanged. When penv.cloud answers `audience_not_workspace_id`, its message is printed as written.

### Added
- **[`penv run`](https://penv.cloud/docs/cli/run) keeps the command's own variables.** A provider's key whose variable would change how the command runs (`NODE_OPTIONS`, `LD_PRELOAD`, `BASH_ENV`, `PATH`, `PIP_INDEX_URL`, `YARN_*`, `CARGO_*` and the rest of `reserved.toml`, in any case), or that is not a variable name, fails the run as `reserved_name`, exit 3, naming the parameter. Declared keys are refused before anything is fetched. The child also never inherits a runner's OIDC request token, its runtime, results and cache tokens, the step files (`GITHUB_ENV`, `GITHUB_OUTPUT`, `GITHUB_PATH`, `GITHUB_STATE`), `INPUT_*` or `STATE_*`; run a command that needs one outside `penv run`.

### Changed
- **`penv login` asks you to type the code.** The approval page no longer fills it in: penv opens the page and prints `Enter this code in the browser: ABCD-EFGH. Only approve it if you started this login.` The device label is the host name reduced to what the page keeps (48 letters, digits, spaces and `._-()'@`). After sign-in penv lists the workspaces the login reaches, which may be one; a 404 says the login may have been approved for another workspace and to run `penv login` again and choose it. A workspace's sign-in policy (single sign-on, two-factor) is printed as the workspace wrote it, and an ended login (30 days from last use, 90 at most, or revoked by a password reset, "Sign out everywhere" or account recovery) says to sign in again.
- **Writes are checked before they are sent.** [`penv push`](https://penv.cloud/docs/cli/push) and [`penv set`](https://penv.cloud/docs/cli/set) refuse a name, path or value penv.cloud would refuse (names `^[A-Za-z0-9_][A-Za-z0-9_.-]*$` up to 255 characters, values up to 256 KiB), naming the key and never the value, before any upload. A push of more than 1000 keys goes in batches of 1000, after every value's size is checked, so nothing is half-written; `--prune` is refused when a push takes more than one batch. `name_invalid` and `value_too_large` show the server's detail, `too_many_keys` and `body_too_large` are named as a bug, and `machine_change_requires_approval` says to make the change as a person in the console, where it becomes a change request.
- Each machine-login refusal says what to change: a refused CI trigger (`pull_request_target`) names the trigger and the ones that work, an expired token says it expired, and an untrusted one points at the trust under Connect a Platform. `unavailable` is tried three more times with a growing pause, then named.
- An AWS login is signed afresh for every try, retries included, since penv.cloud accepts a signed request once.
- A `.env` reads a backtick escaped inside double quotes without a warning, as the penv.cloud console exports it.

### Fixed
- **Arrow keys move the selection in every choice prompt** (workspace, project, environment, guards, target options). Up and Down in application cursor mode (`ESC O A`/`ESC O B`, as tmux, screen and many terminals send after an editor) did nothing, and an arrow whose bytes arrived in two reads cancelled the prompt. penv now reads keys itself: raw mode from before the first read until the prompt ends, CSI and SS3 arrows, `j`/`k` and Ctrl-P/Ctrl-N, an unfinished escape given 100 ms before a lone Esc cancels, the cursor and the terminal put back on Enter, Esc, Ctrl-C and errors, and virtual-terminal input on Windows. A prompt is never shown when stdin is not a terminal.

## 1.0.0-rc.1

The release candidate for 1.0.0. It behaves as 1.0.0-beta.4 does; until 1.0.0 only fixes land.

### Added
- **Stability.** What a 1.0 minor or patch release keeps as it is (command names, flags, exit codes, JSON output, the `.env.schema` grammar, `.penv/config.toml` keys) and what it may change: [docs/Design.md §13](./docs/Design.md#13-stability) and the README.

### Changed
- The Copilot CLI guard says it is experimental wherever `init` and `check` describe it: Copilot publishes no schema for its permissions config, so the guard sits outside the stability promise.
- `sslmode=verify-ca` on a sealed Postgres URL is documented as what it already does: it checks the server's certificate and its host name, as `verify-full` does.

### Fixed
- `/install` and a plain `penv upgrade` gave 1.0.0-beta.3 after beta.4 shipped: the release workflow counted the old `@penvhq/*@0.x` package releases as plain penv releases. Only `v` tags count now, so they give this release.

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
