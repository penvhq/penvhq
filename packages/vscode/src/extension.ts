import { execFile } from "node:child_process";
import * as fs from "node:fs";
import * as vscode from "vscode";
import { LanguageClient, type LanguageClientOptions, type ServerOptions } from "vscode-languageclient/node";
import { findPenv, hasLsp } from "./find";

const INSTALL = "https://github.com/penvhq/penvhq#install";

let client: LanguageClient | undefined;
let output: vscode.OutputChannel | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  output = vscode.window.createOutputChannel("penv");
  context.subscriptions.push(output);
  context.subscriptions.push(
    vscode.commands.registerCommand("penv.restart", async () => {
      await stop();
      await start();
    }),
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (e.affectsConfiguration("penv.path")) {
        await stop();
        await start();
      }
    }),
  );
  await start();
}

export async function deactivate(): Promise<void> {
  await stop();
}

async function start(): Promise<void> {
  // A machine-scoped setting: workspace settings cannot name the program.
  const configured = vscode.workspace.getConfiguration("penv").get<string>("path", "");
  const binary = findPenv(configured, {
    platform: process.platform,
    arch: process.arch,
    pathVar: process.env.PATH ?? process.env.Path ?? "",
    isFile: (p) => {
      try {
        return fs.statSync(p).isFile();
      } catch {
        return false;
      }
    },
    realpath: (p) => {
      try {
        return fs.realpathSync(p);
      } catch {
        return p;
      }
    },
  });
  if (!binary) {
    offerInstall("penv is not on PATH. Install it, or set penv.path in your user settings.");
    return;
  }
  const manifest = await run(binary, ["help", "--json"]);
  if (manifest === undefined) {
    offerInstall(`${binary} did not run. Set penv.path in your user settings to the penv binary.`);
    return;
  }
  if (!hasLsp(manifest)) {
    void vscode.window.showWarningMessage(
      `${binary} has no language server. Update penv: penv upgrade, or the package manager you installed it with.`,
    );
    return;
  }
  const cwd = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  const server: ServerOptions = { command: binary, args: ["lsp"], options: cwd ? { cwd } : {} };
  const options: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", pattern: "**/.env.schema" }],
    outputChannel: output,
  };
  client = new LanguageClient("penv", "penv", server, options);
  output?.appendLine(`penv: ${binary} lsp`);
  try {
    await client.start();
  } catch (error) {
    client = undefined;
    const message = `${binary} lsp stopped: ${error instanceof Error ? error.message : String(error)}`;
    output?.appendLine(message);
    void vscode.window.showErrorMessage(message);
  }
}

async function stop(): Promise<void> {
  const running = client;
  client = undefined;
  if (running) {
    await running.stop();
  }
}

function run(binary: string, args: string[]): Promise<string | undefined> {
  return new Promise((resolve) => {
    execFile(binary, args, { timeout: 10_000, maxBuffer: 4 * 1024 * 1024, windowsHide: true }, (error, stdout) => {
      resolve(error ? undefined : stdout);
    });
  });
}

function offerInstall(message: string): void {
  output?.appendLine(message);
  void vscode.window.showWarningMessage(message, "Install penv").then((choice) => {
    if (choice) {
      void vscode.env.openExternal(vscode.Uri.parse(INSTALL));
    }
  });
}
