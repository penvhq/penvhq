import assert from "node:assert/strict";
import { test } from "node:test";
import { createRequire } from "node:module";

const { findPenv, hasLsp } = createRequire(import.meta.url)("../dist/find.js");

function host(platform, pathVar, files, links = {}) {
  return {
    platform,
    arch: "x64",
    pathVar,
    isFile: (p) => files.includes(p),
    realpath: (p) => links[p] ?? p,
  };
}

test("a configured path wins, trimmed", () => {
  assert.equal(findPenv("  /opt/penv  ", host("linux", "/usr/bin", ["/usr/bin/penv"])), "/opt/penv");
});

test("the first penv on PATH, skipping relative entries a workspace could plant", () => {
  const h = host("linux", ".:bin:/usr/local/bin:/usr/bin", ["bin/penv", "./penv", "/usr/bin/penv"]);
  assert.equal(findPenv("", h), "/usr/bin/penv");
});

test("nothing on PATH is undefined", () => {
  assert.equal(findPenv("", host("darwin", "/usr/bin:/bin", [])), undefined);
});

test("an npm global install runs the platform binary, not the Node launcher", () => {
  const launcher = "/usr/local/lib/node_modules/@penvhq/cli/bin/penv.js";
  const binary = "/usr/local/lib/node_modules/@penvhq/cli-linux-x64/bin/penv";
  const h = host("linux", "/usr/local/bin", ["/usr/local/bin/penv", binary], { "/usr/local/bin/penv": launcher });
  assert.equal(findPenv("", h), binary);
});

test("a nested platform package is found too", () => {
  const launcher = "/home/a/.npm/lib/node_modules/@penvhq/cli/bin/penv.js";
  const binary = "/home/a/.npm/lib/node_modules/@penvhq/cli/node_modules/@penvhq/cli-linux-x64/bin/penv";
  const h = host("linux", "/home/a/.npm/bin", ["/home/a/.npm/bin/penv", binary], { "/home/a/.npm/bin/penv": launcher });
  assert.equal(findPenv("", h), binary);
});

test("a launcher whose platform package is missing is still run", () => {
  const launcher = "/usr/local/lib/node_modules/@penvhq/cli/bin/penv.js";
  const h = host("linux", "/usr/local/bin", ["/usr/local/bin/penv"], { "/usr/local/bin/penv": launcher });
  assert.equal(findPenv("", h), "/usr/local/bin/penv");
});

test("Windows: penv.exe on PATH, quoted entries unquoted", () => {
  const h = host("win32", '"C:\\Program Files\\penv";C:\\Windows', ["C:\\Program Files\\penv\\penv.exe"]);
  assert.equal(findPenv("", h), "C:\\Program Files\\penv\\penv.exe");
});

test("Windows: behind npm's penv.cmd shim, never the shim itself", () => {
  const prefix = "C:\\Users\\a\\AppData\\Roaming\\npm";
  const binary = `${prefix}\\node_modules\\@penvhq\\cli-win32-x64\\bin\\penv.exe`;
  assert.equal(findPenv("", host("win32", prefix, [`${prefix}\\penv.cmd`, binary])), binary);
  assert.equal(findPenv("", host("win32", prefix, [`${prefix}\\penv.cmd`])), undefined);
});

test("Windows: relative PATH entries are skipped", () => {
  assert.equal(findPenv("", host("win32", "tools;.", ["tools\\penv.exe", ".\\penv.exe"])), undefined);
});

test("hasLsp reads the manifest penv help --json prints", () => {
  assert.equal(hasLsp(JSON.stringify({ commands: [{ path: "run" }, { path: "lsp" }] })), true);
  assert.equal(hasLsp(JSON.stringify({ commands: [{ path: "run" }] })), false);
  assert.equal(hasLsp("not json"), false);
});
