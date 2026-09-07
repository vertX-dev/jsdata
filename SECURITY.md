# Security Policy

## Supported versions

jsdata is developed on `main`. Fixes land in the next release; there are no
long-term support branches.

| Version | Supported |
| ------- | --------- |
| 0.1.x   | Yes       |

The VSCode extension shares this policy and its version line with the CLI.

## Reporting a vulnerability

Please report privately through GitHub, not in a public issue:

**[Open a private advisory](https://github.com/vertX-dev/jsdata/security/advisories/new)**
— or go to the repository's **Security** tab and choose *Report a vulnerability*.

Useful things to include: the version, your OS, a minimal `jsdata.cfg` plus
source tree that reproduces it, and what you expected to happen instead.

Expect an acknowledgement within a week. If a report is confirmed, the fix and
an advisory go out together, and you are credited unless you ask otherwise.

## Threat model

**jsdata executes nothing.** Unlike [Vertion](https://github.com/vertX-dev/vertion),
whose `vertion.cfg` deliberately spawns shells via `run`, `run_here` and
`[conditions.*].cmd`, a `jsdata.cfg` has no field that runs a command. There are
no hooks, no scripts, no plugins. Running jsdata on a repository you did not
write is closer to running a formatter over it than to running its build.

What a config *can* do is read files and write one:

- **Read any path you can read.** Sources are ordinary paths and may be absolute
  or use `~`, so a config can name something outside its own project, and the
  contents of whatever it names are copied into the generated module.
- **Expand environment variables into paths.** `%VAR%`, `$VAR`, `${VAR}` and
  `${VAR:-fallback}` resolve from the process environment at build time. The
  values only ever build paths — they are never copied into the output — but a
  config can still steer a read toward a location you did not intend.
- **Write to the `out` path**, which is likewise unconstrained.

So: **read a `jsdata.cfg` from someone else before you run it, the same way you
would read their `Makefile`.** Reports that a config can read a file it names
describe intended behaviour and will be closed.

What *is* a vulnerability:

- executing anything at all — nothing in a config or a source file should ever
  reach a shell
- a **source file's contents** changing which paths get read or written; markers
  and declarations are data, and must never influence path resolution
- writing outside the configured `out` path
- reading a path the config did not name

## `md ... <-- html` does not sanitize

The `md NAME <-- FILE <-- html` directive converts markdown with raw HTML passed
straight through, so `<script>` in the source file lands verbatim in the
generated module and then in whatever page renders it.

This is intended: the directive exists for changelogs and docs you wrote
yourself. It is documented in the README's Security section, and a report that
raw HTML survives conversion will be closed as working-as-intended.

It becomes a real problem the moment the markdown is not yours. If you ever
point `md` at contributor-submitted content, sanitize downstream before
rendering — jsdata will not do it for you.

## Scope notes for the VSCode extension

The extension reads `jsdata.cfg` from the workspace root and appends `var` lines
to it. It never runs the `jsdata` binary, and it does not resolve or read the
source paths a config names — it only parses config lines and decorates matching
declarations in files you already have open.

If you find a way to make the extension execute anything, or write outside the
workspace's `jsdata.cfg`, that is a vulnerability — please report it.
