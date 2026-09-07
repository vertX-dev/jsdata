# jsdata for VS Code

Editor support for [`jsdata`](https://github.com/vertX-dev/jsdata) — a CLI that extracts `const`
declarations out of JS source files into one generated ES module, driven by a `jsdata.cfg`.

## Features

**Syntax highlighting for `jsdata.cfg`** — directives, arrows, version pins and tag lists,
declaration names, and paths with their `~` / `%VAR%` / `$VAR` / `${VAR:-fallback}` expansions.
A `let` right-hand side is highlighted as JavaScript, because that is what it becomes in the
generated module.

```
out src/wiki/_wikiData.js
version 2.4 [beta]

var PASSIVES = src/BP/scripts/passives.js -> const PASSIVES
var TOOLS    = src/BP/scripts/tools.js -> export const TOOLS_VANILLA --- 2.1 2.3
let PASSIVE_COUNT = Object.keys(PASSIVES).length
md GUIDE <-- docs/guide.md <-- html
langs <-- RP/texts <-- RP/texts/languages.json
```

Highlighting describes *shape*, not validity — a path that does not exist is coloured like one
that does. `jsdata --check` remains the judge of whether a config is correct.

**Ctrl+click a path** to open it. Paths resolve against the config file's own directory, matching
the CLI. A path that does not exist, or that still contains an unexpanded variable, is not linked
— only the CLI knows the environment it will run in.

**Snippets** for every directive: `out`, `var`, `varv`, `let`, `md`, `mdhtml`, `langs`, `vars`,
`varsfile`, `version`, `versionTags`.

**Track the declaration under the cursor** — <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>W</kbd> appends
a `var NAME = <path> -> const NAME` line to the workspace's `jsdata.cfg`, pre-filling the export
name from the identifier under the cursor. Declarations a config already tracks get an inline
marker, so you can see at a glance which `const`s feed the generated module.

## File association

The language binds to the exact filename `jsdata.cfg`, and to `*.jsdata.cfg`. It deliberately
does **not** claim `*.cfg`, which would take over `vertion.cfg` and every INI-style config in the
same workspace. To use it on a differently named file, add to your settings:

```json
"files.associations": { "wiki.cfg": "jsdata-cfg" }
```

## Requirements

None. The extension parses the config itself and never runs the `jsdata` binary — you only need
the CLI to actually build a module.

## Notes

The keybind and the inline marker read `jsdata.cfg` from the workspace root. Marker matching uses
a column-0 regex rather than the CLI's full lexer: good enough to mark a line you are looking at,
but it will not spot a declaration that is indented or nested.

Licensed MIT. Changes are listed in [CHANGELOG.md](CHANGELOG.md).
