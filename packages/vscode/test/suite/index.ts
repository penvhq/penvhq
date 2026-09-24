// Runs inside VS Code. The penv binary comes from PATH, as a user's would.
import * as assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";

async function until<T>(what: string, probe: () => T | undefined | Promise<T | undefined>): Promise<T> {
  const deadline = Date.now() + 30_000;
  for (;;) {
    const got = await probe();
    if (got !== undefined) {
      return got;
    }
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${what}`);
    }
    await new Promise((r) => setTimeout(r, 200));
  }
}

export async function run(): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0];
  assert.ok(folder, "the fixture opened as a folder");
  const uri = vscode.Uri.file(path.join(folder.uri.fsPath, ".env.schema"));
  const doc = await vscode.workspace.openTextDocument(uri);
  await vscode.window.showTextDocument(doc);

  const found = await until("diagnostics", () => {
    const d = vscode.languages.getDiagnostics(uri);
    return d.length > 0 ? d : undefined;
  });
  assert.equal(found.length, 1, JSON.stringify(found));
  assert.equal(found[0].source, "penv");
  assert.equal(found[0].severity, vscode.DiagnosticSeverity.Error);
  assert.match(found[0].message, /^@rotate takes/);
  assert.equal(found[0].range.start.line, 7);

  const lines = doc.getText().split("\n");
  const hostsLine = lines.findIndex((l) => l.includes("@hosts"));
  const completions = await vscode.commands.executeCommand<vscode.CompletionList>(
    "vscode.executeCompletionItemProvider",
    uri,
    new vscode.Position(hostsLine, 3),
  );
  const labels = completions.items.map((i) => (typeof i.label === "string" ? i.label : i.label.label));
  assert.ok(labels.includes("@hosts") && labels.includes("@rotate"), labels.join(","));

  const keyLine = lines.findIndex((l) => l.startsWith("STRIPE_SECRET_KEY="));
  const hovers = await vscode.commands.executeCommand<vscode.Hover[]>(
    "vscode.executeHoverProvider",
    uri,
    new vscode.Position(keyLine, 3),
  );
  const text = hovers
    .flatMap((h) => h.contents)
    .map((c) => (typeof c === "string" ? c : "value" in c ? c.value : ""))
    .join("\n");
  assert.match(text, /sealed to `api\.stripe\.com`/);

  const refLine = lines.findIndex((l) => l.startsWith("API_URL="));
  const targets = await vscode.commands.executeCommand<(vscode.Location | vscode.LocationLink)[]>(
    "vscode.executeDefinitionProvider",
    uri,
    new vscode.Position(refLine, lines[refLine].indexOf("API_PORT") + 2),
  );
  const target = targets[0];
  const range = "range" in target ? target.range : target.targetRange;
  assert.equal(range.start.line, lines.findIndex((l) => l.startsWith("API_PORT=")));

  const symbols = await vscode.commands.executeCommand<vscode.DocumentSymbol[]>(
    "vscode.executeDocumentSymbolProvider",
    uri,
  );
  assert.deepEqual(
    symbols.map((s) => s.name),
    ["API_PORT", "API_URL", "STRIPE_SECRET_KEY"],
  );

  // A value file beside it gets nothing from penv. Written here: .env is gitignored.
  const values = vscode.Uri.file(path.join(folder.uri.fsPath, ".env"));
  fs.writeFileSync(values.fsPath, "STRIPE_SECRET_KEY=sk_test_FIXTURE_not_a_real_key\n");
  await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(values));
  await new Promise((r) => setTimeout(r, 1000));
  assert.equal(vscode.languages.getDiagnostics(values).filter((d) => d.source === "penv").length, 0);
}
