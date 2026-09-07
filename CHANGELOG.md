# Changelog

All notable changes to the jsdata CLI are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The VSCode extension has its own changelog:
[`extension/CHANGELOG.md`](extension/CHANGELOG.md).

## [Unreleased]

First release, not yet published to crates.io. Everything below is what 0.1.0
will ship with.

### Config

- Line-based `jsdata.cfg` grammar. Blank lines and `#` comments are ignored, and
  every path resolves against the config file's own directory.
- `out PATH` — where to write the generated module. Defaults to `_wikiData.js`
  next to the config.
- `var NAME = PATH -> const DECL` — extract a top-level `const` from a source
  file and export it as `NAME`. `export const` is accepted; the line splits on
  the last `->`, so paths containing `->` still work.
- `let NAME = EXPR` — emit a JavaScript expression verbatim, with no source file
  involved.
- `langs <-- TEXTS <-- languages.json` — read every listed locale's `.lang` file
  into a `{ locale: { key: value } }` object. A listed locale with no file is
  warned about and skipped rather than failing the build.
- `md NAME <-- FILE [<-- html]` — export a markdown file as a JS string, either
  verbatim or converted to HTML.
- `version` and `versionTags` directives setting defaults for the whole config.
- Path expansion for `~`, `%VAR%`, `$VAR`, `${VAR}` and `${VAR:-fallback}`. The
  fallback form lets one config serve both a plain checkout and a Vertion build
  tree.

### Extraction

- Declarations are copied **verbatim** — the text between `=` and the end of the
  declaration is preserved exactly, so comments, template literals, trailing
  commas and computed expressions all survive.
- A brace/string/regex-aware scanner, so a `}` inside a string, a line comment,
  a block comment or a regex literal does not end a declaration early.
- Only top-level declarations are matched, and only the first binding of a
  `const A = 1, B = 2` list.

### Version filtering

- Reuses [Vertion](https://github.com/vertX-dev/vertion)'s marker filter
  verbatim rather than reimplementing it, so jsdata and `vertion build` cannot
  disagree about what a marker means.
- `--version SPEC` accepts a cumulative version (`2.1`) or a range (`2.1 2.3`).
- Per-directive `--- <spec> [tags]` pins, resolving independently for the
  version and the tags: a pin naming no tags still inherits the surrounding tag
  filter.
- Tags are opt-in, matching Vertion: with no tag filter every *tagged* block is
  skipped and untagged content always survives. `[*]` admits every tag; `[]`
  means "no tags active here", which is how one entry overrides an inherited
  `versionTags`.
- When a file declares the same `const` several times behind different markers,
  the newest surviving declaration wins. If every declaration is gated out, the
  entry is skipped with a warning — along with any `let` that referenced it, so
  the emitted module never names something that isn't there. A declaration that
  exists at *no* version stays a hard error, since that is a typo rather than a
  version gap.

### CLI

- `--check` validates without writing; `--pull` builds once; `--watch` rebuilds
  on change; `--structure` prints what the config resolves to, including which
  version of a multiply-declared `const` this run would select.
- `--pull-all` merges several projects into one namespaced module, either by
  rebuilding each from its own config (`<-- CONFIG`) or by pulling an
  already-generated file (`--> FILE`). Each project becomes an IIFE so
  intra-project references still resolve.
- With no config argument, `./jsdata.cfg` is used, then the nearest one in a
  parent directory — which is what lets a Vertion `run` line just say `jsdata`
  even though the build runs from the output folder.
- `--version` means "extract as of this version", **not** "print the tool's
  version". A `--tag` that still looks like an unexpanded `%VAR%` or `$VAR` is
  dropped with a warning rather than silently filtering everything away.
- Errors are collected across entries rather than failing on the first one, and
  the output file is only rewritten when something other than the timestamp
  changed.

### Lints

- Warns when a `let` expression references a name declared later in the config,
  which would be a temporal-dead-zone error at module load.
- Warns when a `--- <spec>` pin points at a source inside `$VERTION_OUTPUT`,
  since that tree is already filtered and the pin can only narrow it further.
