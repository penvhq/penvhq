// node --test npm/launcher.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { copyFileSync, linkSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const exe = process.platform === "win32" ? ".exe" : "";
const platformPackage = `@penvhq/cli-${process.platform}-${process.arch}`;
const temporary = [];

test.after(() => temporary.forEach((dir) => rmSync(dir, { recursive: true, force: true })));

/// The launcher where npm would put it, with the platform package beside it or absent.
function installed({ withBinary }) {
  const root = mkdtempSync(join(tmpdir(), "penv-npm-"));
  temporary.push(root);
  const modules = join(root, "node_modules", "@penvhq");

  const launcher = join(modules, "cli");
  mkdirSync(join(launcher, "bin"), { recursive: true });
  copyFileSync(join(here, "cli", "package.json"), join(launcher, "package.json"));
  copyFileSync(join(here, "cli", "bin", "penv.js"), join(launcher, "bin", "penv.js"));

  if (withBinary) {
    const platform = join(modules, `cli-${process.platform}-${process.arch}`);
    mkdirSync(join(platform, "bin"), { recursive: true });
    // The exports field pack.mjs writes, so require.resolve is tested against a package that has one.
    writeFileSync(
      join(platform, "package.json"),
      JSON.stringify({
        name: platformPackage,
        version: "0.0.0",
        os: [process.platform],
        cpu: [process.arch],
        exports: { [`./bin/penv${exe}`]: `./bin/penv${exe}`, "./package.json": "./package.json" },
      }),
    );
    // node itself stands in for the binary, so the child is a real executable everywhere.
    const binary = join(platform, "bin", `penv${exe}`);
    try {
      linkSync(process.execPath, binary);
    } catch {
      copyFileSync(process.execPath, binary);
    }
  }
  return join(launcher, "bin", "penv.js");
}

const run = (entry, args) => spawnSync(process.execPath, [entry, ...args], { encoding: "utf8" });

test("the platform package's binary runs with the arguments the launcher was given", () => {
  const answered = run(installed({ withBinary: true }), [
    "-e",
    "process.stdout.write('penv ran ' + process.argv.slice(1).join(' '))",
    "check",
    "--json",
  ]);
  assert.equal(answered.status, 0);
  assert.equal(answered.stdout, "penv ran check --json");
});

test("the exit code the binary answers with is the launcher's", () => {
  const entry = installed({ withBinary: true });
  assert.equal(run(entry, ["-e", "process.exit(3)"]).status, 3);
  assert.equal(run(entry, ["-e", "process.exit(0)"]).status, 0);
});

test("a SIGTERM sent to the launcher reaches penv, and penv's exit code is the answer", { skip: process.platform === "win32" }, async () => {
  const entry = installed({ withBinary: true });
  const script = "process.on('SIGTERM', () => process.exit(7)); process.stdout.write('ready'); setInterval(() => {}, 1000)";
  const launcher = spawn(process.execPath, [entry, "-e", script], { stdio: ["ignore", "pipe", "inherit"] });
  await new Promise((ready) => launcher.stdout.once("data", ready));
  launcher.kill("SIGTERM");
  const [code, signal] = await new Promise((done) => launcher.on("exit", (c, s) => done([c, s])));
  assert.equal(signal, null);
  assert.equal(code, 7);
});

test("a SIGINT sent to the launcher alone reaches penv too", { skip: process.platform === "win32" }, async () => {
  const entry = installed({ withBinary: true });
  const script = "process.on('SIGINT', () => process.exit(8)); process.stdout.write('ready'); setInterval(() => {}, 1000)";
  const launcher = spawn(process.execPath, [entry, "-e", script], { stdio: ["ignore", "pipe", "inherit"] });
  await new Promise((ready) => launcher.stdout.once("data", ready));
  launcher.kill("SIGINT");
  const timer = setTimeout(() => launcher.kill("SIGKILL"), 5000);
  const [code] = await new Promise((done) => launcher.on("exit", (c, s) => done([c, s])));
  clearTimeout(timer);
  assert.equal(code, 8);
});

test("a missing platform package names it and the installer that needs no npm", () => {
  const refused = run(installed({ withBinary: false }), ["--version"]);
  assert.equal(refused.status, 1);
  assert.match(refused.stderr, new RegExp(platformPackage.replace("/", "\\/")));
  assert.match(refused.stderr, /penv\.cloud\/install/);
  assert.match(refused.stderr, /install\.ps1/);
});

test("pack writes six platform packages and stamps the release version through all of them", () => {
  const work = mkdtempSync(join(tmpdir(), "penv-pack-"));
  temporary.push(work);
  const binaries = join(work, "dist");
  const triples = [
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
  ];
  mkdirSync(binaries, { recursive: true });
  for (const triple of triples) {
    writeFileSync(join(binaries, `penv-v1.2.3-${triple}${triple.includes("windows") ? ".exe" : ""}`), `${triple} stands in for a binary`);
  }

  const out = join(work, "build");
  const packed = spawnSync(
    process.execPath,
    [join(here, "pack.mjs"), "--version", "v1.2.3", "--binaries", binaries, "--out", out],
    { encoding: "utf8" },
  );
  assert.equal(packed.status, 0, packed.stderr);

  const read = (file) => JSON.parse(readFileSync(file, "utf8"));
  const launcher = read(join(out, "cli", "package.json"));
  assert.equal(launcher.version, "1.2.3");
  assert.deepEqual(Object.values(launcher.optionalDependencies), Array(6).fill("1.2.3"));
  assert.deepEqual(
    Object.keys(launcher.optionalDependencies),
    Object.keys(read(join(here, "cli", "package.json")).optionalDependencies),
  );

  assert.match(launcher.repository.url, /^git\+https:\/\/\S+\.git$/);

  const windows = read(join(out, "cli-win32-arm64", "package.json"));
  assert.deepEqual(windows.os, ["win32"]);
  assert.deepEqual(windows.cpu, ["arm64"]);
  assert.equal(windows.version, "1.2.3");
  assert.equal(windows.bin, undefined);
  assert.deepEqual(windows.exports, {
    "./bin/penv.exe": "./bin/penv.exe",
    "./package.json": "./package.json",
  });
  assert.deepEqual(read(join(out, "cli-linux-arm64", "package.json")).exports, {
    "./bin/penv": "./bin/penv",
    "./package.json": "./package.json",
  });
  assert.equal(
    readFileSync(join(out, "cli-win32-arm64", "bin", "penv.exe"), "utf8"),
    "aarch64-pc-windows-msvc stands in for a binary",
  );
  assert.equal(readFileSync(join(out, "cli-linux-arm64", "bin", "penv"), "utf8"), "aarch64-unknown-linux-musl stands in for a binary");
});
