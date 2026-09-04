// jsdata VS Code extension (phase 2). Depends only on the config grammar:
//   var NAME = <path> -> const DECL
// Keybind appends a var line for the declaration under the cursor; tracked
// declarations get a decoration, refreshed on config save.

import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

// ponytail: config fixed at <workspace root>/jsdata.cfg, add a setting when
// someone actually keeps it elsewhere
const CONFIG_NAME = "jsdata.cfg";

interface VarEntry {
  name: string;
  source: string; // config-relative path, normalized to forward slashes
  decl: string;
}

function workspaceRoot(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

function configPath(): string | undefined {
  const root = workspaceRoot();
  return root ? path.join(root, CONFIG_NAME) : undefined;
}

/// Mirror of the Rust config grammar, var lines only (all the decorations need).
function parseVars(text: string): VarEntry[] {
  const entries: VarEntry[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line.startsWith("var ")) continue;
    const eq = line.indexOf("=");
    if (eq < 0) continue;
    const name = line.slice(4, eq).trim();
    const rhs = line.slice(eq + 1).trim();
    const arrow = rhs.lastIndexOf("->"); // last occurrence, like the CLI
    if (arrow < 0) continue;
    const source = rhs.slice(0, arrow).trim().replace(/\\/g, "/");
    const declPart = rhs.slice(arrow + 2).trim();
    const m = /^(?:export\s+)?const\s+([A-Za-z_$][A-Za-z0-9_$]*)$/.exec(declPart);
    if (!m || !name) continue;
    entries.push({ name, source, decl: m[1] });
  }
  return entries;
}

function takenNames(text: string): Set<string> {
  const names = new Set<string>();
  for (const raw of text.split(/\r?\n/)) {
    const m = /^(?:var|let)\s+([^=]+)=/.exec(raw.trim());
    if (m) names.add(m[1].trim());
  }
  return names;
}

async function track(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  const cfgPath = configPath();
  if (!editor || !cfgPath) {
    vscode.window.showWarningMessage("jsdata: no active editor / workspace");
    return;
  }
  const wordRange = editor.document.getWordRangeAtPosition(editor.selection.active);
  const word = wordRange ? editor.document.getText(wordRange) : "";
  const name = await vscode.window.showInputBox({
    prompt: "Export name (declaration under cursor is tracked as const of the same name)",
    value: word,
    validateInput: (v) =>
      /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(v) ? undefined : "not a valid JS identifier",
  });
  if (!name) return;

  const cfgText = fs.existsSync(cfgPath) ? fs.readFileSync(cfgPath, "utf8") : "";
  if (takenNames(cfgText).has(name)) {
    vscode.window.showErrorMessage(`jsdata: \`${name}\` is already tracked`);
    return;
  }
  const rel = vscode.workspace.asRelativePath(editor.document.uri, false);
  const nl = cfgText.length === 0 || cfgText.endsWith("\n") ? "" : "\n";
  fs.appendFileSync(cfgPath, `${nl}var ${name} = ${rel} -> const ${name}\n`);
  vscode.window.showInformationMessage(`jsdata: tracking \`${name}\` from ${rel}`);
}

const decoration = vscode.window.createTextEditorDecorationType({
  isWholeLine: true,
  after: {
    contentText: "  ⇠ jsdata",
    color: new vscode.ThemeColor("editorCodeLens.foreground"),
  },
});

function refreshDecorations(): void {
  const cfgPath = configPath();
  const root = workspaceRoot();
  if (!cfgPath || !root || !fs.existsSync(cfgPath)) return;
  const entries = parseVars(fs.readFileSync(cfgPath, "utf8"));

  for (const editor of vscode.window.visibleTextEditors) {
    const rel = vscode.workspace.asRelativePath(editor.document.uri, false).replace(/\\/g, "/");
    const mine = entries.filter((e) => e.source === rel);
    const ranges: vscode.Range[] = [];
    if (mine.length > 0) {
      for (let i = 0; i < editor.document.lineCount; i++) {
        const text = editor.document.lineAt(i).text;
        for (const e of mine) {
          // ponytail: column-0 regex stands in for the CLI's depth-0 lexer;
          // good enough to mark the line a human is looking at
          if (new RegExp(`^(?:export\\s+)?const\\s+${e.decl}\\b`).test(text)) {
            ranges.push(editor.document.lineAt(i).range);
          }
        }
      }
    }
    editor.setDecorations(decoration, ranges);
  }
}

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("jsdata.track", track),
    // config save refreshes per spec; source saves too (decls move around)
    vscode.workspace.onDidSaveTextDocument(refreshDecorations),
    vscode.window.onDidChangeVisibleTextEditors(refreshDecorations),
    decoration
  );
  refreshDecorations();
}

export function deactivate(): void {}
