<!--
Thanks for the patch. Fill in what applies and delete the rest — this is a
prompt, not a form to complete exhaustively.
-->

## What this changes

<!-- One or two sentences. If it fixes an issue, "Fixes #123" here. -->

## Why

<!-- The problem behind the change. For a bug, what went wrong; for a feature,
     what wasn't possible before. -->

## How it was verified

<!-- What you actually ran, and anything you checked by hand. `--watch` has thin
     automated coverage, so manual verification genuinely matters there. -->

- [ ] `cargo fmt --all`
- [ ] `cargo clippy --locked --all-targets -- -D warnings`
- [ ] `cargo test --locked`
- [ ] `npm run compile` in `extension/` (only if the extension changed)

## Checklist

- [ ] Tests added or updated, in the suite that fits the change
      (unit for extraction/filtering logic, `tests/integration.rs` for the
      library end to end, `tests/cli.rs` for binary behaviour)
- [ ] User-visible changes noted in `CHANGELOG.md`
      (or `extension/CHANGELOG.md`)
- [ ] Docs updated if behaviour, flags, or the config grammar changed
      (`README.md` is the only reference there is)
- [ ] Config grammar changed in Rust → checked whether the extension's
      `parseVars` in `extension/src/extension.ts` needs the same change; it
      mirrors the `var` line grammar and the tests will not tell you
- [ ] Marker or tag semantics changed → confirmed it still matches Vertion,
      whose filter jsdata reuses verbatim so the two cannot disagree
