# Changelog

All notable changes to the jsdata VSCode extension are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

The CLI has its own changelog: [`../CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

First release, not yet published to the Marketplace. Everything below is what
0.1.0 will ship with.

### Added

- **Track declaration under cursor** (`jsdata.track`, bound to
  <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>W</kbd>) — appends a
  `var NAME = <path> -> const NAME` line to the workspace's `jsdata.cfg`,
  pre-filling the export name from the identifier under the cursor and
  validating it as a JS identifier. Refuses a name the config already tracks.
- Inline decoration marking every declaration a config already tracks, so you
  can see at a glance which `const`s feed the generated module. Refreshes on
  save and when the visible editors change.
- **Syntax highlighting** for `jsdata.cfg` — directives, arrows, version pins
  and tag lists, declaration names, and paths (including `~`, `%VAR%`, `$VAR`
  and `${VAR:-fallback}` expansions). A `let` right-hand side is highlighted as
  JavaScript, which is what it becomes in the generated module.
- **Clickable paths** — <kbd>Ctrl</kbd>-click a source, markdown, config or
  `languages.json` path to open it. Paths resolve against the config file's own
  directory, matching the CLI.
- Snippets for every directive (`out`, `var`, `varv`, `let`, `md`, `mdhtml`,
  `langs`, `vars`, `varsfile`, `version`, `versionTags`).

### Notes

- The extension depends only on the `var NAME = <path> -> const DECL` line
  grammar, which it parses itself; it never runs the `jsdata` binary.
- The language is bound to the exact filename `jsdata.cfg` (and
  `*.jsdata.cfg`), not to `*.cfg` — claiming every `.cfg` would take over
  `vertion.cfg` and any INI-style config in the same workspace.
- Highlighting describes shape, not validity: a path that does not exist is
  coloured like one that does. `jsdata --check` remains the judge. A link is
  only offered for a path that resolves to a file that exists, and never for one
  containing an unexpanded `~` / `%VAR%` / `$VAR`, since only the CLI knows the
  environment it will run in.
- The config path is fixed at `<workspace root>/jsdata.cfg`. A setting for
  configs kept elsewhere will follow if anyone needs one.
- Decoration matching uses a column-0 regex rather than the CLI's full lexer.
  That is good enough to mark a line a human is looking at, but it will not spot
  a declaration that is indented or nested.
