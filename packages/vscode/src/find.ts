// Finds the penv binary to start. Pure over what it is handed, so the tests run
// it for every platform on any one.

import * as path from "node:path";

export interface Host {
  platform: NodeJS.Platform;
  arch: string;
  /** The PATH variable. */
  pathVar: string;
  isFile(p: string): boolean;
  /** Resolves symlinks; returns the input when it cannot. */
  realpath(p: string): string;
}

/**
 * The binary to run: `configured` when set, else the first `penv` on PATH. An npm
 * install puts a Node launcher on PATH (a `.cmd` shim on Windows, which Node
 * cannot start without a shell); the platform binary behind it is used instead.
 * Relative PATH entries are skipped, so a workspace cannot put its own `penv`
 * ahead of the installed one.
 */
export function findPenv(configured: string, host: Host): string | undefined {
  if (configured.trim() !== "") {
    return configured.trim();
  }
  const p = host.platform === "win32" ? path.win32 : path.posix;
  const exe = host.platform === "win32" ? ".exe" : "";
  const inside = ["@penvhq", `cli-${host.platform}-${host.arch}`, "bin", `penv${exe}`];
  const dirs = host.pathVar.split(host.platform === "win32" ? ";" : ":");
  for (const raw of dirs) {
    const dir = raw.trim().replace(/^"(.*)"$/, "$1");
    if (dir === "" || !p.isAbsolute(dir)) {
      continue;
    }
    if (host.platform === "win32") {
      const direct = p.join(dir, "penv.exe");
      if (host.isFile(direct)) {
        return direct;
      }
      if (host.isFile(p.join(dir, "penv.cmd"))) {
        // npm's global prefix holds the shim, and node_modules beside it.
        const found = behindLauncher(p, p.join(dir, "node_modules", "@penvhq", "cli"), inside, host);
        if (found) {
          return found;
        }
      }
      continue;
    }
    const candidate = p.join(dir, "penv");
    if (!host.isFile(candidate)) {
      continue;
    }
    const real = host.realpath(candidate);
    if (real.endsWith(p.join("@penvhq", "cli", "bin", "penv.js"))) {
      const found = behindLauncher(p, p.dirname(p.dirname(real)), inside, host);
      if (found) {
        return found;
      }
    }
    return candidate;
  }
  return undefined;
}

/** The platform package, nested under the launcher's package or beside it. */
function behindLauncher(
  p: typeof path.posix,
  cliDir: string,
  inside: string[],
  host: Host,
): string | undefined {
  const places = [p.join(cliDir, "node_modules", ...inside), p.join(p.dirname(p.dirname(cliDir)), ...inside)];
  return places.find((c) => host.isFile(c));
}

/** True when `penv help --json` lists the `lsp` command. */
export function hasLsp(manifestJson: string): boolean {
  try {
    const manifest = JSON.parse(manifestJson) as { commands?: { path?: string }[] };
    return (manifest.commands ?? []).some((c) => c.path === "lsp");
  } catch {
    return false;
  }
}
