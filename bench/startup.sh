#!/usr/bin/env bash
# Startup time and peak memory of `run -- true` for penv, varlock, dotenvx and dotenv-cli
# on the same files. Each tool's npm build is installed at a pinned version; a standalone
# binary joins the table when its variable points at one.
#
#   bench/startup.sh
#   PENV=~/.penv/bin/penv VARLOCK=~/.config/varlock/bin/varlock DOTENVX=/usr/local/bin/dotenvx bench/startup.sh
#
# Needs: node 18+ and npm, git, python3. Writes startup-results.json in the current directory.
set -uo pipefail

. "$(dirname "$0")/lib.sh"

PENV_VERSION=${PENV_VERSION:-1.0.0-rc.1}
VARLOCK_VERSION=${VARLOCK_VERSION:-1.21.0}
DOTENVX_VERSION=${DOTENVX_VERSION:-2.31.1}
DOTENV_CLI_VERSION=${DOTENV_CLI_VERSION:-11.0.0}
RUNS=${RUNS:-30}
OUT=${OUT:-$PWD/startup-results.json}
for tool in node npm git python3; do command -v "$tool" >/dev/null || die "$tool is required"; done

WORK=$(mktemp -d "${TMPDIR:-/tmp}/penv-startup.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
echo "installing from npm into $WORK ..."
(cd "$WORK" && printf '{"private":true}' > package.json && npm i -s \
  "@penvhq/cli@$PENV_VERSION" "varlock@$VARLOCK_VERSION" \
  "@dotenvx/dotenvx@$DOTENVX_VERSION" "dotenv-cli@$DOTENV_CLI_VERSION" >/dev/null 2>&1) \
  || die "npm could not install the four packages"
BIN=$WORK/node_modules/.bin

# tool, build, version, then the command. The binary builds come from the environment.
ENTRIES=()
add() { ENTRIES+=("$1"$'\t'"$2"$'\t'"$3"$'\t'"$4"); }
binary() { # tool, path, run args...
  local tool=$1 path=$2; shift 2
  [ -x "$path" ] || die "$tool: $path is not an executable"
  case $path in /*) ;; *) path=$PWD/$path ;; esac
  add "$tool" binary "$("$path" --version 2>/dev/null | head -n 1 | sed 's/^[^0-9]*//')" "$path $*"
}
[ -n "${PENV:-}" ] && binary penv "$PENV" run -- true
add penv npm "$PENV_VERSION" "$BIN/penv run -- true"
[ -n "${VARLOCK:-}" ] && binary varlock "$VARLOCK" run -- true
add varlock npm "$VARLOCK_VERSION" "$BIN/varlock run -- true"
[ -n "${DOTENVX:-}" ] && binary dotenvx "$DOTENVX" run -- true
add dotenvx npm "$DOTENVX_VERSION" "$BIN/dotenvx run -- true"
add dotenv-cli npm "$DOTENV_CLI_VERSION" "$BIN/dotenv -- true"

project startup; common_schema
echo "machine: $(uname -sm), node $(node --version), $RUNS runs per timing"
echo
echo "| Tool         | Build    | Version              | run -- true (median ms) | Peak memory (MB) |"
echo "|--------------|----------|----------------------|-------------------------|------------------|"
RESULTS=()
for entry in "${ENTRIES[@]}"; do
  IFS=$'\t' read -r tool build version cmd <<<"$entry"
  read -r -a argv <<<"$cmd"
  # A command that fails would be timed as a start.
  "${argv[@]}" >/dev/null 2>&1 || die "$tool ($build): '$cmd' exited $?"
  ms=$(median_ms "${argv[@]}"); mb=$(peak_mb "${argv[@]}")
  printf '| %-12s | %-8s | %-20s | %-23s | %-16s |\n' "$tool" "$build" "$version" "$ms" "$mb"
  RESULTS+=("$(printf '{"tool":%s,"build":%s,"version":%s,"run_ms":%s,"peak_mb":%s}' \
    "$(json "$tool")" "$(json "$build")" "$(json "$version")" "$ms" "$mb")")
done

printf '{"runs":%s,"machine":%s,"node":%s,"results":[%s]}\n' \
  "$RUNS" "$(json "$(uname -sm)")" "$(json "$(node --version)")" "$(IFS=,; echo "${RESULTS[*]}")" > "$OUT"
echo
echo "results: $OUT"
