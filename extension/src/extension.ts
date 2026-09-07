// jsdata VS Code extension (phase 2). Depends only on the config grammar:
//   var NAME = <path> -> const DECL
// Keybind appends a var line for the declaration under the cursor; tracked
// declarations get a decoration, refreshed on config save. The config itself
// gets highlighting (syntaxes/jsdata.tmLanguage.json) and clickable paths.

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

interface PathSpan {
  start: number;
  end: number;
}

/// One path operand of a directive. `prefix` is the project path a `vars` file
/// is resolved against, matching the CLI's `dir.join(project).join(file)`.
interface PathRef {
  span: PathSpan;
  prefix?: string;
}

/// Span of `line[from..to]` with the surrounding whitespace trimmed off, or
/// undefined when nothing is left.
function trimmedSpan(line: string, from: number, to: number): PathSpan | undefined {
  let start = from;
  let end = to;
  while (start < end && /\s/.test(line[start])) start++;
  while (end > start && /\s/.test(line[end - 1])) end--;
  return end > start ? { start, end } : undefined;
}

/// The path operands of one config line. Mirrors the Rust parser's splitting
/// order: the ` ---` pin comes off the end first, then the arrows.
function pathRefs(line: string): PathRef[] {
  const head = /^\s*(\S+)/.exec(line);
  if (!head || head[1].startsWith("#")) return [];
  const word = head[1];
  const from = head[0].length;
  let to = line.length;

  if (word === "var" || word === "vars" || word === "langs") {
    const pin = line.lastIndexOf(" ---");
    if (pin >= from && line.slice(pin + 4).trim() !== "") to = pin;
  }

  const keep = (s: PathSpan | undefined): PathRef[] => (s ? [{ span: s }] : []);
  const after = (i: number) => i + 3; // past a `<--` / `-->`

  switch (word) {
    case "out":
      return keep(trimmedSpan(line, from, to));
    case "var": {
      const eq = line.indexOf("=", from);
      if (eq < 0) return [];
      const rhs = eq + 1;
      const arrow = line.slice(rhs, to).lastIndexOf("->");
      return arrow < 0 ? [] : keep(trimmedSpan(line, rhs, rhs + arrow));
    }
    case "md": {
      const first = line.indexOf("<--", from);
      if (first < 0) return [];
      const second = line.indexOf("<--", after(first));
      return keep(trimmedSpan(line, after(first), second < 0 ? to : second));
    }
    case "langs": {
      const first = line.indexOf("<--", from);
      if (first < 0) return [];
      const second = line.indexOf("<--", after(first));
      if (second < 0) return [];
      return [
        ...keep(trimmedSpan(line, after(first), second)),
        ...keep(trimmedSpan(line, after(second), to)),
      ];
    }
    case "vars": {
      const first = line.indexOf("<--", from);
      if (first < 0) return [];
      // `--> FILE` is tried before `<-- CONFIG`, like the CLI
      const built = line.indexOf("-->", after(first));
      const sep = built >= 0 ? built : line.indexOf("<--", after(first));
      if (sep < 0) return [];
      const project = trimmedSpan(line, after(first), sep);
      const target = trimmedSpan(line, after(sep), to);
      if (!target) return [];
      return [{ span: target, prefix: project && line.slice(project.start, project.end) }];
    }
    default:
      return [];
  }
}

/// Ctrl+click a path to open it. Only paths that resolve to a file that exists
/// become links; an unexpanded `~` / `%VAR%` / `$VAR` is left alone rather than
/// guessed at, since only the CLI knows the environment it will run in.
function documentLinks(document: vscode.TextDocument): vscode.DocumentLink[] {
  const dir = path.dirname(document.uri.fsPath);
  const links: vscode.DocumentLink[] = [];
  for (let i = 0; i < document.lineCount; i++) {
    const line = document.lineAt(i).text;
    for (const ref of pathRefs(line)) {
      const text = line.slice(ref.span.start, ref.span.end);
      if (/[~%$]/.test(text) || /[~%$]/.test(ref.prefix ?? "")) continue;
      const target = path.resolve(dir, ref.prefix ?? "", text);
      try {
        if (!fs.statSync(target).isFile()) continue;
      } catch {
        continue; // not there yet; nothing to link to
      }
      links.push(
        new vscode.DocumentLink(
          new vscode.Range(i, ref.span.start, i, ref.span.end),
          vscode.Uri.file(target)
        )
      );
    }
  }
  return links;
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
    vscode.languages.registerDocumentLinkProvider(
      { language: "jsdata-cfg" },
      { provideDocumentLinks: documentLinks }
    ),
    // config save refreshes per spec; source saves too (decls move around)
    vscode.workspace.onDidSaveTextDocument(refreshDecorations),
    vscode.window.onDidChangeVisibleTextEditors(refreshDecorations),
    decoration
  );
  refreshDecorations();
}

export function deactivate(): void {}
