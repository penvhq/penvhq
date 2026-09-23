# penv v1 design

penv is a secrets manager. The cloud (penv.cloud) is the store. The `penv` binary gets the right values into the right process at the right time, validated, and never onto disk in the clear unless a human asks. This document is the contract every crate, target and docs page is built from. Decisions here were taken on 2026-09-07 and 2026-09-08 with the research in the penv-cloud repo (`docs/Research-Dotenv-Dialects-And-Typed-Env.md`, `docs/Research-Agent-Secret-Isolation.md`).

## 1. Principles

1. **The first minute needs no account.** `penv init` and `penv run` work on a plain `.env` before anyone logs in. The cloud is the upgrade that takes the second minute.
2. **Local first.** Everything but sharing works with no account: layered `.env` files, computed values, validation, masking, rotation reminders and leak scanning. The cloud is where values go when a team shares them; locally, a file still wins over the cloud on the machine that holds it.
3. **One binary, no runtime.** No Node, no plugins, no JavaScript config. A static binary per platform.
4. **Validation happens in the binary before the process starts.** A Go service with no adapter still gets a refusal naming the key.
5. **Types are generated, never a runtime.** Language targets are data folders (template plus type map). A new language is a folder.
6. **The agent is a principal.** Trusted to write code, not to hold credentials. Detection changes defaults and friction; it is never the last line of defence.
7. **Say only what is true.** The security claim is a string the binary owns and prints, and it differs by state.
8. **Drop-in or evict.** Every part is independent enough that removing it leaves the rest working, and adding a sibling touches no existing part. This applies to language targets, harness guards, credential kinds, output formats and commands.

## 2. Files in a repository

Two things are committed: `.env.schema`, the contract, and the `.penv/` folder beside it, which holds settings every clone on every machine must agree on: `config.toml` and `targets/` (remembered `gen` output). Neither ever holds a value.

```toml
# .penv/config.toml
[schema]
version = 1                      # the schema language version; init writes it

[public]
prefixes = ["APP_PUBLIC_"]       # beyond the frameworks' own

[rotation]
STRIPE_SECRET_KEY = "2026-09-22T14:05:00Z"   # written by penv set in local mode
```

The version lives here and not in `.env.schema`, because varlock rejects `@schema`. An `@schema=N` line still reads; the config wins, and a version this build does not read is an error.

```dotenv
# @penv=acme/api-gateway @schema=1
# @defaultSensitive=true

# @type=url
DATABASE_URL=

# @type=string(startsWith=sk_) @rotate=90d
STRIPE_SECRET_KEY=

# @type=url @sensitive=false
NEXT_PUBLIC_APP_URL=http://localhost:3000

# @type=port
PORT=3000

# @type=enum(development,staging,production)
NODE_ENV=development
```

Rules:
- The header is the first comment block, whether or not a blank line follows it; a first block that sits directly on a key and contains a header decorator is still the header. `@penv=<org>/<project>` names the cloud project; absent means local mode. `@schema=<n>` is the grammar version the file was written with.
- Decorators sit in `#` comment lines directly above a key. A comment line not starting with `@` is the key's description. The blank line ends a block.
- A decorator is `@name` or `@name=value`. Never positional. Order never matters. Quote a value that contains whitespace or `#`.
- A comment line of `---` (bare or labelled, `# --- api ---`) ends a block the way a blank line does.
- Header decorators: `@penv=<org>/<project>`, `@schema=<n>`, `@defaultSensitive`, `@defaultRequired` (`true`, `false` or `infer`: required only when the schema line carries a value), `@currentEnv=$KEY` (the key whose value names the environment), `@import(<path>, KEY, ...)` (another schema's keys, all of them when none are named; the importing file's own block wins; a loop is an error).
- Vocabulary follows [`@env-spec`](https://varlock.dev) verbatim: `@type=`, `@required`, `@optional`, `@sensitive`, `@sensitive=false`, `@defaultSensitive`, `@defaultRequired`, `@example`, `@docs`. Types: `string`, `number`, `boolean`, `url`, `email`, `port`, `enum(a,b,c)`, plus the spec's function-call constraints such as `string(startsWith=sk_)` and `number(isInt=true)`. There is no `integer` type; integers are `number(isInt=true)`, and a target's `[types]` map may carry an `integer` entry used when that constraint is set. Whitespace inside parentheses is allowed.
- Required is inferred: an empty value is required, a value present on the line is the default and makes the key optional. `@required`/`@optional` override.
- Sensitive is inferred: sensitive by default, public when the key carries a bundler prefix (`NEXT_PUBLIC_`, `VITE_`, `PUBLIC_`, `EXPO_PUBLIC_`, `NUXT_PUBLIC_`, `REACT_APP_`) or `@sensitive=false`. A key that is both prefixed and `@sensitive` fails `check`.
- Adopted from the spec as-is: `@deprecated`, and the boolean `@dynamic`/`@static` pair (accepted and preserved, not used by penv).
- One penv extension, where the spec has no concept: `@rotate=<span>`, a reminder that never blocks. A span is units largest first, each once: `y`, `m` (months), `w`, `d`, `h`, `min`, `s` (`1y6m`, `90d`, `12h`, `30min`). The clock is the value's last write: the cloud's `updatedAt` under a header, the `[rotation]` table in `.penv/config.toml` without one, which `penv set` fills. `check` names keys that are due, overdue or have no recorded write, and exits 0 for all three. `@scope` does not exist; who reads a key is an authorization rule in the console.
- One concept, one name. No aliases. A decorator, type or constraint penv does not act on is a warning from `check`, never a parse failure, so any varlock schema parses; `@plugin` is named as varlock's.
- Nearest `.env.schema` upward from the working directory wins. A monorepo holds one per app.

### Values

Value files layer in the order dotenv-flow, Next.js and Vite use, later files winning:

```text
.env  ->  .env.local  ->  .env.<env>  ->  .env.<env>.local        (test skips .env.local)
```

`*.local` files belong to one machine and are never pushed. `.env.schema`, `.env.example`, `.env.sample`, `.env.template` and `.env.defaults` are not value files.

A value is computed unless it is single-quoted or backticked:

```dotenv
DATABASE_URL=postgres://${DB_HOST}:${DB_PORT:-5432}/app     # ${KEY}, $KEY, ${KEY:-or}, ${KEY-or}; \$ for a literal $
API_URL=if(eq($APP_ENV, production), api.acme.test, staging.acme.test)
REPLICA_URL=penv(production/DATABASE_URL)
LITERAL='${not expanded}'
```

Functions: `ref`, `concat`, `fallback`, `if`, `match`, `eq`, `not`, `and`, `or`, `isEmpty`, `startsWith`, `endsWith`, `forEnv`, `penv`, `random`.

```dotenv
API=match($APP_ENV, production: api.acme.com, staging: stg.acme.com, _: localhost:3000)
DATABASE_URL=postgres://app:${DB_PASS | urlencode}@db/app     # filters: urlencode, base64, lower, upper, trim
SESSION_SECRET=random(32)                                      # generated once per machine
# @assert(if(forEnv(production), startsWith($STRIPE_KEY, sk_live_), true), "test key in production")
```

- `match(subject, label: value, ..., _: value)`: the first label equal to the subject wins, `_` otherwise; no match and no `_` is a validation failure. A case needs a space after its `:`, so `localhost:3000` is a value, not a case.
- `${KEY | filter | ...}` runs filters left to right. An unknown filter is a validation failure naming it. A default cannot hold a `|`.
- `random(N)`, N from 8 to 512, letters and digits from the operating system's generator. `run` generates it once and keeps it in `.env.local` (`.env.test.local` for test); later runs read it back. `check`, `ls` and `scan` never write it; `check` notes it as pending. `push` never sends it.
- `@assert(expression, "message")`, in any block, is checked by `run` (exit 3 before the child starts) and `check`. Only the message is shown; a check that cannot run says so without values.
- **Taint.** A value computed from a sensitive key, directly or through other keys, filters or `penv()`, is masked like a sensitive one, whatever its own `@sensitive` says; `ls` shows it as sensitive and `check` notes it. A key read only to decide (an `if` condition, a `match` subject, `eq`, `startsWith` and the other true-or-false functions) does not taint the value chosen. `penv(env/KEY)` inherits KEY's sensitivity; an address penv cannot judge is sensitive.
- `check` notes a schema that uses penv-only features (`@rotate`, `@assert`, `random()`, `match()`, `penv()`, filters): varlock will not load it. Any other `name(...)` value, a varlock plugin's `op(...)` included, is a validation failure naming the function rather than a literal string handed to the process; single-quote it to pass it as written. `exec` is refused by name: a schema never starts a process. A cycle, a chain deeper than 128 references, or a value that cannot be computed is a validation failure naming the key and never the value. A schema default may be computed the same way.

Precedence, highest first: the process environment (a key it already sets keeps that value, as dotenv and varlock do), then the value files, then the cloud, then the schema default. The key `@currentEnv` names always holds the environment the command is for, so `--env production` and `$APP_ENV` agree. A `$KEY` that nothing sets resolves empty and is reported by the key holding it, never by the referenced name, which in an unquoted password is part of the secret. `@required=forEnv(a, b)` and `@optional=forEnv(a, b)` settle per environment.

An environment name is letters, digits, `-`, `_` and inner dots, and never `local` or a suffix that names a non-value file (`schema`, `example`, `sample`, `template`, `defaults`, `vault`, `keys`, `me`), because it becomes part of a file name. `.env.vault`, `.env.keys` and `.env.me` (dotenv-vault, dotenvx) are never read as values. `@import` refuses a value file: its values would become defaults that `penv schema` prints past every guard.

`penv(address)` reads another environment's value: `penv()` (the key it sits on, the form `@penvhq/varlock-plugin` writes), `penv(KEY)`, `penv(env/KEY)`, `penv(project/env/KEY)`, `penv(org/project/env/KEY)`, with `@penv=` supplying what is left out. Under a header it reads the cloud, with this machine's files for that environment laid over it when it is the same project. Without one it reads that environment's files, and creates an empty `.env.<env>` (or `.env` for development) when there is none. Each address is read once per command.

The files penv writes (from `pull`, `set`) are plain: UTF-8 without BOM, LF, `KEY=value`, upper snake case keys, no `export`, no spaces around `=`, quotes only when needed, a value holding `$` single-quoted so it reads back literally, no duplicates, no comments emitted. `set` and `unset` change their one key's lines and leave the rest of the file as written.

Quoting is the subset Node's `util.parseEnv` and dotenv both read back, and no more: **inside double quotes, `\n` is an escape and nothing else is**. That is how a multi-line value such as a PEM key sits on one line. So the writer folds `\r\n` into `\n`, double-quotes a value that breaks lines escaping those newlines, and refuses it outright when it also holds a `"`, a `\` or a carriage return of its own, because none of those has a portable escape. A value holding a `"` or a `\` and no line break is single-quoted, where nothing is an escape, and is refused when it also holds a `'`. A value goes bare only when it holds no whitespace at all, no `#`, no quote and no backslash; anything else in between is double-quoted with nothing to escape. The reader still decodes `\r`, `\t`, `\"` and `\\` so a file another tool wrote is read rather than mangled, and warns once per value, naming the line and the column and never the character.

## 3. Location

```text
local   value files on disk, .env.schema present, no @penv header or not logged in
cloud   @penv header, credential in the keychain; value files present only when a person wrote or pulled them, and then they win on that machine
```

`penv` with no arguments prints the location (`local` or `cloud`), the version, the environment and the one next command. A folder with no schema is `local`, and its next command is `penv init`.

## 4. Commands

| Command | Does | Fires automatically when |
|---|---|---|
| `penv` | Prints the version, the location (`local` or `cloud`), the environment it resolves to, the value files that environment layers, and the one next command. Reads only; writes nothing | no args |
| `init` | When `.env.schema` already exists and `--force` is not given, keeps it and only adds the ignore lines and the schema version. Otherwise reads every value file beside it, `.env` first, the rest adding keys the earlier ones lack (writing an empty `.env` only when there is no value file at all), writes `.env.schema`, gitignores `.env`, prints what it inferred. Every key is sensitive and required unless bundler-prefixed. A value is copied into the schema as a default only when the key is bundler-prefixed, or the value is a boolean, an integer, a lowercase word of letters, a lowercase slug of at most 32 characters whose segments are joined by `-`, `_` or `.` and where one segment is letters only and no segment carrying a digit runs past four characters (`us-east-1`, `gpt-4o`, `api.internal`, never `a3f9c2d4e5b6`), or a localhost URL with no userinfo and no query. A key named for what it holds keeps its value out however dull it reads and whatever prefix it carries, so `NEXT_PUBLIC_SUPABASE_ANON_KEY` is not copied either: the words are `AUTH`, `KEY`, `SECRET`, `TOKEN`, `PASSWORD`, `PASSWD`, `PASSPHRASE`, `PASS`, `PWD`, `PW`, `CREDENTIAL`, `CRED`, `DSN`, `SALT`, `SEED` and `SIGNATURE`, each matching the word itself or a plural of it in `S` or `ES`; sensitivity still follows the prefix, since a bundler-prefixed value reaches the browser either way. At a terminal, with no agent, it offers a picker over every harness penv knows with the installed ones already chosen; `--guards <NAMES>` (trimmed, deduped, and none when it names nothing) and `--no-guards` decide it without a prompt, and every other run guards the installed set. It then generates for every target this repository uses, resolving the output the way `gen` does; `--output <PATH>` names the file instead, and only when one target applies | `run` finds any value file and no schema |
| `run [--env E] -- cmd` | Validates, injects into the child only, masks child output on every run, and after a successful run scans the browser output folders it wrote | never |
| `push [--env E]` | Moves the shared layers for the environment (`.env`, `.env.<env>`) to the cloud, computed, defaults left out, then deletes those files. `*.local` stays. With nothing named and a person at a terminal, offers the project's environments | `init` when logged in, as an offer |
| `pull [--env E]` | Writes `.env` for development, `.env.<env>` otherwise, so `push` and `run` read it back as the same layer. Same picker as `push` | never |
| `login` / `logout` | Device-code sign in; credential in the OS keychain | `run`/`push` lack a credential and a human is at a TTY |
| `set KEY` / `unset KEY` | Prompted or piped write, never echoed. Under a header it writes the cloud; without one it writes `.env` or `.env.<env>` and records the moment in `.penv/config.toml` when the key has `@rotate` | never |
| `ls` | Names, types, presence; values masked; JSON when stdout is not a TTY | never |
| `check [KEY] [--env E]` | Fails when a value file holding a sensitive value is tracked by git or not ignored (asked of git itself: nested `.gitignore`, `info/exclude` and global excludes count; a file holding no sensitive value may be committed, as Next.js and Vite projects do with `.env`). Schema validity and warnings, missing values, values that cannot be computed, why a key fails, `@rotate` reminders, guard status. Reads what `run` reads; when the cloud cannot be read (CI with no credential) it checks the local files and says so | `run` before exec; `init` after import |
| `scan [PATH...] [--staged] [--install-hook] [--env E]` | Looks for the environment's sensitive values, in every form `run` masks, in what git would commit (or the staged blobs, or the paths named). Names file, line and key, never the value; exit 3 on a finding. `--install-hook` writes a pre-commit hook running `penv scan --staged` and leaves a hook it did not write alone | never |
| `gen <target>` | Writes the typed file for a language target and prints how to import it. `--out <PATH>` says where; without it penv asks at a terminal and skips anywhere else. Either answer is remembered. `--options` prints what this target's options change and what they are set to. No target lists them with their options and the directories they were detected in | `init`/`push` when a directory holds a target's detect file |
| `guard [--check]` | Writes every recognised harness config; `--check` reports coverage | `init`; any command that detects a new harness |
| `reveal KEY` | Prints one value. A person reads it straight; an agent session gets an approval id and exit 4 instead, and `--approval <ID>` prints the value once a person has approved it in the console | never |
| `machine enroll <secret>` | Binds a server keypair from a one-time secret | never |
| `upgrade` | Replaces this binary with the raw asset for its target from the latest GitHub release, after checking the digest the release publishes. `--check` reports what the release carries and changes nothing | never on its own; a schema this build cannot read names the command in its refusal |
| `completions <shell>` | Writes the completion script for bash, zsh, fish, powershell or elvish, generated from the manifest | never |
| `help --json` | The command manifest | never |

Not commands: `env`/`use` (use `--env` or `PENV_ENV`, default `development`), `config`, `doctor` (it is `check`), `agent` (agents are detected).

### Environment selection
`--env`, else `PENV_ENV`, else the key `@currentEnv` names (the process, then `.env` and `.env.local`, then its default), else `development`. Local mode runs any environment its files describe, and warns naming the files used when `.env.<env>` is missing. A human identity running `production` in the cloud is refused unless the console unlocked it for that environment. Machine identities read their bound environment only.

### Output contract
- Human at a TTY: aligned text, no spinners in non-TTY, `NO_COLOR` and `CLICOLOR=0` honoured.
- stdout not a TTY, or an agent marker present: JSON on stdout, one object; errors as JSON on stderr `{ "error": "<code>", "message": "...", "fix": "..." }`.
- `--json` forces JSON; `--agent` forces JSON plus masking and is a parse error combined with any raw-value format.
- Exit codes: 0 ok, 1 error, 2 auth, 3 validation, 4 confirmation required (JSON carries the exact replay command), 5 no credential, 6 environment refused. `help --json` publishes the table.

### The manifest
`penv help --json` emits every command with args, flags (`env` var alias, `default`, `human: true` for flags stripped under agent policy), the `values` an argument accepts where the list is fixed and the `completes` hint where the system answers instead, `revealsValues`, `requiresApproval`, and per-command exit codes, plus `schemaVersion`. The docs generator, completions and the agent skill consume it. Nothing about commands is hand-written twice. Every release carries it as the `manifest.json` asset, emitted by the binary that release ships, so a consumer reads one version's commands without installing that version.

## 5. `run`

```text
find .env.schema (nearest upward)               <1ms
local mode: layer the value files, compute, validate, exec
cloud mode:
  open encrypted cache (keychain key)          ~2ms
  fresh (<60s dev, 0s otherwise)  -> exec
  HEAD /envs/:id with ETag                     ~40ms
  changed -> GET, rewrite cache
  offline -> dev: use cache, warn once a day; other envs: fail closed (exit 5)
no keychain (containers, servers) -> no cache, always online
then: lay every local value file for the environment over the cloud values,
      warn naming each key replaced and the file it came from; compute; validate; exec
```

Target: under 50ms to exec on a warm cache. Injection is into the child environment only. Output masking scrubs the child's stdout and stderr for every sensitive value, boundary-safe across chunk splits, plus base64 and JSON-escaped forms of each value. The masker's secret list is every value present in the resolved environment except keys the schema marks public; a `.env` key the schema does not list is masked and `check` names it as drift. Masking is on for every run, a person's terminal and CI included, because a log is copied further than anyone expects. `--no-mask` is a `human: true` flag honoured only when stdin and stdout are both terminals and no agent is detected. Masking pipes the child's output, so when penv itself is on a terminal it sets `FORCE_COLOR=1` and `CLICOLOR_FORCE=1` for the child, unless `NO_COLOR` or either variable is already set, to keep colour. A sensitive value shorter than 4 characters cannot be masked; `run` names the key and `check` notes it.

### Inside the process

The pipe sees what the child prints, not what it hands a log shipper or sends to a client. `run` closes that with a preload that ships inside the binary, is written to this user's cache folder (`~/.cache/penv`, `~/Library/Caches/penv`, `%LOCALAPPDATA%\penv`; never the shared temp folder, where another account could place its own file first; mode 700) and is loaded into the child's runtime:

| Runtime | How it loads |
|---|---|
| Node, and everything launched through it (Next.js, Vite, Nuxt, Remix, Astro, tsx, npm scripts) | `NODE_OPTIONS=--require`, appended to any existing value |
| Bun | `BUN_OPTIONS=--preload=`; Bun does not unquote it, so a cache path with a space leaves Bun to the pipe alone |
| Deno | `--preload` added after `deno run`, `serve`, `test` and `watch`, when that Deno lists the flag (asked once per binary); `deno task` is left alone |
| Python | `sitecustomize.py` first on `PYTHONPATH`; a project's own `sitecustomize` still runs after it |
| Ruby, Java, .NET, PHP, Go, Rust and other compiled languages | not loaded; the pipe and the output scans are what cover them |

What it masks:

- JavaScript: `console` arguments, including functions a log shipper assigns later, because each method is an accessor that wraps whatever is set. Writes on sockets a server accepted, which cover `res.write`, raw `res.socket.write`, WebSocket frames and HTTPS plaintext. Bodies of a web `Response`, which cover `Bun.serve` and `Deno.serve`.
- Python: log records at creation (`msg`, `args`) and `getMessage`. Bytes sent on accepted connections.

Served bytes keep their length (two bytes kept, the rest `*`), so a `Content-Length` the app already sent stays right. Outbound requests are never touched: a secret in an `Authorization` header to its own API is doing its job. Key names arrive in `PENV_SENSITIVE`, and values are read from the child's own environment, so no second copy exists. Under Deno, only variables the app was granted are read, so the preload never raises a permission prompt. Any failure inside the preload leaves the app as it was.

`--no-preload` and `.penv/config.toml` `[run] preload = false` turn it off for a person at a terminal. An agent session ignores both: otherwise an agent could start a server that echoes its environment and read the secret back over HTTP.

This stops accidental leaks and naive echoes. It is not a sandbox: code written to disguise a value (reversed, XORed, split across writes) gets past any in-process masking. uvloop and other native event loops bypass Python's socket layer.

### Browser safety

A key whose name starts with a public prefix is sent to the browser by its framework, so it is public: `NEXT_PUBLIC_` (Next.js), `VITE_` (Vite, and Remix, SolidStart and TanStack Start on it), `PUBLIC_` (SvelteKit, Astro, Rsbuild), `EXPO_PUBLIC_` (Expo), `NUXT_PUBLIC_` (Nuxt's public runtime config), `REACT_APP_` (Create React App), `GATSBY_` (Gatsby), `VUE_APP_` (Vue CLI), `STORYBOOK_` (Storybook). A custom Vite `envPrefix` or any other goes in `.penv/config.toml`:

```toml
[public]
prefixes = ["APP_PUBLIC_"]
```

A public key defaults to `@sensitive=false`; marking one `@sensitive` is a schema error. A public key computed from a sensitive value (taint) fails `check` and stops `run` with exit 3, naming the key and prefix. A secret that only decides a public value (`NEXT_PUBLIC_MODE=if(startsWith($KEY, sk_live_), live, test)`) is allowed.

Bundlers that inline any referenced variable (Parcel, a hand-written `define`) have no prefix to check, so the output is checked instead. After a run that exits 0, penv reads the files it wrote under `.next/static`, `out`, `dist`, `build`, `.output/public`, `.svelte-kit/output/client`, `storybook-static`, `.vercel/output/static`, and `public` when a Gatsby config is present. React Native bundles are read by name (`*.bundle`, `*.jsbundle`, `*.hbc`) under `android/app/build` and `ios/build`, because their exact paths move between versions; Hermes bytecode is searched as raw bytes and reported without a line. A sensitive value found there in any masked form turns the exit code into 3 and names file, line and key. Symbolic links are not followed and value files are skipped. `penv scan <dir>` runs the same check on demand.

`check` also reads the setups that send keys to the client whatever their prefix. `react-native-config` with a sensitive key in a `.env` file, and babel's `transform-inline-environment-variables`, fail it: both ship every key. A `next.config`, `vite.config`, `nuxt.config`, `astro.config`, `svelte.config`, `webpack.config`, `rsbuild.config`, `rspack.config`, Expo `app.config` or babel config that names a sensitive key is a note, because it may only read the key on the server. Parcel in `package.json` is a note that the build scan covers it.

## 6. Agents

Detection is advisory and ordered, because vendors collide:

1. `AGENT=amp`  2. `COPILOT_CLI=1`  3. `CLAUDE_CODE_CHILD_SESSION=1`  4. `CLAUDECODE=1`  5. `CODEX_THREAD_ID`/`CODEX_SESSION_ID`  6. `GEMINI_CLI=1`  7. `CURSOR_SANDBOX`/`CURSOR_AGENT`  8. `CLINE_ACTIVE`, `ROO_ACTIVE`/`ROO_CLI_RUNTIME`, `OR_APP_NAME=Aider`, `###PS1JSON###` in `PS1`, `AI_AGENT` (parse both `name_version_mode` and `name@version`), `AGENT` against an allowlist, `/opt/.devin`  9. none: process-ancestry walk. Non-TTY plus non-interactive `GIT_EDITOR` tightens, never loosens.

An agent session flips: JSON output, masking on, `reveal` sent through console approval, `pull` refused unless `--i-am-human` is passed by a person, shorter credential TTL, and the session id (`CLAUDE_CODE_SESSION_ID`, `CODEX_THREAD_ID`, `CURSOR_TRACE_ID`, `AMP_CURRENT_THREAD_ID`, `COPILOT_AGENT_SESSION_ID`) stamped on every cloud request for audit.

`reveal` under an agent creates an approval request carrying the key, this machine, the harness and the session, and exits 4 with the id, the console page and the expiry; the value is never in that answer. A person approves on the page, and `penv reveal KEY --approval <ID>` redeems it once, which is the audit row that names them both. A request nobody has answered yet is exit 4 again, a denial is exit 2, and an expired or spent id says to ask for a new one. Two asks for the same key in the same session reuse the one request rather than filling the console with duplicates. The routes are in [Cloud-API.md](./Cloud-API.md).

`guard` writes what each harness enforces, from `.env.schema`, idempotently and additively. Guards are folders (`guards/<harness>/`) with a `guard.toml` (detect paths, files to merge, scope, and the hook response shape the harness expects) and templates; the binary knows no harness by name, and `penv hook <harness>` renders the deny response from the folder. Deny patterns are `.env` and `.env.*` (never `.env.schema`, which the hook allows by name), so a new environment file is covered without a list; the binary merges JSON or TOML fragments without ever weakening an existing rule. Ranked: Claude Code (`.claude/settings.json` deny rules in project scope, `sandbox.credentials` mask block printed for user scope, static-binary PreToolUse hook), Codex (permission profile denying `**/.env` and `**/.env.*`, `ignore_default_excludes=false`), Cursor (`.cursor/cli.json` deny, `.cursor/hooks.json` with `failClosed`), Amp (`amp.guardedFiles.allowlist: []`), Copilot CLI (permissions config), Gemini (`.gemini/settings.json` PreToolUse), Cline (`.clinerules/hooks/`), Windsurf (`.windsurf/hooks.json`). Native Windows has no Claude Code sandbox; `guard --check` says so.

The hook binary is `penv` itself (`penv hook claude-code`), never a script needing an interpreter, because a missing interpreter fails open.

## 7. Language targets

```text
targets/<name>/target.toml   name, output path, detect files, [types] map, [options] table,
                             [[option]] descriptions, [[suggest]] knobs, [[layout]] shapes,
                             import line, optional [check] command
targets/<name>/env.tmpl      minijinja template over the schema JSON
```

In a repository a target is configured in one place: a `[targets.<name>]` section of `.penv/config.toml` holding any `target.toml` field (`output`, an `[targets.<name>.options]` table, and for a language penv does not ship, `detect`, `types`, `import` and the rest), plus an optional `.penv/<name>.tmpl` template. A template with no section overrides only the template. Section names are words (`a-z`, `0-9`, `-`, `_`); any other is ignored. A `.penv/targets/<name>/` folder from before still reads, below the section, and the next `penv gen` of that target moves it in: its fields into the section (the section wins), its `env.tmpl` to `.penv/<name>.tmpl`, then the folder is deleted.

Lookup order: the repository's configuration above (or its old folder), `~/.penv/targets/<name>/`, built in (ts, py). Same layout in all three, and the order is read through rather than winner-takes-all: a folder holding only a `target.toml` inherits the template from the next place, and a field that file does not set is inherited the same way, key by key inside a table as well, so an override naming one `output` or one `[options]` knob keeps the `[types]` map, the other knobs, the `[check]` command and the template it was going to use anyway. A folder holding an `env.tmpl` and no `target.toml` overrides nothing and is refused as the typo it is. `Target.source` is where the `target.toml` came from, and `Target.output_source` is where the `output` field came from, which is not always the same folder.

The template sees the schema JSON with three changes from `penv schema`: a computed default (`if(...)`, `random(32)`, `${KEY}`) is dropped and the key is required, because a literal fallback like `"random(48)"` would be a wrong value outside `penv run`; a key computed from a secret is `"sensitive": true`; each key carries `"public"`. The ts target exports one `env` whose properties are getters. A public key is read by its literal name (`process.env.NEXT_PUBLIC_X`, `import.meta.env.VITE_X`), the only form a bundler inlines, with a computed fallback where nothing inlined it and the runtime has no `process` (edge, Workers). Every other key is read by computed name through one `read(name)`, which no bundler inlines, so no build can write a secret into client code; `read` tries `process.env`, `Deno.env`, then `Netlify.env`, and throws `<KEY> is server-only` where none exists (a browser). `runtime = "workers"` reads bindings from `cloudflare:workers` instead, which works under any compatibility flags. Checked with real builds: Next.js 16 (server component, Node route, edge route, client component in Chromium), Vite 8 (client in Chromium, SSR in Node), Parcel 2.16 (in Chromium), workerd with and without `nodejs_compat`, Node 22, Bun 1.4 and Deno 2.9. A bundler told to inline the whole `process.env` object defeats this, and the post-build scan catches it.

### Ask, never guess

Two things decide where a generated file goes, and nothing else ever does:

1. an explicit `--out` (`gen`) or `--output` (`init`), relative to the repository root;
2. the remembered repo override, but only when it names `output`.

Detection has two jobs and no third. It says whether a target is relevant to this repository at all, and it offers directories to choose from. A directory holding **any one** of the names in `detect` counts, walking three levels deep and skipping `node_modules`, `dist`, `build`, `target` and every directory whose name starts with a dot, which is how `.git` and `.next` are skipped without naming them. `ts` detects `package.json` or `tsconfig.json`; `py` detects `pyproject.toml`, `requirements.txt`, `setup.py` or `Pipfile`. A workspace root is a suggestion like any other, never a verdict about anything, and it is offered last: the rest come shallowest first, by name, so Enter in a monorepo lands on a package.

With neither source, at a terminal with no agent, penv asks once per relevant target:

```text
#  PATH
1  apps/web/src/env.ts
2  apps/api/src/env.ts

where should env.ts go? [apps/web/src/env.ts] (Enter, number, path, none):
```

The table is shown only when there is more than one suggestion. The default in brackets is the first suggestion joined with the target's `output`; Enter takes it, a number takes another off the list, a typed path is used as written relative to the repository root, and `none` skips the target. With neither source and nobody to ask, penv writes nothing for that target and reports it skipped, with the reason `pass --output <PATH>` (`--out` under `gen`). `gen --check` never asks either, and reports the same skip with exit 0.

The answer is remembered in `.penv/config.toml` as `[targets.<name>]`: `output`, and an `options` table holding every knob at the value in effect, so a repository is asked once, ever. `penv gen <name> --options` lists each knob with what it changes and what it takes. That holds for an explicit flag too, and for an answer that happens to equal the built-in default, because it was still an answer. Only `output` and `options` are written; every other field a section holds is kept, and a value edited in place is read back and written back. `.penv/config.toml` is committed. penv rewrites it as TOML, so comments in it do not survive a write. Only the generated file moves: `.env.schema` and the gitignore stay at the root. A path outside the repository is refused rather than written, whether it arrived as an absolute path or as a `..`, and `.` and `..` are taken out of a path before anything downstream sees it.

A `[[layout]]` block shapes the output inside a chosen package: `when` is a path under the package holding one `/*/`, where the `*` stands for a directory name, `output` reads the same name back, and `root` is the directory the language imports from. A `when` with no `/*/` in it matches nothing and is refused when the folder loads. `py` uses one for src layouts, so a package holding `src/billing/__init__.py` is offered `src/billing/penv_env.py` and imports `from billing.penv_env import env`.

penv never edits a `tsconfig.json`, a `package.json` or any other build config. It prints the import line instead, from the `import` the target names: `{specifier}` is the path the language imports by and `{module}` its dotted form. When the target names a `paths_from` file, penv follows its relative `extends` chain, resolves each `paths` target against the effective `baseUrl`, and prints the alias when one is proven to reach the output; otherwise it prints the relative import.

Targets receive `penv schema --json` and nothing else: no values, no network. Whatever `[options]` holds reaches the template as `options`, unread by Rust, so a folder names its own knobs. Each one is described by a `[[option]]` block — `name`, `default`, `values` (the words it takes, empty for free text) and a one-line `about` in plain English saying what it changes — which is where `penv gen <target> --options`, the `OPTIONS` column of the listing and the comments in the remembered override all come from. A `[[option]]` whose `default` is not one of its `values`, and an `[options]` key no `[[option]]` describes, are both refused when the folder loads: a knob nobody can find is a knob nobody has. A `[[suggest]]` block asks penv to work one out from the chosen package: `option` names the `[options]` key (its entry there is the default), and each `[[suggest.rule]]` carries the `value` it sets outright, from the `files` — optionally with the text they must `contain` — that imply it. A rule naming neither `files` nor `contains` is refused when the folder loads. `contains` reads only the lines of a file that are not its own comments, so a dependency somebody commented out is not one. The prompt offers the default first and then each rule's value once. penv asks only where a rule and the default disagree, and takes the default silently when nobody is there to ask; a `contains` rule never settles a knob unasked, so a non-interactive run remembers the path with every knob still at its default:

```text
which runtime reads the env? [vite] (node, vite, deno):
use pydantic types? [true] (false, true):
```

`ts` takes `key_case = "upper" | "camel"` for the property names it exports, and `runtime = "node" | "vite" | "deno"` for the one accessor the whole file reads through (`process.env[key]`, `import.meta.env[key]`, `Deno.env.get(key)`); a `vite.config.*` or a `deno.json` in the chosen package suggests the runtime. A key with a default in the schema reads through the accessor (`read("PORT") ?? "3000"`) instead of widening to `| undefined`, and the Standard Schema validator returns the typed `env`. `py` renders on the standard library alone, and `pydantic = true` swaps `HttpUrl` and `SecretStr` back in; a `pyproject.toml` listing pydantic suggests it.

One fixture schema is snapshot-rendered through every target in CI. `gen --check` compiles the output when the toolchain is present, and says which tool it looked for when it is not. `[check] command` and `probe` may offer alternatives in their first element as `python3|python|py`, and the first that both resolves and answers the probe wins; `[check] bin` names directories under the chosen package looked in before PATH, which is how `ts` finds a project's own `node_modules/.bin/tsc`. Resolution is PATHEXT-aware, so `tsc` finds `tsc.cmd` on Windows and not the shell script beside it. `[check.files]` carries whatever globals the output needs to compile.

## 8. Cloud

The API already exists in penv-cloud (`/api/v1/secrets`, `/api/v1/auth/{oidc,aws,keypair,revoke}`, `/api/v1/dynamic`). A machine identity is bound to one project and environment with a role and proves itself by one of: `oidc` (platform JWT exchanged for a short-lived credential; the binary fixes the lifetime at 15 minutes), `aws-iam` (SigV4-signed STS request), `bound-keypair` (Ed25519 challenge-response with a generation counter, for hosts that can attest nothing), `token` (`pck_` bearer with a required expiry; the last resort). The variable is `PENV_TOKEN`.

Cloud-side: the schema is stored per key next to values; the console renders and edits it; `push` and `pull` carry it. Push targets (Vercel, Netlify, etc.) are cloud integrations, not CLI features. There is no fetch SDK and nothing to install in an app; the one piece of penv that runs inside an app is the preload `run` writes (section 5).

**Implementation debt (penv-cloud).** What the CLI and `@penvhq/varlock-plugin` read, and the server does not yet guarantee. The first two are missing; the rest must be verified against the deployed API before the plugin's first release is announced:

1. `updatedAt` (RFC 3339) on every key in `GET /envs`. `@rotate` counts from it; until it arrives, `check` reports cloud keys as having no recorded write.
2. A verified override round trip. A local value file wins over the cloud on its machine, but today `run` cannot tell a deliberate override from a stale pulled copy, so every override warns the same way. The fix: `pull` records each key's `version` beside the file it writes, and `run` compares it with the cloud's, so it can say "stale: the cloud is at v7, `.env.production` holds v5" instead of "replaced". That needs `version` on every key in `GET /envs` (present) and `updatedAt` (above).

Must verify:

3. **An environment name holding `/` is one path segment.** Clients send `feature/foo` as `/api/v1/envs/acme/api/feature%2Ffoo`. The server must decode each segment on its own, after routing: a framework or proxy that decodes `%2F` before routing sends that request to project `api`, environment `feature`, key `foo`, or to a 404. Test through the production edge (Vercel), not only the app.
4. **Key names are data, never object keys with a prototype.** `__proto__`, `constructor` and `toString` are valid key names. Storing, listing and returning them must not touch `Object.prototype`: keep them in arrays or `Object.create(null)` maps, and check the JSON `GET /envs` returns lists `__proto__` as an ordinary key.
5. **A `pck_` machine token is a bearer on `GET /envs`.** The plugin sends it directly, with no exchange. Confirm it is accepted there, that `403` answers an environment outside its scope and `401` an expired or revoked token, with the `{ "error": ... }` bodies the API section lists.
6. **No redirects on the API.** Both clients refuse a redirect instead of following it, so `/api/v1/*` must answer directly, with no trailing-slash or locale redirect in front of it.
7. **`GET /envs` is JSON on every status.** An HTML error page from the platform in front of the app (timeouts, 5xx) reaches the client as "not JSON". Serve JSON bodies for errors the app itself does not produce, or document which statuses may carry HTML.

### Deploying

Credentials are tried in this order: `PENV_TOKEN`, a person's login, an enrolled keypair, the platform's OIDC token (GitHub Actions, GitLab `ID_TOKEN`, or `PENV_OIDC_TOKEN`), then AWS: keys in the environment, web identity (`AWS_WEB_IDENTITY_TOKEN_FILE` + `AWS_ROLE_ARN`, which EKS IRSA sets; exchanged with STS `AssumeRoleWithWebIdentity`, honouring `AWS_ENDPOINT_URL_STS`), then the container endpoint (`AWS_CONTAINER_CREDENTIALS_RELATIVE_URI` for ECS task roles, `..._FULL_URI` with `..._AUTHORIZATION_TOKEN[_FILE]` for EKS Pod Identity). A full URI is called only over https or to loopback and the ECS and EKS link-local hosts, never with user info in it, and no request follows a redirect. Whatever the kind, the server receives a signed `GetCallerIdentity`, never a secret key.

An enrolled keypair lives in the OS keychain. A container has none, so `penv machine enroll` does not apply there; a container proves itself with `PENV_TOKEN`, OIDC or its AWS role. With no keychain there is also no value cache: every start reads penv.cloud.

```yaml
# GitHub Actions
permissions: { id-token: write, contents: read }
steps:
  - uses: actions/checkout@v4
  - run: curl -fsSL https://penv.cloud/install | sh
  - run: penv check --env staging
  - run: penv run --env staging -- npm run build      # exit 3 when a secret lands in browser output
```

```dockerfile
# Docker, ECS, EKS, Cloud Run, Fly: values at start, never in a layer
COPY --from=ghcr.io/penvhq/penv:1 /penv /usr/local/bin/penv
RUN --mount=type=secret,id=penv_token,env=PENV_TOKEN penv run --env production -- npm run build
ENTRYPOINT ["penv", "run", "--env", "production", "--"]
CMD ["node", "server.js"]
```

AWS Lambda managed runtimes (Node.js, Python, Java, .NET, Ruby) take `packaging/lambda/`: a layer with `bin/penv` and `penv-wrapper`, enabled by `AWS_LAMBDA_EXEC_WRAPPER=/opt/penv-wrapper` and `PENV_ENV`. The wrapper runs from `LAMBDA_TASK_ROOT`, keeps the preload under `/tmp/.cache`, and refuses to start without an `.env.schema` rather than let `run` write one. `build-layer.sh` fetches the release through `install.sh`, so the layer gets the same signature and digest checks as an install. Vercel and Netlify functions, Cloudflare Workers and Vercel Edge do not run penv at runtime: their values come from penv.cloud.

**Which authorities are trusted.** Every request penv makes trusts the Mozilla roots compiled into the binary, so a host with no CA bundle, such as a `scratch` image, still reaches penv.cloud, and a network that re-signs TLS is refused. `SSL_CERT_FILE` replaces those roots with the PEM bundle it names, the way OpenSSL, curl and Python read it: that is how a TLS-inspecting proxy is trusted. A named file that cannot be read or holds no certificate is an error, never a silent fallback. In an agent session the bundle is used only when this user cannot write it, because an agent that could plant a bundle, and a proxy with it, would read penv's credential and every value; a bundle the user cannot write passes, and so does a distribution's own root-owned trust store (`/etc/ssl/certs/ca-certificates.crt`, `/etc/pki/tls/certs/ca-bundle.crt`, `/etc/ssl/cert.pem`, `/etc/ssl/ca-bundle.pem`), which is the only case that lets an agent running as root behind a proxy reach penv.cloud; a root agent could change that store, but that is the machine's trust, not a file planted for penv.

## 9. Claim

```text
local:  penv validates your .env, keeps values out of your agent's output, and blocks it from reading the file where its harness allows.
cloud:  penv keeps secrets out of the files, the repo and the shell history your coding agent reads, and out of the output it captures. It cannot stop a process running as you from looking, so every value is short-lived, scoped and attributable to the session that used it.
```

Only the cloud sentence is marketed. Both are printed by `guard --check`.

## 10. Distribution

The release is the source of truth: archives for linux x86_64/aarch64 (musl), macOS x86_64/aarch64, windows x86_64/aarch64, and beside each archive the raw binary `penv-<tag>-<target>`, which is what `upgrade` and the installers download because the binary carries no decompressor. One `penv-<tag>-<target>.sha256` covers both, and `penv-<tag>-<target>.sha256.sig` beside it is that file's Ed25519 signature, base64 on one line. Every download is checked against the digest before it lands, and the digest is only trusted once the signature over the file carrying it holds. One `manifest.json` covers the whole release: the `sign` job runs `help --json` from the linux x86_64 binary it just uploaded and puts the result beside it, still in the draft, so `releases/latest/download/manifest.json` is what the documentation site generates from.

**The release key.** One Ed25519 pair. `cargo run -p penv-release -- keygen` prints it: the private line becomes the `PENV_SIGNING_KEY` repository secret, the public line is pasted into `PUBLIC_KEYS` in `crates/penv/src/upgrade.rs` and `public_keys` in `install.sh`, both as the raw 32 key bytes in base64. The release is created as a draft and every asset lands in that draft, so nothing is downloadable while it is unsigned. The `sign` job in the release workflow compiles `penv-release` in a step that carries no secret, downloads every `.sha256` asset, signs each one in a step where `PENV_SIGNING_KEY` is the only thing added, uploads the `.sig` beside it and only then publishes the release. Without the secret it stops with an `::error::` naming it and the draft stays a draft, and the npm publish waits on that job, so nothing unsigned is ever published. `penv upgrade` fetches the `.sig`, verifies it against every listed key and refuses with `signature_missing` or `signature_invalid` before it reads a digest; a build whose `PUBLIC_KEYS` is empty refuses to upgrade at all with `unsigned_build`, because it cannot tell a release from a file somebody put in its place. Rotation is three steps and no flag day: add the new key to the list, ship a release signed with the old key that carries both, then drop the old key from the release after that. A compromised key has no revocation path in binaries already installed, since a build trusts the list compiled into it and reads no revocation anywhere: the answer is to rotate, say so, and have people reinstall from the installer.

What the installers can prove is less. `install.sh` verifies the signature with `openssl pkeyutl -verify -pubin -rawin` where OpenSSL 1.1.1 or newer is on PATH, which is where Ed25519 arrived, and refuses a release whose signature is wrong or missing; where there is no such OpenSSL, or no key in the script yet, it prints one dim line saying the signature went unchecked and installs on the digest alone. `install.ps1` prints that line always: .NET carries no Ed25519 and this installer takes no dependency to get one. `aarch64-pc-windows-msvc` sits under "Tier 1 with Host Tools" in rustc's platform support list, and the windows runner image carries the MSVC ARM64 toolset, so it cross-builds beside x86_64 with only `rustup target add`.

**One address.** `https://penv.cloud/releases/latest` answers with the release JSON and `https://penv.cloud/releases/download/<tag>/<asset>` with an asset, both by redirect, TLS on every hop. Nothing installed anywhere carries a repository path: the slug is Cargo.toml's `[workspace.package] repository`, which is metadata the npm packages read at release time, and one redirect constant in penv-cloud. Moving the repository to another owner changes those two lines. `PENV_RELEASE_BASE` is an installer variable only, and is what the installer tests point at a local directory; the binary carries the one address and reads no override.

Three ways in, in this order:

```bash
curl -fsSL https://penv.cloud/install | sh     # install.sh, POSIX sh
irm https://penv.cloud/install.ps1 | iex       # install.ps1, no param block, no $PSScriptRoot
npm i -g @penvhq/cli                           # the launcher package
```

`install.sh` and `install.ps1` sit at the repository root and are release assets, so `/install` is served from `releases/latest/download/install.sh` and every installer is the one that release was cut with rather than whatever `main` holds. Both detect the host, resolve the tag (`PENV_VERSION` pins one, normalised to a leading `v` and refused when it carries a slash or whitespace), verify the digest with `sha256sum`, `shasum -a 256` or `Get-FileHash`, install to `$HOME/.penv/bin/penv` or `%USERPROFILE%\.penv\bin\penv.exe` (`PENV_INSTALL_DIR` moves it), and refuse to run as root or elevated unless `PENV_ALLOW_ROOT=1` or `PENV_ALLOW_ELEVATED=1` says otherwise. The shell installer downloads into the install directory itself and renames within it, so the swap is atomic and lands over a running penv, and the temp file goes on every exit path. Neither edits an rc file: the shell installer prints the line for the shell `$SHELL` names, and the PowerShell one writes the user PATH only when `-AddToPath` is passed, or `PENV_ADD_TO_PATH=1` when it is piped through `iex` and has no flags to take. That write goes through the registry raw, keeping the value kind so a PATH holding `%USERPROFILE%` is not flattened. `install.test.sh` and `install.test.ps1` run each against a fake release served from a directory, and skip where there is no python to serve it.

The npm side is the pattern Biome and Turborepo publish Rust binaries with, and has no postinstall step. `@penvhq/cli` is a launcher holding one `bin/penv.js`, which resolves `@penvhq/cli-<platform>-<arch>/bin/penv` with `require.resolve`, spawns it with the same arguments and stdio, and answers with the child's exit code, re-raising its signal. Its `optionalDependencies` name all six platform packages pinned to the release version, and each of those carries `os`, `cpu`, an `exports` map publishing that one binary path, and nothing else, so npm installs the one that matches and skips the rest. There is no `libc` field: the linux binary is static musl and runs on glibc hosts too. When npm skipped a platform package silently, the launcher says which one is missing and that the two installers above need no npm. `npm/pack.mjs` writes all seven packages from the built binaries in the release workflow, and the publish job needs an `NPM_TOKEN` secret and `id-token: write` for provenance. All seven publish under a dist-tag the version decides, `latest` for a plain one and `next` for anything carrying a `-`, so a prerelease never becomes what `npm i -g @penvhq/cli` installs; a version already on the registry is skipped rather than republished, so a rerun of the workflow finishes.

The workflow creates the release once as a draft, in a job the six build jobs need, so nothing races to create it and the builds only upload into it; the `sign` job is what takes it out of draft. That job refuses the tag first when `v<tag>` is not the `[workspace.package]` version in `Cargo.toml`, and it is where the two installers are uploaded.

`penv upgrade` replaces the binary an installer placed. It refuses under `node_modules` and names `npm i -g @penvhq/cli`, alongside the same refusal for Homebrew, Nix, winget and Scoop, because that binary belongs to the package a manager put it in. Homebrew tap and winget are still to come.

**The image.** `ghcr.io/penvhq/penv` holds the signed static Linux binary at `/penv` for amd64 and arm64, on `scratch`, as uid 65532, with OCI labels for source, version, revision and licence. Tags: the version always; `X.Y`, `X` and `latest` for a plain release; `next` for a prerelease. The release workflow's `image` job runs after the `sign` job publishes the release. `packaging/docker/fetch.sh` takes both binaries through `install.sh`, and fails where `install.sh` would install on the digest alone for want of OpenSSL. `packaging/docker/Dockerfile` copies them in with no build step, so the image holds the bytes the release signed, and both platforms build without emulation. Release binaries are stripped (`[profile.release] strip = true`): the static x86_64 binary is 11.4 MB where it was 14.8 MB. The first image is published by the first release tagged after this change; `v1.0.0-beta.1` has none. Release builds run only in CI; this development machine runs `cargo check` and `cargo test` with two jobs.

## 11. Crate layout

```text
crates/penv           the binary: clap commands, output, exit codes, manifest
crates/penv-schema    .env.schema parser, IR, JSON, validation (no I/O)
crates/penv-dotenv    the safe-subset .env reader and writer
crates/penv-agent     detection and policy (pure functions over an environment map)
crates/penv-mask      streaming scrubber
crates/penv-targets   target loading and rendering
crates/penv-guards    harness guard loading and merging
crates/penv-cloud     HTTP client, credential kinds, cache, keychain, release signatures
crates/penv-release   the release signing tool: keygen and sign. Never published, never shipped
```

Each crate depends only on `penv-schema` and the standard library unless the brief for that crate says otherwise. Keep the dependency set small: clap, serde, serde_json, toml, minijinja, thiserror, and for the cloud crate ureq with rustls, keyring, and a small AEAD. No async runtime.

## 12. Phases

1. Local mode: schema, dotenv, `init`, `check`, `ls`, `run` with detection and masking, `gen` (ts, py), `guard`, `help --json`, bare `penv`, CI on GitHub Actions.
2. Cloud mode: `login`, `push`, `pull`, `set`, `unset`, `reveal`, `machine enroll`, cache, environment refusal, audit stamping. penv-cloud is the first project through it, by hand. 2b is `reveal` under an agent, through console approval.
3. Distribution: release workflow, installers, npm shim, `upgrade`, `completions`.
4. Local first: the value-file cascade, `@currentEnv`, `@import`, computed values and `penv()`, varlock schemas parse with warnings, `@rotate` reminders with `.penv/config.toml`, local `set`/`unset`, `scan`. The varlock plugin that makes penv.cloud a varlock backend is a separate package.
