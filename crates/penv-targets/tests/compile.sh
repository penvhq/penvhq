#!/usr/bin/env bash
# Compile and run each language snapshot against real toolchains: a good
# environment loads and prints secrets as [redacted]; a bad one is refused with
# every problem named. A toolchain that is missing is skipped, unless
# REQUIRE_ALL=1 (CI), where it fails.
set -euo pipefail
cd "$(dirname "$0")/snapshots"
SNAP=$(pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
failed=0

GOOD=(DATABASE_URL=postgres://db.example.test/app STRIPE_SECRET_KEY=sk_live_COMPILE_0123456789 PLAN_TIER=pro MAX_RETRIES=3 DEBUG_TRACING=yes)
BAD=(PORT=0 DATABASE_URL=x STRIPE_SECRET_KEY=y PLAN_TIER=gold MAX_RETRIES=x DEBUG_TRACING=maybe)

have() { command -v "$1" >/dev/null 2>&1; }
skip() {
  if [ "${REQUIRE_ALL:-}" = 1 ]; then echo "FAIL $1: no $2"; failed=1; else echo "skip $1: no $2"; fi
}
# check <lang> <good output> <bad output>
check() {
  local lang=$1 good=$2 bad=$3 ok=1
  case "$good" in *"key=[redacted]"*"exposed=sk_live_COMPILE_0123456789"*"port=3000"*) ;; *) ok=0 ;; esac
  case "$good" in *"db=[redacted]"*) ;; *) ok=0 ;; esac
  for want in "PORT is not a port" "PLAN_TIER is not one of free, pro, enterprise" "MAX_RETRIES is not an integer" "DEBUG_TRACING is not a boolean"; do
    case "$bad" in *"$want"*) ;; *) ok=0 ;; esac
  done
  case "$bad" in *sk_live*|*"=y"*) ok=0 ;; esac
  if [ $ok = 1 ]; then echo "ok   $lang"; else echo "FAIL $lang"; echo "  good: $good"; echo "  bad:  $bad"; failed=1; fi
}

if have go; then
  d=$WORK/go && mkdir -p "$d/env" && cp "$SNAP/go.env.go" "$d/env/env.go"
  printf 'module example.test/app\n\ngo 1.21\n' >"$d/go.mod"
  cat >"$d/main.go" <<'GO'
package main

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
}
GO
  (cd "$d" && test -z "$(gofmt -l env)" && go vet ./... && go build -o app .) || { echo "FAIL go: build"; failed=1; }
  [ -x "$d/app" ] && check go "$(env "${GOOD[@]}" "$d/app")" "$(env "${BAD[@]}" "$d/app")"
else skip go go; fi

if have rustc; then
  d=$WORK/rust && mkdir -p "$d" && cp "$SNAP/rust.env.rs" "$d/env.rs"
  cat >"$d/main.rs" <<'RS'
mod env;
fn main() {
    match env::Env::load() {
        Ok(e) => println!("key={} exposed={} port={} db={:?}", e.stripe_secret_key, e.stripe_secret_key.expose(), e.port, e.database_url),
        Err(e) => println!("error: {e}"),
    }
}
RS
  rustc --edition 2021 -D warnings "$d/main.rs" -o "$d/app" || { echo "FAIL rust: build"; failed=1; }
  [ -x "$d/app" ] && check rust "$(env "${GOOD[@]}" "$d/app")" "$(env "${BAD[@]}" "$d/app")"
else skip rust rustc; fi

if have php; then
  d=$WORK/php && mkdir -p "$d" && cp "$SNAP/php.Env.php" "$d/Env.php"
  cat >"$d/main.php" <<'PHP'
<?php
require __DIR__ . '/Env.php';
try {
    $e = App\Env::load();
    echo "key=", $e->stripeSecretKey, " exposed=", $e->stripeSecretKey->expose(), " port=", $e->port, " db=", $e->databaseUrl, "\n";
} catch (RuntimeException $x) {
    echo "error: ", $x->getMessage(), "\n";
}
PHP
  php -l "$d/Env.php" >/dev/null || { echo "FAIL php: lint"; failed=1; }
  check php "$(env "${GOOD[@]}" php "$d/main.php")" "$(env "${BAD[@]}" php "$d/main.php")"
else skip php php; fi

if have javac; then
  d=$WORK/java && mkdir -p "$d/config" && cp "$SNAP/java.Env.java" "$d/config/Env.java"
  cat >"$d/Main.java" <<'JAVA'
import config.Env;

public class Main {
    public static void main(String[] args) {
        try {
            Env e = Env.load();
            System.out.println("key=" + e.stripeSecretKey() + " exposed=" + e.stripeSecretKey().expose() + " port=" + e.port() + " db=" + e.databaseUrl());
        } catch (IllegalStateException x) {
            System.out.println("error: " + x.getMessage());
        }
    }
}
JAVA
  (cd "$d" && javac -Xlint:all -Werror config/Env.java Main.java) || { echo "FAIL java: build"; failed=1; }
  [ -f "$d/Main.class" ] && check java "$(cd "$d" && env "${GOOD[@]}" java Main)" "$(cd "$d" && env "${BAD[@]}" java Main)"
else skip java javac; fi

if have dotnet; then
  d=$WORK/cs && mkdir -p "$d" && cp "$SNAP/csharp.Env.g.cs" "$d/Env.g.cs"
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
  cat >"$d/Program.cs" <<'CS'
using System;
using App;

try
{
    var e = Env.Load();
    Console.WriteLine($"key={e.StripeSecretKey} exposed={e.StripeSecretKey.Expose()} port={e.Port} db={e.DatabaseUrl}");
}
catch (InvalidOperationException x)
{
    Console.WriteLine($"error: {x.Message}");
}
CS
  (cd "$d" && dotnet build -nologo -v q -o out >/dev/null) || { echo "FAIL csharp: build"; failed=1; }
  [ -f "$d/out/app.dll" ] && check csharp "$(env "${GOOD[@]}" dotnet "$d/out/app.dll")" "$(env "${BAD[@]}" dotnet "$d/out/app.dll")"
else skip csharp dotnet; fi

exit $failed
