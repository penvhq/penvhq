#!/usr/bin/env node
"use strict";

// npm installs one of the six @penvhq/cli-<platform> packages and skips the rest,
// so this shim runs whichever one landed. There is no postinstall step.
const { spawn } = require("node:child_process");

const PUBLISHED = ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64", "win32-arm64", "win32-x64"];
const platform = `${process.platform}-${process.arch}`;
const pkg = `@penvhq/cli-${platform}`;
const inside = `${pkg}/bin/penv${process.platform === "win32" ? ".exe" : ""}`;

let binary;
try {
  binary = require.resolve(inside);
} catch {
  process.stderr.write(
    PUBLISHED.includes(platform)
      ? `penv: ${pkg} is missing, and npm skips an optional dependency it could not install without saying so. ` +
          `Reinstall with npm i -g @penvhq/cli, or install without npm: curl -fsSL https://penv.cloud/install | sh, ` +
          `or on Windows irm https://penv.cloud/install.ps1 | iex\n`
      : `penv: there is no penv build for ${platform}. The releases cover ${PUBLISHED.join(", ")}.\n`,
  );
  process.exit(1);
}

const child = spawn(binary, process.argv.slice(2), { stdio: "inherit" });

// Passed on, so a signal sent to the launcher alone (kill, timeout, a CI cancel)
// reaches penv; one the terminal also sent penv is not passed to its child twice.
// Windows' kill ends a process outright, and its console sends Ctrl-C to both,
// so there the launcher only waits.
const windows = process.platform === "win32";
const ignore = () => {};
const forward = (signal) => () => {
  try {
    child.kill(signal);
  } catch {
    // A signal this platform cannot send.
  }
};
const handlers = [
  ["SIGINT", windows ? ignore : forward("SIGINT")],
  ["SIGQUIT", windows ? ignore : forward("SIGQUIT")],
  ["SIGTERM", forward("SIGTERM")],
  ["SIGHUP", forward("SIGHUP")],
];
for (const [signal, handler] of handlers) {
  try {
    process.on(signal, handler);
  } catch {
    // Windows has no SIGQUIT.
  }
}

child.on("error", (error) => {
  process.stderr.write(`penv: ${binary} could not be run: ${error.message}\n`);
  process.exit(1);
});

child.on("exit", (code, signal) => {
  // Re-raised rather than translated, so a caller sees the signal that stopped penv.
  if (signal) {
    for (const [name, handler] of handlers) process.removeListener(name, handler);
    process.kill(process.pid, signal);
    // Reached only when the signal was ignored, so there is still a code to answer with.
    process.exit(1);
  }
  process.exit(code === null ? 1 : code);
});
