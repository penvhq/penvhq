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

// The terminal sends these to the whole process group, so penv already has them;
// the launcher waits for its answer rather than dying first.
const ignore = () => {};
// Sent to the launcher alone (kill, timeout, a supervisor), so they are passed on.
const forward = (signal) => () => child.kill(signal);
const handlers = [
  ["SIGINT", ignore],
  ["SIGQUIT", ignore],
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
