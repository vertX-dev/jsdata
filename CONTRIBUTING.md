# Contributing to jsdata

Thanks for taking the time. Bug reports and small focused pull requests are both
welcome; if you're planning something large, open an issue first so we can agree
on the shape before you write it.

## Getting set up

jsdata needs **Rust 1.85 or newer**. That floor is real, not aspirational: CI
runs a `cargo check` against 1.85 on every push, and the `vertion` dependency
declares the same minimum.

```sh
git clone https://github.com/vertX-dev/jsdata.git
cd jsdata
cargo build
cargo test
```

The VSCode extension is a separate Node project:

```sh
cd extension
npm ci
npm run compile
```

### Changing the config grammar

The config DSL is described in three places, and a change to one usually needs
the other two:

| Where | What it does |
|---|---|
| [`src/config.rs`](src/config.rs) | The parser. The only authority on what is valid. |
| [`extension/syntaxes/jsdata.tmLanguage.json`](extension/syntaxes/jsdata.tmLanguage.json) | Highlighting. Deliberately loose — it colours shape and never judges validity, so it cannot disagree with the parser about whether a config is correct. |
| [`extension/src/extension.ts`](extension/src/extension.ts) | `pathRefs` mirrors the splitting rules (the ` ---` pin first, then the arrows) to find the paths worth linking; `parseVars` mirrors the `var` line for decorations. |

Adding a directive means a new rule in the grammar, a case in `pathRefs` if it
names a path, and a snippet in
[`extension/snippets/jsdata.code-snippets`](extension/snippets/jsdata.code-snippets).

## Before you open a pull request

Run what CI runs, so you find out here rather than there:

```sh
cargo fmt --all
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

CI additionally runs the test suite on Windows and macOS, a `cargo check` on
Rust 1.85, `cargo audit`, and the extension's `tsc` build. All five jobs must
pass.

## Tests

Three suites, and which one you want depends on what you changed:

| Where | What it covers |
| --- | --- |
| `src/**` unit tests | Logic in isolation: config parsing, declaration extraction, marker filtering, duplicate picking |
| `tests/integration.rs` | The library end to end — a real config and source tree on disk, driven through `jsdata::build` |
| `tests/cli.rs` | The real binary — argument parsing, exit codes, stdout/stderr text |

A change to extraction or filtering usually wants a unit test. A change to
argument handling, exit codes, or what gets printed wants a `tests/cli.rs` test.

Two traps worth knowing before you write assertions:

- **The generated module's header embeds the config's absolute path.** Asserting
  that some short word is *absent* from the whole file therefore depends on
  where the temp directory happens to live — macOS puts it under
  `/var/folders/…`, and "f-**old**-ers" contains "old". `tests/integration.rs`
  has a `body()` helper that strips the header; use it whenever the needle is a
  plain word.
- **Tags are opt-in.** With no tag filter active, every *tagged* marker block is
  skipped and only untagged content survives. This is Vertion's semantics, which
  jsdata reuses verbatim so that the generated module matches what the build
  actually shipped. `[*]` is the wildcard.

## Style

Match the file you're editing. A few things that hold throughout:

- Comments explain *why*, not *what*. If a line needs a comment to say what it
  does, the line is usually the problem.
- Error messages are lowercase and go to stderr as `error: <message>`; warnings
  as `warning: <message>`. Neither aborts a run that can still produce output —
  errors are collected across entries rather than failing on the first one.
- A missing declaration is an error, but a declaration that exists and is gated
  out at the requested version is a *warning* and a skip. Keep that distinction;
  it is the difference between a typo and a version gap.

## The relationship with Vertion

jsdata does not parse version markers itself. It reuses Vertion's `config`,
`filter` and `parser` modules so that the two cannot disagree about what a
marker means — if `vertion build --tag beta` ships a block, `jsdata --tag beta`
documents it.

If you find yourself about to reimplement marker handling, don't. Fix it
upstream in Vertion instead, and bump the dependency.

## Security

Please don't file security problems as public issues — see
[SECURITY.md](SECURITY.md), which also explains the threat model: a `jsdata.cfg`
executes nothing, but it can read any path it names.

## Commits and releases

Branch off `main`. Keep the subject line short and imperative.

User-visible changes belong in [CHANGELOG.md](CHANGELOG.md), and extension
changes in [extension/CHANGELOG.md](extension/CHANGELOG.md).
