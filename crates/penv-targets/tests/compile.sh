#!/usr/bin/env bash
# Compile and run each language snapshot against real toolchains: a good
# environment loads and prints secrets as [redacted]; a bad one is refused with
# every problem named, and never a value. The strings-only, empty and reserved
# fixtures are compiled and loaded too. A toolchain that is missing is skipped,
# unless REQUIRE_ALL=1 (CI), where it fails.
set -euo pipefail
cd "$(dirname "$0")/snapshots"
SNAP=$(pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
failed=0

GOOD=(DATABASE_URL=postgres://db.example.test/app STRIPE_SECRET_KEY=sk_live_COMPILE_0123456789 PLAN_TIER=pro MAX_RETRIES=3 DEBUG_TRACING=yes NEXT_PUBLIC_CHECKOUT_TOKEN=tok_COMPILE_0123)
BAD=(PORT=0 DATABASE_URL=x STRIPE_SECRET_KEY=y PLAN_TIER=gold MAX_RETRIES=x DEBUG_TRACING=maybe)
# Each extra fixture: the environment it loads with, and one it refuses.
declare -A LOADS=([strings]="API_TOKEN=tok_COMPILE_0123" [empty]="" [reserved]="RAW=raw_COMPILE BAD=bad_COMPILE THIS=this_COMPILE N=1 E=2")
declare -A REFUSES=([strings]="" [empty]="" [reserved]="RAW=raw_COMPILE BAD=bad_COMPILE PORT=0 N=nope")
EXTRA=(strings empty reserved)

have() { command -v "$1" >/dev/null 2>&1; }
skip() {
  if [ "${REQUIRE_ALL:-}" = 1 ]; then echo "FAIL $1: no $2"; failed=1; else echo "skip $1: no $2"; fi
}
fail() { echo "FAIL $1"; shift; for line in "$@"; do echo "  $line"; done; failed=1; }
# leaked <output>: true when a value from any environment above reached it.
leaked() { case "$1" in *sk_live*|*"=y"*|*_COMPILE*|*nope*) return 0 ;; *) return 1 ;; esac; }
# check <lang> <good output> <bad output> <what the bad output names...>
check() {
  local lang=$1 good=$2 bad=$3 ok=1
  shift 3
  case "$good" in *"exposed=sk_live_COMPILE_0123456789"*"port=3000"*) ;; *) ok=0 ;; esac
  for want in "$@"; do
    case "$bad" in *"$want"*) ;; *) ok=0 ;; esac
  done
  leaked "$bad" && ok=0
  if [ $ok = 1 ]; then echo "ok   $lang"; else fail "$lang" "good: $good" "bad:  $bad"; fi
}
# Every loader but ts names a problem the same way.
PROBLEMS=("PORT is not a port" "PLAN_TIER is not one of free, pro, enterprise" "MAX_RETRIES is not an integer" "DEBUG_TRACING is not a boolean")
REDACTED=("key=[redacted]" "db=[redacted]")
# redacted <lang> <good output>: the secrets printed as their type prints them.
redacted() {
  local want
  for want in "${REDACTED[@]}"; do
    case "$2" in *"$want"*) ;; *) fail "$1 redaction" "good: $2"; return ;; esac
  done
}
# extra <lang> <fixture> <run command...>: loads with its environment, refuses the other.
extra() {
  local lang=$1 fixture=$2 out
  shift 2
  # shellcheck disable=SC2086
  out=$(env ${LOADS[$fixture]} "$@" 2>&1 || true)
  case "$out" in *loaded*) echo "ok   $lang $fixture" ;; *) fail "$lang $fixture" "load: $out" ;; esac
  [ -n "${REFUSES[$fixture]}" ] || return 0
  # shellcheck disable=SC2086
  out=$(env ${REFUSES[$fixture]} "$@" 2>&1 || true)
  case "$out" in *error:*PORT*) ;; *) fail "$lang $fixture refused" "bad: $out"; return ;; esac
  if leaked "$out"; then fail "$lang $fixture refused" "bad: $out"; fi
}

if have npm && have node; then
  # The strictest settings a project is likely to use: an unused declaration or
  # a missing `override` is an error, as a linter would report it.
  deps=$WORK/ts-deps && mkdir -p "$deps"
  if (cd "$deps" && npm i -s --no-audit --no-fund typescript@5 @types/node@22 >/dev/null 2>&1); then
    ts() {
      local d=$1 snapshot=$2 main=$3
      mkdir -p "$d" && cp "$snapshot" "$d/env.ts" && printf '%s\n' "$main" >"$d/main.ts"
      ln -s "$deps/node_modules" "$d/node_modules"
      cat >"$d/shims.d.ts" <<'TS'
interface ImportMetaEnv { [key: string]: string | undefined }
interface ImportMeta { readonly env: ImportMetaEnv }
TS
      cat >"$d/tsconfig.json" <<'JSON'
{ "compilerOptions": { "target": "ES2022", "module": "ESNext", "moduleResolution": "bundler", "strict": true,
  "noEmit": true, "skipLibCheck": true, "noUnusedLocals": true, "noUnusedParameters": true,
  "noImplicitOverride": true, "types": ["node"], "lib": ["ES2022"] }, "files": ["env.ts", "main.ts", "shims.d.ts"] }
JSON
      (cd "$d" && ./node_modules/.bin/tsc -p . &&
        ./node_modules/.bin/tsc -p . --noEmit false --outDir out --module commonjs --moduleResolution node10)
    }
    main='import { env, envSchema } from "./env";
const checked = envSchema["~standard"].validate(process.env);
if ("issues" in checked && checked.issues) {
  console.log("error: " + checked.issues.map((i) => i.message).join("; "));
} else {
  process.stdout.write(`exposed=${env.STRIPE_SECRET_KEY} port=${env.PORT} tracing=${env.DEBUG_TRACING}\n`);
  console.log(`key=${env.STRIPE_SECRET_KEY} token=${env.NEXT_PUBLIC_CHECKOUT_TOKEN}`);
}'
    if ts "$WORK/ts" "$SNAP/ts.env.ts" "$main"; then
      good=$(env "${GOOD[@]}" node "$WORK/ts/out/main.js")
      check ts "$good" "$(env "${BAD[@]}" node "$WORK/ts/out/main.js")" \
        "PORT must be a port" "PLAN_TIER must be one of free, pro, enterprise" "MAX_RETRIES must be a whole number" "DEBUG_TRACING must be a boolean"
      # The console masks every secret, the public one built from a secret too.
      case "$good" in *"tracing=true"*"key=sk"*"token=to"*) ;; *) fail "ts booleans and masking" "good: $good" ;; esac
      case "$(printf '%s\n' "$good" | grep '^key=')" in *_COMPILE_*) fail "ts masking" "good: $good" ;; esac
    else
      fail "ts: build"
    fi
    for fixture in "${EXTRA[@]}"; do
      main='import { env, envSchema } from "./env";
const checked = envSchema["~standard"].validate(process.env);
if ("issues" in checked && checked.issues) console.log("error: " + checked.issues.map((i) => i.message).join("; "));
else console.log(`loaded ${Object.keys(env).length}`);'
      if ts "$WORK/ts-$fixture" "$SNAP/$fixture/ts.env.ts" "$main"; then
        extra ts "$fixture" node "$WORK/ts-$fixture/out/main.js"
      else
        fail "ts $fixture: build"
      fi
    done
  else
    fail "ts: npm install"
  fi
else skip ts npm; fi

if have python3; then
  py() {
    local d=$1 snapshot=$2 main=$3
    mkdir -p "$d" && cp "$snapshot" "$d/penv_env.py" && printf '%s\n' "$main" >"$d/main.py"
    python3 -m py_compile "$d/penv_env.py"
  }
  main='try:
    from penv_env import env
except RuntimeError as problems:
    print(f"error: {problems}")
else:
    print(f"exposed={env.STRIPE_SECRET_KEY} port={env.PORT} tracing={env.DEBUG_TRACING}")'
  if py "$WORK/py" "$SNAP/py.penv_env.py" "$main"; then
    good=$(env "${GOOD[@]}" python3 "$WORK/py/main.py")
    check py "$good" "$(env "${BAD[@]}" python3 "$WORK/py/main.py")" "${PROBLEMS[@]}"
    case "$good" in *"tracing=True"*) ;; *) fail "py booleans" "good: $good" ;; esac
  else
    fail "py: compile"
  fi
  for fixture in "${EXTRA[@]}"; do
    main='try:
    from penv_env import env
except RuntimeError as problems:
    print(f"error: {problems}")
else:
    print("loaded")'
    if py "$WORK/py-$fixture" "$SNAP/$fixture/py.penv_env.py" "$main"; then
      extra py "$fixture" python3 "$WORK/py-$fixture/main.py"
    else
      fail "py $fixture: compile"
    fi
  done
else skip py python3; fi

if have go; then
  gobuild() {
    local d=$1 snapshot=$2 main=$3
    mkdir -p "$d/env" && cp "$snapshot" "$d/env/env.go" && printf '%s\n' "$main" >"$d/main.go"
    printf 'module example.test/app\n\ngo 1.21\n' >"$d/go.mod"
    (cd "$d" && test -z "$(gofmt -l env)" && go vet ./... && go build -o app .)
  }
  main='package main

import (
	"fmt"

	"example.test/app/env"
)

func main() {
	e, err := env.Load()
	if err != nil {
		fmt.Println("error:", err)
		return
	}
	fmt.Printf("key=%v exposed=%s port=%d db=%+v\n", e.StripeSecretKey, e.StripeSecretKey.Value(), e.Port, e.DatabaseUrl)
}'
  if gobuild "$WORK/go" "$SNAP/go.env.go" "$main"; then
    good=$(env "${GOOD[@]}" "$WORK/go/app")
    check go "$good" "$(env "${BAD[@]}" "$WORK/go/app")" "${PROBLEMS[@]}"
    redacted go "$good"
  else
    fail "go: build"
  fi
  for fixture in "${EXTRA[@]}"; do
    main='package main

import (
	"fmt"

	"example.test/app/env"
)

func main() {
	if _, err := env.Load(); err != nil {
		fmt.Println("error:", err)
		return
	}
	fmt.Println("loaded")
}'
    if gobuild "$WORK/go-$fixture" "$SNAP/$fixture/go.env.go" "$main"; then
      extra go "$fixture" "$WORK/go-$fixture/app"
    else
      fail "go $fixture: build"
    fi
  done
else skip go go; fi

if have rustc; then
  rsbuild() {
    local d=$1 snapshot=$2 main=$3
    mkdir -p "$d" && cp "$snapshot" "$d/env.rs" && printf '%s\n' "$main" >"$d/main.rs"
    rustc --edition 2021 -D warnings "$d/main.rs" -o "$d/app"
  }
  main='mod env;
fn main() {
    match env::Env::load() {
        Ok(e) => println!("key={} exposed={} port={} db={:?}", e.stripe_secret_key, e.stripe_secret_key.expose(), e.port, e.database_url),
        Err(e) => println!("error: {e}"),
    }
}'
  if rsbuild "$WORK/rust" "$SNAP/rust.env.rs" "$main"; then
    good=$(env "${GOOD[@]}" "$WORK/rust/app")
    check rust "$good" "$(env "${BAD[@]}" "$WORK/rust/app")" "${PROBLEMS[@]}"
    redacted rust "$good"
  else
    fail "rust: build"
  fi
  for fixture in "${EXTRA[@]}"; do
    main='mod env;
fn main() {
    match env::Env::load() {
        Ok(_) => println!("loaded"),
        Err(e) => println!("error: {e}"),
    }
}'
    if rsbuild "$WORK/rust-$fixture" "$SNAP/$fixture/rust.env.rs" "$main"; then
      extra rust "$fixture" "$WORK/rust-$fixture/app"
    else
      fail "rust $fixture: build"
    fi
  done
else skip rust rustc; fi

if have php; then
  phpbuild() {
    local d=$1 snapshot=$2 main=$3
    mkdir -p "$d" && cp "$snapshot" "$d/Env.php" && printf '%s\n' "$main" >"$d/main.php"
    php -l "$d/Env.php" >/dev/null
  }
  main='<?php
require __DIR__ . "/Env.php";
try {
    $e = App\Env::load();
    echo "key=", $e->stripeSecretKey, " exposed=", $e->stripeSecretKey->expose(), " port=", $e->port, " db=", $e->databaseUrl, "\n";
} catch (RuntimeException $x) {
    echo "error: ", $x->getMessage(), "\n";
}'
  if phpbuild "$WORK/php" "$SNAP/php.Env.php" "$main"; then
    good=$(env "${GOOD[@]}" php "$WORK/php/main.php")
    check php "$good" "$(env "${BAD[@]}" php "$WORK/php/main.php")" "${PROBLEMS[@]}"
    redacted php "$good"
  else
    fail "php: lint"
  fi
  for fixture in "${EXTRA[@]}"; do
    main='<?php
require __DIR__ . "/Env.php";
try {
    App\Env::load();
    echo "loaded\n";
} catch (RuntimeException $x) {
    echo "error: ", $x->getMessage(), "\n";
}'
    if phpbuild "$WORK/php-$fixture" "$SNAP/$fixture/php.Env.php" "$main"; then
      extra php "$fixture" php "$WORK/php-$fixture/main.php"
    else
      fail "php $fixture: lint"
    fi
  done
else skip php php; fi

if have javac; then
  javabuild() {
    local d=$1 snapshot=$2 main=$3
    mkdir -p "$d/config" && cp "$snapshot" "$d/config/Env.java" && printf '%s\n' "$main" >"$d/Main.java"
    (cd "$d" && javac -Xlint:all -Werror config/Env.java Main.java)
  }
  main='import config.Env;

public class Main {
    public static void main(String[] args) {
        try {
            Env e = Env.load();
            System.out.println("key=" + e.stripeSecretKey() + " exposed=" + e.stripeSecretKey().expose() + " port=" + e.port() + " db=" + e.databaseUrl());
        } catch (IllegalStateException x) {
            System.out.println("error: " + x.getMessage());
        }
    }
}'
  if javabuild "$WORK/java" "$SNAP/java.Env.java" "$main"; then
    good=$(cd "$WORK/java" && env "${GOOD[@]}" java Main)
    check java "$good" "$(cd "$WORK/java" && env "${BAD[@]}" java Main)" "${PROBLEMS[@]}"
    redacted java "$good"
  else
    fail "java: build"
  fi
  for fixture in "${EXTRA[@]}"; do
    main='import config.Env;

public class Main {
    public static void main(String[] args) {
        try {
            Env.load();
            System.out.println("loaded");
        } catch (IllegalStateException x) {
            System.out.println("error: " + x.getMessage());
        }
    }
}'
    if javabuild "$WORK/java-$fixture" "$SNAP/$fixture/java.Env.java" "$main"; then
      extra java "$fixture" java -cp "$WORK/java-$fixture" Main
    else
      fail "java $fixture: build"
    fi
  done
else skip java javac; fi

if have dotnet; then
  csbuild() {
    local d=$1 snapshot=$2 main=$3
    mkdir -p "$d" && cp "$snapshot" "$d/Env.g.cs" && printf '%s\n' "$main" >"$d/Program.cs"
    cat >"$d/app.csproj" <<'XML'
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net8.0</TargetFramework>
    <Nullable>enable</Nullable>
    <TreatWarningsAsErrors>true</TreatWarningsAsErrors>
    <ImplicitUsings>disable</ImplicitUsings>
  </PropertyGroup>
</Project>
XML
    (cd "$d" && dotnet build -nologo -v q -o out >/dev/null)
  }
  main='using System;
using App;

try
{
    var e = Env.Load();
    Console.WriteLine($"key={e.StripeSecretKey} exposed={e.StripeSecretKey.Expose()} port={e.Port} db={e.DatabaseUrl}");
}
catch (InvalidOperationException x)
{
    Console.WriteLine($"error: {x.Message}");
}'
  if csbuild "$WORK/cs" "$SNAP/csharp.Env.g.cs" "$main"; then
    good=$(env "${GOOD[@]}" dotnet "$WORK/cs/out/app.dll")
    check csharp "$good" "$(env "${BAD[@]}" dotnet "$WORK/cs/out/app.dll")" "${PROBLEMS[@]}"
    redacted csharp "$good"
  else
    fail "csharp: build"
  fi
  for fixture in "${EXTRA[@]}"; do
    main='using System;
using App;

try
{
    Env.Load();
    Console.WriteLine("loaded");
}
catch (InvalidOperationException x)
{
    Console.WriteLine($"error: {x.Message}");
}'
    if csbuild "$WORK/cs-$fixture" "$SNAP/$fixture/csharp.Env.g.cs" "$main"; then
      extra csharp "$fixture" dotnet "$WORK/cs-$fixture/out/app.dll"
    else
      fail "csharp $fixture: build"
    fi
  done
else skip csharp dotnet; fi

exit $failed
