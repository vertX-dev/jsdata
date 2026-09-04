//! jsdata — extracts JS declarations from addon sources into a generated
//! `_wikiData.js` ES module. Library API so the wiki build (or other tools)
//! can embed it; the `jsdata` binary is a thin CLI wrapper.

pub mod config;
pub mod emit;
pub mod extract;
pub mod lang;
pub mod markdown;
pub mod paths;
pub mod pick;
pub mod structure;
pub mod version;
pub mod watch;

use config::{Config, Entry, MergeEntry};
use version::VersionSpec;
use std::fs;
use std::path::{Path, PathBuf};

/// Everything a single run can be told from the command line: `--version` and
/// `--tag`. Per-project versions live on the merge config's `vars` lines
/// (`--- <spec>`), which is where they belong — with the project they describe.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub version: VersionSpec,
}

impl Options {
    pub fn from_spec(version: Option<&str>) -> Options {
        Options {
            version: version.map(VersionSpec::from_spec).unwrap_or_default(),
        }
    }
}

/// Pick the effective filter from layers ordered **highest precedence first**.
///
/// The version and the tags resolve *independently*, so a `--- 2.4` pin that
/// names no tags still inherits `--tag` / `versionTags`. Write `--- 2.4 []` for
/// "this entry, with no tags active", or `--- 2.4 [*]` to admit every tag.
pub(crate) fn effective_version(layers: &[Option<&VersionSpec>]) -> Option<VersionSpec> {
    let spec = layers.iter().flatten().find_map(|v| v.spec.clone());
    let tags = layers.iter().flatten().find_map(|v| v.tags.clone());
    let v = VersionSpec { spec, tags };
    if v.is_empty() { None } else { Some(v) }
}

#[derive(Debug)]
pub struct Outcome {
    pub out: PathBuf,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// `Some(true)` wrote, `Some(false)` unchanged, `None` not written
    /// (check mode or errors).
    pub wrote: Option<bool>,
}

/// Read and parse a config file. Parse errors are collected alongside the
/// entries that did parse.
pub fn load(config_path: &Path) -> Result<(Config, Vec<String>), String> {
    let text = fs::read_to_string(config_path)
        .map_err(|e| format!("cannot read {}: {e}", config_path.display()))?;
    let dir = config_path.parent().unwrap_or(Path::new(""));
    Ok(config::parse(&text, dir))
}

/// The entry's own `--- <spec>` pin, if any (versionable entry kinds only).
pub(crate) fn entry_version(entry: &Entry) -> Option<&VersionSpec> {
    match entry {
        Entry::Var { version, .. } | Entry::Langs { version, .. } => version.as_ref(),
        _ => None,
    }
}

/// Filter `src` to `version` (Vertion markers) before extraction, if a version
/// is in effect; otherwise return it unchanged.
fn apply_version(
    src: &str,
    path: &Path,
    version: Option<&VersionSpec>,
) -> Result<String, String> {
    match version {
        Some(v) => {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            version::filter_source(src, ext, v)
        }
        None => Ok(src.to_string()),
    }
}

/// Produce one entry's JS value: read + (optionally version-filter) + extract
/// for `var`, verbatim for `let`. `version` is the already-resolved effective
/// version for this entry (see the precedence in [`collect_config_values`]).
pub fn resolve(entry: &Entry, version: Option<&VersionSpec>) -> Result<Option<String>, String> {
    match entry {
        Entry::Let { expr, .. } => Ok(Some(expr.clone())),
        Entry::Var { name, source, decl, .. } => {
            let raw = fs::read_to_string(source)
                .map_err(|e| format!("{name}: cannot read {}: {e}", source.display()))?;
            extract_declaration(&raw, source, decl, version)
                .map_err(|e| format!("{name}: {e}"))
        }
        Entry::Langs { .. } => resolve_langs_entry(entry, version).0,
        Entry::Md { name, source, html, .. } => {
            let md = fs::read_to_string(source)
                .map_err(|e| format!("{name}: cannot read {}: {e}", source.display()))?;
            let content = if *html { markdown::to_html(&md) } else { md };
            Ok(Some(emit::json_str(&content)))
        }
    }
}

/// Pull one declaration out of a source file, resolving **duplicates** by
/// version.
///
/// A file may declare the same `const` several times, each in its own marker
/// block. The filter removes marker lines, so the filtered text can't say which
/// surviving declaration is newest — [`pick`] answers that from the *unfiltered*
/// source, and the rank it returns indexes into the filtered text (filtering
/// deletes the losers and preserves the order of the rest).
fn extract_declaration(
    raw: &str,
    source: &Path,
    decl: &str,
    version: Option<&VersionSpec>,
) -> Result<Option<String>, String> {
    let filtered = apply_version(raw, source, version)?;

    // The candidate filter must match the one that produced `filtered`, or the
    // winner's rank won't index into it. With no version that filter is the
    // identity: every declaration survives, and `since` alone picks the newest.
    let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("");
    let parts = version.map(version::filter_parts).transpose()?;
    let cands = pick::candidates(
        raw,
        decl,
        version::style_for(ext),
        parts.as_ref().map(|(m, t)| (m, t.as_slice())),
    )?;

    // Not declared at any version: a name that isn't in the file at all is a
    // typo in the config, not a version gap, so it stays a hard error.
    if cands.is_empty() {
        return Err(format!(
            "`const {decl}` not found in {} at any version",
            source.display()
        ));
    }
    if cands.len() == 1 {
        return Ok(extract::extract(&filtered, decl).ok());
    }

    // Declared, but every declaration is gated out here — skip, don't fail.
    let Some(rank) = pick::choose(&cands) else {
        return Ok(None);
    };

    let surviving: Vec<String> = extract::extract_all(&filtered)?
        .into_iter()
        .filter(|(n, _)| n == decl)
        .map(|(_, v)| v)
        .collect();
    Ok(surviving.into_iter().nth(rank))
}

/// [`extract_declaration`] for the structure report: any failure reads as
/// "no value at this version" rather than propagating, because a timeline row
/// legitimately covers versions where the declaration doesn't exist.
pub(crate) fn extract_declaration_for_report(
    raw: &str,
    source: &Path,
    decl: &str,
    version: Option<&VersionSpec>,
) -> Option<String> {
    extract_declaration(raw, source, decl, version).ok().flatten()
}

/// Read `languages.json` + every `<locale>.lang`, optionally version-filter
/// each `.lang` (with `#` markers), render the `{ locale: { key: value } }`
/// object literal. Returns the JS value plus warnings (a listed locale with no
/// `.lang` file is warned + skipped). Errors only if `languages.json` is
/// unreadable or the version spec is invalid.
pub fn resolve_langs(
    texts_dir: &Path,
    languages_json: &Path,
    version: Option<&VersionSpec>,
) -> Result<(String, Vec<String>), String> {
    let json = fs::read_to_string(languages_json)
        .map_err(|e| format!("langs: cannot read {}: {e}", languages_json.display()))?;
    let locales = lang::parse_languages_json(&json);
    let mut warnings = Vec::new();
    if locales.is_empty() {
        warnings.push(format!("langs: no locales in {}", languages_json.display()));
    }
    let mut out = Vec::new();
    for loc in locales {
        let path = texts_dir.join(format!("{loc}.lang"));
        match fs::read_to_string(&path) {
            Ok(txt) => {
                let txt = match version {
                    Some(v) => version::filter_source(&txt, "lang", v)
                        .map_err(|e| format!("langs: {loc}: {e}"))?,
                    None => txt,
                };
                out.push((loc, lang::parse_lang(&txt)));
            }
            Err(e) => warnings.push(format!(
                "langs: {loc}: cannot read {} ({e}) — skipped",
                path.display()
            )),
        }
    }
    Ok((emit::render_langs_object(&out), warnings))
}

/// [`resolve`] plus warnings, for any entry kind. `version` is the effective
/// version already resolved for this entry.
pub(crate) fn resolve_langs_entry(
    entry: &Entry,
    version: Option<&VersionSpec>,
) -> (Result<Option<String>, String>, Vec<String>) {
    match entry {
        Entry::Langs { texts_dir, languages_json, .. } => {
            match resolve_langs(texts_dir, languages_json, version) {
                Ok((js, warns)) => (Ok(Some(js)), warns),
                Err(e) => (Err(e), vec![]),
            }
        }
        _ => (resolve(entry, version), vec![]),
    }
}

/// Lint: a `--- <spec>` version pin on a source that lives inside the tree
/// [Vertion] just built.
///
/// Vertion keeps its `//version` markers in build output by default, so the pin
/// still *runs* — but the tree has already been filtered, so the pin can only
/// narrow further, never recover content the build dropped. That makes it a
/// silent no-op in the common case. The two coherent setups are:
///
/// - **follow the build** — sources under `$VERTION_OUTPUT`, no pins
/// - **multi-version wiki** — sources under `./src`, pins kept
///
/// [Vertion]: https://github.com/vertX-dev/vertion
pub fn pin_under_build_warnings(cfg: &Config) -> Vec<String> {
    let Some(build_dir) = std::env::var_os("VERTION_OUTPUT")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    else {
        return Vec::new();
    };
    pins_under(cfg, &build_dir)
}

/// The lint itself, with the build directory passed in — so tests don't have to
/// mutate a process-global environment variable while running in parallel.
fn pins_under(cfg: &Config, build_dir: &Path) -> Vec<String> {
    cfg.entries
        .iter()
        .filter(|e| entry_version(e).is_some())
        .filter_map(|e| match e {
            Entry::Var { name, source, .. } => Some((name, source)),
            Entry::Langs {
                name, texts_dir, ..
            } => Some((name, texts_dir)),
            _ => None,
        })
        .filter(|(_, source)| source.starts_with(build_dir))
        .map(|(name, source)| {
            format!(
                "`{name}`: version pin on {}, which is inside the vertion build output \
                 ({}) — that tree is already filtered, so the pin can only narrow it further. \
                 Either drop the pin, or point the source at the unfiltered sources.",
                source.display(),
                build_dir.display()
            )
        })
        .collect()
}

/// TDZ lint: a `let` expression textually referencing a name declared later
/// in the config would throw at module load time.
pub fn tdz_warnings(cfg: &Config) -> Vec<String> {
    let mut warns = Vec::new();
    for (i, e) in cfg.entries.iter().enumerate() {
        if let Entry::Let { name, expr } = e {
            for later in &cfg.entries[i + 1..] {
                if references(expr, later.name()) {
                    warns.push(format!(
                        "`{name}` references `{}`, which is declared later (TDZ error at runtime)",
                        later.name()
                    ));
                }
            }
        }
    }
    warns
}

/// Whole-identifier textual match.
fn references(expr: &str, name: &str) -> bool {
    let b = expr.as_bytes();
    let is_ident_byte = |c: u8| c == b'_' || c == b'$' || c.is_ascii_alphanumeric();
    let mut from = 0;
    while let Some(p) = expr[from..].find(name) {
        let (s, e) = (from + p, from + p + name.len());
        if (s == 0 || !is_ident_byte(b[s - 1])) && (e == b.len() || !is_ident_byte(b[e])) {
            return true;
        }
        from = s + 1;
    }
    false
}

/// Load a config and resolve every entry to `(name, js)` pairs, without
/// writing. Returns the output path plus collected errors and warnings.
/// Shared by [`build`] and by `--pull-all`'s `<-- CONFIG` project form.
///
/// `version_override` is the command `-v` (or, for a merge sub-build, the
/// project's effective version). Per-entry version precedence, highest first:
/// the entry's own `--- <spec>` pin → `version_override` → the config's
/// `version` directive → unfiltered.
/// What one config yields: the `out` path plus the `(name, js)` pairs, with
/// errors and warnings collected rather than fail-fast.
struct Collected {
    out: PathBuf,
    values: Vec<(String, String)>,
    errors: Vec<String>,
    warnings: Vec<String>,
}

fn collect_config_values(config_path: &Path, cli: &VersionSpec) -> Collected {
    let (cfg, mut errors) = match load(config_path) {
        Ok(v) => v,
        Err(e) => {
            return Collected {
                out: PathBuf::new(),
                values: vec![],
                errors: vec![e],
                warnings: vec![],
            };
        }
    };
    let cfg_version = cfg.version.as_ref();
    let mut values = Vec::new();
    let mut warnings = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for entry in &cfg.entries {
        let eff = effective_version(&[entry_version(entry), Some(cli), cfg_version]);

        // A `let` whose expression names a skipped entry would reference an
        // export that no longer exists — drop it too rather than emit a module
        // that throws on load.
        if let Entry::Let { name, expr } = entry {
            if let Some(gone) = skipped.iter().find(|s| references(expr, s)) {
                warnings.push(format!("{name}: references skipped `{gone}` — skipped"));
                skipped.push(name.clone());
                continue;
            }
        }

        let (res, warns) = resolve_langs_entry(entry, eff.as_ref());
        warnings.extend(warns);
        match res {
            Ok(Some(js)) => values.push((entry.name().to_string(), js)),
            Ok(None) => {
                let at = eff
                    .as_ref()
                    .and_then(|v| v.spec.clone())
                    .unwrap_or_else(|| "this version".into());
                warnings.push(format!(
                    "{}: not present at version {at} — skipped",
                    entry.name()
                ));
                skipped.push(entry.name().to_string());
            }
            Err(e) => errors.push(e),
        }
    }
    warnings.extend(tdz_warnings(&cfg));
    warnings.extend(pin_under_build_warnings(&cfg));
    Collected {
        out: cfg.out,
        values,
        errors,
        warnings,
    }
}

/// One-shot build. `write: false` is `--check` (validate only). `version` is
/// the command-line `-v` override. Errors are collected, not fail-fast.
pub fn build(config_path: &Path, write: bool, version: Option<&str>) -> Outcome {
    build_with(config_path, write, &Options::from_spec(version))
}

/// [`build`] with the full command-line [`Options`] (version, tags, replace).
pub fn build_with(config_path: &Path, write: bool, opts: &Options) -> Outcome {
    let Collected {
        out,
        values,
        mut errors,
        warnings,
    } = collect_config_values(config_path, &opts.version);
    let mut wrote = None;
    if errors.is_empty() && write {
        let content = emit::render(&config_path.display().to_string(), &values);
        match emit::write_if_changed(&out, &content) {
            Ok(w) => wrote = Some(w),
            Err(e) => errors.push(format!("cannot write {}: {e}", out.display())),
        }
    }
    Outcome {
        out,
        errors,
        warnings,
        wrote,
    }
}

/// `--pull-all`: merge several projects' generated wikiData modules into one
/// namespaced module. Reads a `parse_merge` config, pulls every top-level
/// export from each project's `_wikiData.js`, and emits one grouped object per
/// project. `write: false` is `--check`. `version` is the command-line `-v`.
///
/// Per-project version precedence, highest first: the project's `--- <spec>`
/// pin → the CLI per-project override (`per_project[name]`) → the CLI global
/// `version` → the merge config's `version` directive.
pub fn pull_all(config_path: &Path, write: bool, version: Option<&str>) -> Outcome {
    pull_all_with(config_path, write, &Options::from_spec(version))
}

/// [`pull_all`] with the full command-line [`Options`], applied to this config
/// and to every project config it rebuilds.
pub fn pull_all_with(config_path: &Path, write: bool, opts: &Options) -> Outcome {
    let text = match fs::read_to_string(config_path) {
        Ok(t) => t,
        Err(e) => {
            return Outcome {
                out: PathBuf::new(),
                errors: vec![format!("cannot read {}: {e}", config_path.display())],
                warnings: vec![],
                wrote: None,
            }
        }
    };
    let dir = config_path.parent().unwrap_or(Path::new(""));
    let (cfg, mut errors) = config::parse_merge(&text, dir);
    let merge_version = cfg.version.as_ref();
    let mut warnings = Vec::new();
    let mut values: Vec<(String, String)> = Vec::new();
    for entry in &cfg.entries {
        match entry {
            MergeEntry::Project { name, source } => match fs::read_to_string(source) {
                // a pre-generated file has no markers; version doesn't apply
                Ok(src) => match extract::extract_all(&src) {
                    Ok(vars) => {
                        if vars.is_empty() {
                            warnings.push(format!("{name}: no exports found in {}", source.display()));
                        }
                        values.push((name.clone(), emit::render_project_iife(&vars)));
                    }
                    Err(e) => errors.push(format!("{name}: {}: {e}", source.display())),
                },
                Err(e) => errors.push(format!("{name}: cannot read {}: {e}", source.display())),
            },
            MergeEntry::ProjectFromConfig { name, config, version: pin } => {
                // rerun the normal build for this project, in memory, at its
                // effective version: pin → CLI per-project → CLI global → merge-global
                let eff = effective_version(&[
                    pin.as_ref(),
                    Some(&opts.version),
                    merge_version,
                ])
                .unwrap_or_default();
                let Collected {
                    values: vars,
                    errors: errs,
                    warnings: warns,
                    ..
                } = collect_config_values(config, &eff);
                warnings.extend(warns.into_iter().map(|w| format!("{name}: {w}")));
                if errs.is_empty() {
                    if vars.is_empty() {
                        warnings.push(format!("{name}: no exports built from {}", config.display()));
                    }
                    values.push((name.clone(), emit::render_project_iife(&vars)));
                } else {
                    errors.extend(errs.into_iter().map(|e| format!("{name}: {e}")));
                }
            }
            MergeEntry::Langs { name, texts_dir, languages_json, version: pin } => {
                let eff = effective_version(&[
                    pin.as_ref(),
                    Some(&opts.version),
                    merge_version,
                ]);
                match resolve_langs(texts_dir, languages_json, eff.as_ref()) {
                    Ok((js, warns)) => {
                        warnings.extend(warns);
                        values.push((name.clone(), js));
                    }
                    Err(e) => errors.push(e),
                }
            }
        }
    }
    let mut wrote = None;
    if errors.is_empty() && write {
        let content = emit::render(&config_path.display().to_string(), &values);
        match emit::write_if_changed(&cfg.out, &content) {
            Ok(w) => wrote = Some(w),
            Err(e) => errors.push(format!("cannot write {}: {e}", cfg.out.display())),
        }
    }
    Outcome {
        out: cfg.out,
        errors,
        warnings,
        wrote,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_identifier_only() {
        assert!(references("Object.keys(PASSIVES).length", "PASSIVES"));
        assert!(!references("Object.keys(PASSIVES2).length", "PASSIVES"));
        assert!(!references("MY_PASSIVES.length", "PASSIVES"));
        assert!(references("PASSIVES", "PASSIVES"));
    }

    #[test]
    fn warns_about_a_pin_inside_the_build_output() {
        // A pin against an already-filtered tree can only narrow further, so
        // it's a silent no-op — the one mistake this design invites.
        let (cfg, errs) = config::parse(
            concat!(
                "var PINNED = /build/2.5.0/BP/a.js -> const A --- 1.2\n",
                "var UNPINNED = /build/2.5.0/BP/b.js -> const B\n",
                "var ELSEWHERE = /src/BP/c.js -> const C --- 1.2\n",
                "langs <-- /build/2.5.0/RP/texts <-- /build/2.5.0/RP/texts/l.json --- 1.2\n",
            ),
            Path::new("base"),
        );
        assert!(errs.is_empty(), "{errs:?}");

        let warns = pins_under(&cfg, Path::new("/build/2.5.0"));
        // The pinned var and the pinned langs bundle; not the unpinned one, and
        // not the pinned source that lives outside the build tree.
        assert_eq!(warns.len(), 2, "{warns:?}");
        assert!(warns[0].contains("PINNED"), "{}", warns[0]);
        assert!(warns[0].contains("already filtered"), "{}", warns[0]);
        assert!(warns[1].contains("langs"), "{}", warns[1]);
        assert!(!warns.iter().any(|w| w.contains("UNPINNED")));
        assert!(!warns.iter().any(|w| w.contains("ELSEWHERE")));
    }

    #[test]
    fn no_pin_warning_for_sources_outside_the_build() {
        let (cfg, _) = config::parse(
            "var PINNED = ./src/BP/a.js -> const A --- 1.2\n",
            Path::new("base"),
        );
        assert!(pins_under(&cfg, Path::new("/build/2.5.0")).is_empty());
    }
}
