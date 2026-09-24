// Starts a real VS Code with this extension and runs test/suite in it.
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { runTests } from "@vscode/test-electron";

async function main(): Promise<void> {
  const root = path.resolve(__dirname, "..", "..");
  // A fresh folder: the repository ignores .env.* files, so the schema is kept
  // as fixture.env.schema and copied in under its real name.
  const fixture = fs.mkdtempSync(path.join(os.tmpdir(), "penv-vscode-"));
  fs.copyFileSync(path.join(root, "test", "fixture.env.schema"), path.join(fixture, ".env.schema"));
  await runTests({
    extensionDevelopmentPath: root,
    extensionTestsPath: path.join(root, "dist", "test", "suite", "index.js"),
    version: process.env.VSCODE_VERSION ?? "stable",
    launchArgs: [fixture, "--disable-extensions", "--disable-workspace-trust"],
  });
}

main().catch((error: unknown) => {
  console.error(error);
  process.exit(1);
});
