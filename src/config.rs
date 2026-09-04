//! Parser for the jsdata config DSL. Strictly line-based; no multiline,
//! no escaping. All paths resolve relative to the config file's directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::version::VersionSpec;

#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// `var NAME = path -> const DECL` — extracted from a source file.
    /// `version` is the optional `--- <spec>` per-directive pin.
    Var {
        name: String,
        source: PathBuf,
        decl: String,
        version: Option<VersionSpec>,
    },
    /// `let NAME = expr` — expression emitted as-is.
    Let { name: String, expr: String },
    /// `langs <-- TEXTS <-- languages.json` — locale `.lang` bundle, exported
    /// as `langs`. Valid in both a normal (`--pull`) and a merge config.
    Langs {
        name: String,
        texts_dir: PathBuf,
        languages_json: PathBuf,
        version: Option<VersionSpec>,
    },
    /// `md NAME <-- FILE [<-- html]` — a markdown file as a JS string, either
    /// verbatim or converted to HTML.
    Md {
        name: String,
        source: PathBuf,
        html: bool,
    },
}

impl Entry {
    pub fn name(&self) -> &str {
        match self {
            Entry::Var { name, .. }
            | Entry::Let { name, .. }
            | Entry::Langs { name, .. }
            | Entry::Md { name, .. } => name,
        }
    }
}

/// Split a trailing `--- <spec> [tags]` version pin off a directive's RHS. The
/// separator is ` ---` (space + three dashes), which never collides with the
/// `->`/`-->`/`<--` arrows (all two-dash). The pin text is returned raw; the
/// caller parses it so a bad pin is reported with its line number.
fn take_version_suffix(rest: &str) -> (&str, Option<&str>) {
    if let Some(pos) = rest.rfind(" ---") {
        let head = rest[..pos].trim_end();
        let spec = rest[pos + 4..].trim();
        if !spec.is_empty() {
            return (head, Some(spec));
        }
    }
    (rest, None)
}

/// Parse a raw `--- ` pin, tagging failures with the line number.
fn parse_pin(pin: Option<&str>, n: usize) -> Result<Option<VersionSpec>, String> {
    pin.map(|p| VersionSpec::parse(p).map_err(|e| format!("line {n}: {e}")))
        .transpose()
}

/// Match a `NAME VALUE` / `NAME = VALUE` / `NAME=VALUE` directive, returning the
/// value. The name must be followed by whitespace, `=`, or end-of-line, so
/// `version` does not swallow a `versionTags` line.
fn strip_directive<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(name)?;
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with('=')) {
        return None;
    }
    let rest = rest.trim_start();
    Some(rest.strip_prefix('=').unwrap_or(rest).trim())
}

/// Shared handling of the `version` / `versionTags` config directives.
/// Returns `true` when the line was one of them (parsed or errored).
fn take_config_version(
    line: &str,
    n: usize,
    spec: &mut Option<String>,
    tags: &mut Option<Vec<String>>,
    errors: &mut Vec<String>,
) -> bool {
    // versionTags first: `version` would otherwise match its prefix.
    if let Some(value) = strip_directive(line, "versionTags") {
        if tags.is_some() {
            errors.push(format!("line {n}: duplicate `versionTags`"));
        } else {
            // Accept `[a, b]`, `a, b` and `a b` alike.
            let inner = value.trim_start_matches('[').trim_end_matches(']');
            match VersionSpec::parse(&format!("[{inner}]")) {
                Ok(v) => *tags = v.tags,
                Err(e) => errors.push(format!("line {n}: versionTags {e}")),
            }
        }
        return true;
    }
    if let Some(value) = strip_directive(line, "version") {
        if value.is_empty() {
            errors.push(format!(
                "line {n}: `version` needs a spec (x.y or x.y x1.y1)"
            ));
        } else if spec.is_some() {
            errors.push(format!("line {n}: duplicate `version`"));
        } else {
            match VersionSpec::parse(value) {
                // `version 2.4 [beta]` is allowed too — it just sets both.
                Ok(v) => {
                    *spec = v.spec;
                    if v.tags.is_some() {
                        if tags.is_some() {
                            errors.push(format!("line {n}: duplicate `versionTags`"));
                        } else {
                            *tags = v.tags;
                        }
                    }
                }
                Err(e) => errors.push(format!("line {n}: {e}")),
            }
        }
        return true;
    }
    false
}

/// Fold the two directives into one optional filter.
fn config_version(spec: Option<String>, tags: Option<Vec<String>>) -> Option<VersionSpec> {
    let v = VersionSpec { spec, tags };
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Expand `~` / `%VAR%` / `$VAR` / `${VAR:-fallback}` in a path written in the
/// config. Failures are phrased like every other parse error so they land in the
/// same collected list.
fn expand_frag(raw: &str, n: usize) -> Result<String, String> {
    if !crate::paths::has_references(raw) {
        return Ok(raw.to_string());
    }
    crate::paths::expand(raw).map_err(|e| format!("line {n}: `{raw}`: {e}"))
}

/// Expand, then resolve against the config file's directory. An expanded
/// **absolute** path replaces `dir` through normal `join` semantics.
fn resolve(dir: &Path, raw: &str, n: usize) -> Result<PathBuf, String> {
    Ok(dir.join(expand_frag(raw, n)?))
}

/// Parse a `langs <-- TEXTS <-- languages.json` line's RHS (everything after
/// the `langs` keyword). Shared by the normal and merge parsers.
fn parse_langs_rhs(
    rest: &str,
    dir: &Path,
    n: usize,
    version: Option<VersionSpec>,
) -> Result<Entry, String> {
    let after = rest
        .strip_prefix("<--")
        .ok_or_else(|| format!("line {n}: expected `langs <-- TEXTS <-- languages.json`"))?;
    let (texts, ljson) = after
        .split_once("<--")
        .ok_or_else(|| format!("line {n}: missing second `<--` before languages.json"))?;
    let (texts, ljson) = (texts.trim(), ljson.trim());
    if texts.is_empty() || ljson.is_empty() {
        return Err(format!(
            "line {n}: texts dir and languages.json are both required"
        ));
    }
    Ok(Entry::Langs {
        name: "langs".into(),
        texts_dir: resolve(dir, texts, n)?,
        languages_json: resolve(dir, ljson, n)?,
        version,
    })
}

#[derive(Debug, Clone)]
pub struct Config {
    pub out: PathBuf,
    pub entries: Vec<Entry>,
    /// `version` + `versionTags` directives — defaults for every extraction
    /// here, each overridable by a `--- ` pin or a CLI flag.
    pub version: Option<VersionSpec>,
}

// unicode-aware, matching the extractor's identifier handling
pub fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// Parse config text. `dir` is the config file's directory. Errors are
/// collected, not fail-fast; entries that parsed fine are still returned.
pub fn parse(text: &str, dir: &Path) -> (Config, Vec<String>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out: Option<PathBuf> = None;
    let mut spec: Option<String> = None;
    let mut tags: Option<Vec<String>> = None;
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for (idx, raw) in text.lines().enumerate() {
        let n = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if take_config_version(line, n, &mut spec, &mut tags, &mut errors) {
            continue;
        }
        let (word, rest) = match line.split_once(char::is_whitespace) {
            Some((w, r)) => (w, r.trim()),
            None => (line, ""),
        };
        match word {
            "out" => {
                if rest.is_empty() {
                    errors.push(format!("line {n}: `out` needs a path"));
                } else if out.is_some() {
                    errors.push(format!("line {n}: duplicate `out`"));
                } else {
                    match resolve(dir, rest, n) {
                        Ok(p) => out = Some(p),
                        Err(e) => errors.push(e),
                    }
                }
            }
            "langs" => {
                let (rest, pin) = take_version_suffix(rest);
                let ver = match parse_pin(pin, n) {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                };
                match parse_langs_rhs(rest, dir, n, ver) {
                    Ok(entry) => {
                        if !seen.insert("langs".to_string()) {
                            errors.push(format!("line {n}: duplicate name `langs`"));
                        } else {
                            entries.push(entry);
                        }
                    }
                    Err(e) => errors.push(e),
                }
            }
            "md" => {
                let (rest, pin) = take_version_suffix(rest);
                if pin.is_some() {
                    errors.push(format!(
                        "line {n}: version filtering is not supported on `md` yet"
                    ));
                    continue;
                }
                // NAME <-- FILE           (verbatim markdown string)
                // NAME <-- FILE <-- html  (converted to HTML)
                let parts: Vec<&str> = rest.split("<--").map(str::trim).collect();
                let (name, path, html) = match parts.as_slice() {
                    [name, path] => (*name, *path, false),
                    [name, path, mode] if *mode == "html" => (*name, *path, true),
                    [_, _, other] => {
                        errors.push(format!(
                            "line {n}: expected `html` after the second `<--`, got `{other}`"
                        ));
                        continue;
                    }
                    _ => {
                        errors.push(format!("line {n}: expected `md NAME <-- FILE [<-- html]`"));
                        continue;
                    }
                };
                if !is_ident(name) {
                    errors.push(format!("line {n}: `{name}` is not a valid JS identifier"));
                    continue;
                }
                if path.is_empty() {
                    errors.push(format!("line {n}: missing markdown file path"));
                    continue;
                }
                if !seen.insert(name.to_string()) {
                    errors.push(format!("line {n}: duplicate name `{name}`"));
                    continue;
                }
                let source = match resolve(dir, path, n) {
                    Ok(p) => p,
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                };
                entries.push(Entry::Md {
                    name: name.into(),
                    source,
                    html,
                });
            }
            "var" | "let" => {
                let Some((name, rhs)) = rest.split_once('=') else {
                    errors.push(format!("line {n}: missing `=`"));
                    continue;
                };
                let (name, rhs) = (name.trim(), rhs.trim());
                // `let X == 1` / `let X => 1` split at the first `=` and would
                // otherwise emit invalid JS silently
                if rhs.starts_with('=') || rhs.starts_with('>') {
                    errors.push(format!("line {n}: expected a single `=` after `{name}`"));
                    continue;
                }
                if !is_ident(name) {
                    errors.push(format!("line {n}: `{name}` is not a valid JS identifier"));
                    continue;
                }
                if !seen.insert(name.to_string()) {
                    errors.push(format!("line {n}: duplicate name `{name}`"));
                    continue;
                }
                if word == "let" {
                    if rhs.is_empty() {
                        errors.push(format!("line {n}: empty expression for `{name}`"));
                        continue;
                    }
                    entries.push(Entry::Let {
                        name: name.into(),
                        expr: rhs.into(),
                    });
                } else {
                    // optional `--- <spec> [tags]` pin, then split on the LAST
                    // `->` so paths containing `->` still work
                    let (rhs, pin) = take_version_suffix(rhs);
                    let ver = match parse_pin(pin, n) {
                        Ok(v) => v,
                        Err(e) => {
                            errors.push(e);
                            continue;
                        }
                    };
                    let Some(pos) = rhs.rfind("->") else {
                        errors.push(format!("line {n}: missing `-> const NAME`"));
                        continue;
                    };
                    let path = rhs[..pos].trim();
                    let decl = rhs[pos + 2..].trim();
                    let decl_name = decl.strip_prefix("export ").unwrap_or(decl);
                    let decl_name = match decl_name.strip_prefix("const ") {
                        Some(d) if is_ident(d.trim()) => d.trim(),
                        _ => {
                            errors.push(format!(
                                "line {n}: expected `const NAME` after `->`, got `{decl}`"
                            ));
                            continue;
                        }
                    };
                    if path.is_empty() {
                        errors.push(format!("line {n}: missing source path"));
                        continue;
                    }
                    let source = match resolve(dir, path, n) {
                        Ok(p) => p,
                        Err(e) => {
                            errors.push(e);
                            continue;
                        }
                    };
                    entries.push(Entry::Var {
                        name: name.into(),
                        source,
                        decl: decl_name.into(),
                        version: ver,
                    });
                }
            }
            _ => errors.push(format!("line {n}: unknown directive `{word}`")),
        }
    }

    let out = out.unwrap_or_else(|| dir.join("_wikiData.js"));
    let version = config_version(spec, tags);
    (
        Config {
            out,
            entries,
            version,
        },
        errors,
    )
}

/// One entry in a `--pull-all` merge config, in config (= emit) order.
#[derive(Debug, Clone, PartialEq)]
pub enum MergeEntry {
    /// `vars NAME <-- PROJECT --> FILE` — pull a project's already-generated
    /// module verbatim.
    Project {
        name: String,
        /// Resolved path to that project's generated `_wikiData.js`.
        source: PathBuf,
    },
    /// `vars NAME <-- PROJECT <-- CONFIG` — build the project fresh from its
    /// own `jsdata.cfg` (in memory), then group its exports. `version` is the
    /// optional `--- <spec>` pin for this project.
    ProjectFromConfig {
        name: String,
        /// Resolved path to that project's `jsdata.cfg`.
        config: PathBuf,
        version: Option<VersionSpec>,
    },
    /// `langs <-- TEXTS <-- languages.json` — read every locale's `.lang` file
    /// into a `{ locale: { key: value } }` object.
    Langs {
        name: String,
        texts_dir: PathBuf,
        languages_json: PathBuf,
        version: Option<VersionSpec>,
    },
}

impl MergeEntry {
    pub fn name(&self) -> &str {
        match self {
            MergeEntry::Project { name, .. }
            | MergeEntry::ProjectFromConfig { name, .. }
            | MergeEntry::Langs { name, .. } => name,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MergeConfig {
    pub out: PathBuf,
    pub entries: Vec<MergeEntry>,
    /// `version` + `versionTags` directives — merge-global defaults.
    pub version: Option<VersionSpec>,
}

/// Parse a `--pull` config: `out` plus `vars NAME <-- PROJECT --> FILE` and/or
/// `langs <-- TEXTS <-- languages.json` lines. Paths resolve relative to the
/// config file. Errors collected, not fail-fast, matching [`parse`].
pub fn parse_merge(text: &str, dir: &Path) -> (MergeConfig, Vec<String>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out: Option<PathBuf> = None;
    let mut spec: Option<String> = None;
    let mut tags: Option<Vec<String>> = None;
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for (idx, raw) in text.lines().enumerate() {
        let n = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if take_config_version(line, n, &mut spec, &mut tags, &mut errors) {
            continue;
        }
        let (word, rest) = match line.split_once(char::is_whitespace) {
            Some((w, r)) => (w, r.trim()),
            None => (line, ""),
        };
        match word {
            "out" => {
                if rest.is_empty() {
                    errors.push(format!("line {n}: `out` needs a path"));
                } else if out.is_some() {
                    errors.push(format!("line {n}: duplicate `out`"));
                } else {
                    match resolve(dir, rest, n) {
                        Ok(p) => out = Some(p),
                        Err(e) => errors.push(e),
                    }
                }
            }
            "vars" => {
                // NAME <-- PROJECT --> FILE   (pull an existing generated file)
                // NAME <-- PROJECT <-- CONFIG (build the project from its config)
                // optional trailing `--- <spec>` pins this project's version
                let (rest, pin_raw) = take_version_suffix(rest);
                let pin = match parse_pin(pin_raw, n) {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                };
                let Some((name, tail)) = rest.split_once("<--") else {
                    errors.push(format!(
                        "line {n}: expected `NAME <-- PROJECT --> FILE` or `NAME <-- PROJECT <-- CONFIG`"
                    ));
                    continue;
                };
                let name = name.trim();
                if !is_ident(name) {
                    errors.push(format!("line {n}: `{name}` is not a valid JS identifier"));
                    continue;
                }
                // an absolute project path overrides `dir` (std join semantics);
                // `-->` FILE pulls an existing module, `<--` CONFIG rebuilds it
                let entry = if let Some((proj, file)) = tail.split_once("-->") {
                    if pin.is_some() {
                        // a pre-generated file can't be re-filtered
                        errors.push(format!("line {n}: `--- <version>` needs the `<-- CONFIG` form; a `--> FILE` is already built"));
                        continue;
                    }
                    let (proj, file) = (proj.trim(), file.trim());
                    if proj.is_empty() || file.is_empty() {
                        errors.push(format!("line {n}: project path and wikiData file are both required"));
                        continue;
                    }
                    let source = match (resolve(dir, proj, n), expand_frag(file, n)) {
                        (Ok(p), Ok(f)) => p.join(f),
                        (Err(e), _) | (_, Err(e)) => {
                            errors.push(e);
                            continue;
                        }
                    };
                    MergeEntry::Project {
                        name: name.into(),
                        source,
                    }
                } else if let Some((proj, cfg)) = tail.split_once("<--") {
                    let (proj, cfg) = (proj.trim(), cfg.trim());
                    if proj.is_empty() || cfg.is_empty() {
                        errors.push(format!("line {n}: project path and config file are both required"));
                        continue;
                    }
                    let config = match (resolve(dir, proj, n), expand_frag(cfg, n)) {
                        (Ok(p), Ok(c)) => p.join(c),
                        (Err(e), _) | (_, Err(e)) => {
                            errors.push(e);
                            continue;
                        }
                    };
                    MergeEntry::ProjectFromConfig {
                        name: name.into(),
                        config,
                        version: pin,
                    }
                } else {
                    errors.push(format!("line {n}: missing `--> FILE` or `<-- CONFIG` after the project"));
                    continue;
                };
                if !seen.insert(name.to_string()) {
                    errors.push(format!("line {n}: duplicate name `{name}`"));
                    continue;
                }
                entries.push(entry);
            }
            "langs" => {
                let (rest, pin) = take_version_suffix(rest);
                let ver = match parse_pin(pin, n) {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                };
                match parse_langs_rhs(rest, dir, n, ver) {
                    Ok(Entry::Langs { texts_dir, languages_json, version, .. }) => {
                        if !seen.insert("langs".to_string()) {
                            errors.push(format!("line {n}: duplicate name `langs`"));
                        } else {
                            entries.push(MergeEntry::Langs {
                                name: "langs".into(),
                                texts_dir,
                                languages_json,
                                version,
                            });
                        }
                    }
                    Ok(_) => unreachable!("parse_langs_rhs only yields Entry::Langs"),
                    Err(e) => errors.push(e),
                }
            }
            _ => errors.push(format!(
                "line {n}: unknown directive `{word}` (pull config takes `out`, `version`, `vars`, `langs`)"
            )),
        }
    }

    let out = out.unwrap_or_else(|| dir.join("_wikiData.js"));
    let version = config_version(spec, tags);
    (
        MergeConfig {
            out,
            entries,
            version,
        },
        errors,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> (Config, Vec<String>) {
        parse(text, Path::new("base"))
    }

    #[test]
    fn spec_example() {
        let (cfg, errs) = p("# comment\nout src/wiki/_wikiData.js\n\nvar PASSIVES = src/BP/scripts/main.js -> const PASSIVES\nlet PASSIVE_COUNT = Object.keys(PASSIVES).length\n");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(cfg.out, Path::new("base/src/wiki/_wikiData.js"));
        assert_eq!(cfg.entries.len(), 2);
        assert_eq!(
            cfg.entries[0],
            Entry::Var {
                name: "PASSIVES".into(),
                source: "base/src/BP/scripts/main.js".into(),
                decl: "PASSIVES".into(),
                version: None,
            }
        );
        assert_eq!(
            cfg.entries[1],
            Entry::Let {
                name: "PASSIVE_COUNT".into(),
                expr: "Object.keys(PASSIVES).length".into(),
            }
        );
    }

    /// Env vars are process-global; give every test its own name.
    fn with_var<T>(name: &str, value: &str, f: impl FnOnce() -> T) -> T {
        std::env::set_var(name, value);
        let out = f();
        std::env::remove_var(name);
        out
    }

    #[test]
    fn every_path_directive_expands() {
        with_var("WD_CFG_BUILD", "/build/2.5.0", || {
            let (cfg, errs) = p(concat!(
                "out ${WD_CFG_MISSING:-wiki}/_wikiData.js\n",
                "var A = $WD_CFG_BUILD/BP/a.js -> const A\n",
                "md GUIDE <-- ${WD_CFG_BUILD}/BP/guide.md\n",
                "langs <-- %WD_CFG_BUILD%/RP/texts <-- $WD_CFG_BUILD/RP/texts/languages.json\n",
            ));
            assert!(errs.is_empty(), "{errs:?}");
            // `out` had no reference to a build → stays under the config's dir.
            assert_eq!(cfg.out, Path::new("base/wiki/_wikiData.js"));
            // An expanded *absolute* path overrides the config dir — this is how
            // a source reads the build tree while `out` stays in the project.
            match &cfg.entries[0] {
                Entry::Var { source, .. } => {
                    assert_eq!(source, Path::new("/build/2.5.0/BP/a.js"));
                    assert!(!source.starts_with("base"));
                }
                other => panic!("expected var, got {other:?}"),
            }
            match &cfg.entries[1] {
                Entry::Md { source, .. } => {
                    assert_eq!(source, Path::new("/build/2.5.0/BP/guide.md"))
                }
                other => panic!("expected md, got {other:?}"),
            }
            match &cfg.entries[2] {
                Entry::Langs {
                    texts_dir,
                    languages_json,
                    ..
                } => {
                    assert_eq!(texts_dir, Path::new("/build/2.5.0/RP/texts"));
                    assert_eq!(
                        languages_json,
                        Path::new("/build/2.5.0/RP/texts/languages.json")
                    );
                }
                other => panic!("expected langs, got {other:?}"),
            }
        });
    }

    #[test]
    fn version_and_version_tags_directives() {
        // Both `=` and bare forms, and both spellings of a tag list.
        for text in [
            "version = 2.4\nversionTags = [beta, ui]\n",
            "version 2.4\nversionTags [beta ui]\n",
            "version=2.4\nversionTags=[beta,ui]\n",
        ] {
            let (cfg, errs) = p(text);
            assert!(errs.is_empty(), "{text:?} -> {errs:?}");
            assert_eq!(
                cfg.version,
                Some(VersionSpec {
                    spec: Some("2.4".into()),
                    tags: Some(vec!["beta".into(), "ui".into()]),
                }),
                "{text:?}"
            );
        }
    }

    #[test]
    fn version_directives_are_independent() {
        // tags without a version
        let (cfg, errs) = p("versionTags = [beta]");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(
            cfg.version,
            Some(VersionSpec {
                spec: None,
                tags: Some(vec!["beta".into()]),
            })
        );
        // version without tags
        let (cfg, errs) = p("version = 2.1 2.3");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(cfg.version, Some(VersionSpec::from_spec("2.1 2.3")));
        // neither
        let (cfg, errs) = p("let A = 1");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(cfg.version, None);
    }

    #[test]
    fn version_tags_is_not_swallowed_by_version() {
        // `strip_directive` must require a boundary after the name.
        let (cfg, errs) = p("versionTags = [beta]\nversion = 2.4\n");
        assert!(errs.is_empty(), "{errs:?}");
        let v = cfg.version.unwrap();
        assert_eq!(v.spec.as_deref(), Some("2.4"));
        assert_eq!(v.tags, Some(vec!["beta".into()]));
    }

    #[test]
    fn version_directive_errors() {
        let (_, errs) = p("version = notaversion");
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].starts_with("line 1:"), "{}", errs[0]);

        let (_, errs) = p("version = 2.1\nversion = 2.2\n");
        assert!(
            errs.iter().any(|e| e.contains("duplicate `version`")),
            "{errs:?}"
        );

        let (_, errs) = p("versionTags = [a]\nversionTags = [b]\n");
        assert!(
            errs.iter().any(|e| e.contains("duplicate `versionTags`")),
            "{errs:?}"
        );
    }

    #[test]
    fn pins_carry_tags() {
        let (cfg, errs) = p(concat!(
            "var A = a.js -> const A --- 2.4\n",
            "var B = b.js -> const B --- 2.4 [beta]\n",
            "var C = c.js -> const C --- [beta,ui]\n",
            "var D = d.js -> const D --- 2.1 2.3 [beta]\n",
            "var E = e.js -> const E --- 2.4 []\n",
            "langs <-- t <-- t/l.json --- 2.4 [beta]\n",
        ));
        assert!(errs.is_empty(), "{errs:?}");
        let pin = |i: usize| match &cfg.entries[i] {
            Entry::Var { version, .. } | Entry::Langs { version, .. } => version.clone().unwrap(),
            other => panic!("expected a versionable entry, got {other:?}"),
        };
        assert_eq!(pin(0), VersionSpec::from_spec("2.4"));
        assert_eq!(pin(1).tags, Some(vec!["beta".into()]));
        assert_eq!(pin(2).spec, None); // tags-only pin inherits the version
        assert_eq!(pin(3).spec.as_deref(), Some("2.1 2.3"));
        assert_eq!(pin(4).tags, Some(vec![])); // explicit "no tags"
        assert_eq!(pin(5).tags, Some(vec!["beta".into()]));
    }

    #[test]
    fn a_bad_pin_is_reported_on_its_line() {
        let (cfg, errs) = p(concat!(
            "var A = a.js -> const A\n",
            "var B = b.js -> const B --- 1.2 1.3 1.4\n",
            "var C = c.js -> const C --- 2.4 [beta\n",
            "var D = d.js -> const D\n",
        ));
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert!(errs[0].starts_with("line 2:"), "{}", errs[0]);
        assert!(errs[1].starts_with("line 3:"), "{}", errs[1]);
        // collected, not fail-fast
        assert_eq!(cfg.entries.len(), 2);
    }

    #[test]
    fn a_relative_expansion_still_resolves_against_the_config_dir() {
        with_var("WD_CFG_REL", "generated", || {
            let (cfg, errs) = p("var A = $WD_CFG_REL/a.js -> const A");
            assert!(errs.is_empty(), "{errs:?}");
            match &cfg.entries[0] {
                Entry::Var { source, .. } => assert_eq!(source, Path::new("base/generated/a.js")),
                other => panic!("expected var, got {other:?}"),
            }
        });
    }

    #[test]
    fn an_unset_variable_errors_on_its_own_line() {
        let (cfg, errs) = p(concat!(
            "var A = ./src/a.js -> const A\n",
            "var B = $WD_CFG_NOPE/b.js -> const B\n",
            "md M <-- $WD_CFG_NOPE/m.md\n",
            "out $WD_CFG_NOPE/o.js\n",
            "var C = ./src/c.js -> const C\n",
        ));
        assert_eq!(errs.len(), 3, "{errs:?}");
        assert!(errs[0].starts_with("line 2:"), "{}", errs[0]);
        assert!(errs[0].contains("WD_CFG_NOPE"), "{}", errs[0]);
        assert!(errs[1].starts_with("line 3:"), "{}", errs[1]);
        assert!(errs[2].starts_with("line 4:"), "{}", errs[2]);
        // Collected, not fail-fast: the good entries survive.
        assert_eq!(cfg.entries.len(), 2);
        assert_eq!(cfg.entries[0].name(), "A");
        assert_eq!(cfg.entries[1].name(), "C");
    }

    #[test]
    fn merge_config_paths_expand() {
        with_var("WD_CFG_ROOT", "/projects", || {
            let (cfg, errs) = parse_merge(
                concat!(
                    "out ${WD_CFG_MISSING:-site}/all.js\n",
                    "vars bp <-- $WD_CFG_ROOT/betterPotions --> src/_wikiData.js\n",
                    "vars boss <-- $WD_CFG_ROOT/boss <-- jsdata.cfg\n",
                    "langs <-- $WD_CFG_ROOT/texts <-- $WD_CFG_ROOT/texts/languages.json\n",
                ),
                Path::new("base"),
            );
            assert!(errs.is_empty(), "{errs:?}");
            assert_eq!(cfg.out, Path::new("base/site/all.js"));
            match &cfg.entries[0] {
                MergeEntry::Project { source, .. } => {
                    assert_eq!(
                        source,
                        Path::new("/projects/betterPotions/src/_wikiData.js")
                    )
                }
                other => panic!("expected project, got {other:?}"),
            }
            match &cfg.entries[1] {
                MergeEntry::ProjectFromConfig { config, .. } => {
                    assert_eq!(config, Path::new("/projects/boss/jsdata.cfg"))
                }
                other => panic!("expected project-from-config, got {other:?}"),
            }
            match &cfg.entries[2] {
                MergeEntry::Langs { texts_dir, .. } => {
                    assert_eq!(texts_dir, Path::new("/projects/texts"))
                }
                other => panic!("expected langs, got {other:?}"),
            }
        });
    }

    #[test]
    fn merge_config_reports_an_unset_variable() {
        let (cfg, errs) = parse_merge("vars a <-- $WD_CFG_NOPE/p --> f.js", Path::new("base"));
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].starts_with("line 1:"), "{}", errs[0]);
        assert!(errs[0].contains("WD_CFG_NOPE"));
        assert!(cfg.entries.is_empty());
    }

    #[test]
    fn out_defaults_relative_to_config() {
        let (cfg, errs) = p("let A = 1");
        assert!(errs.is_empty());
        assert_eq!(cfg.out, Path::new("base/_wikiData.js"));
    }

    #[test]
    fn export_const_decl_and_different_names() {
        let (cfg, errs) = p("var X = a.js -> export const Y");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(
            cfg.entries[0],
            Entry::Var {
                name: "X".into(),
                source: "base/a.js".into(),
                decl: "Y".into(),
                version: None,
            }
        );
    }

    #[test]
    fn path_containing_arrow_splits_on_last() {
        let (cfg, errs) = p("var X = weird->dir/a.js -> const X");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(
            cfg.entries[0],
            Entry::Var {
                name: "X".into(),
                source: "base/weird->dir/a.js".into(),
                decl: "X".into(),
                version: None,
            }
        );
    }

    #[test]
    fn let_expr_may_contain_equals() {
        let (cfg, errs) = p("let X = a === b ? 1 : 2");
        assert!(errs.is_empty());
        assert_eq!(
            cfg.entries[0],
            Entry::Let {
                name: "X".into(),
                expr: "a === b ? 1 : 2".into(),
            }
        );
    }

    #[test]
    fn errors_collected_not_fail_fast() {
        let (cfg, errs) = p("bogus line\nvar 1BAD = a.js -> const A\nlet A = 1\nlet A = 2\nvar B = a.js\nlet C =\nlet OK = 5");
        assert_eq!(errs.len(), 5, "{errs:?}");
        assert!(errs[0].contains("unknown directive"));
        assert!(errs[1].contains("not a valid JS identifier"));
        assert!(errs[2].contains("duplicate name `A`"));
        assert!(errs[3].contains("missing `-> const NAME`"));
        assert!(errs[4].contains("empty expression"));
        assert_eq!(cfg.entries.len(), 2); // A and OK survived
    }

    #[test]
    fn duplicate_out_is_error() {
        let (_, errs) = p("out a.js\nout b.js");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("duplicate `out`"));
    }

    #[test]
    fn double_equals_and_arrow_are_errors() {
        let (_, errs) = p("let X == 1\nlet Y => 2");
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert!(errs.iter().all(|e| e.contains("single `=`")));
    }

    #[test]
    fn normal_config_accepts_langs() {
        let (cfg, errs) = p("var A = a.js -> const A\nlangs <-- texts <-- texts/languages.json");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(cfg.entries.len(), 2);
        match &cfg.entries[1] {
            Entry::Langs {
                name,
                texts_dir,
                languages_json,
                ..
            } => {
                assert_eq!(name, "langs");
                assert!(texts_dir.ends_with("texts"));
                assert!(languages_json.ends_with("texts/languages.json"));
            }
            _ => panic!("expected langs entry"),
        }
    }

    #[test]
    fn duplicate_langs_in_normal_config() {
        let (_, errs) = p("langs <-- t1 <-- j1\nlangs <-- t2 <-- j2");
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("duplicate name `langs`"));
    }

    #[test]
    fn md_directive_both_forms() {
        let (cfg, errs) = p("md GUIDE <-- docs/guide.md\nmd HELP <-- docs/help.md <-- html");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(
            cfg.entries[0],
            Entry::Md {
                name: "GUIDE".into(),
                source: "base/docs/guide.md".into(),
                html: false,
            }
        );
        assert_eq!(
            cfg.entries[1],
            Entry::Md {
                name: "HELP".into(),
                source: "base/docs/help.md".into(),
                html: true,
            }
        );
    }

    #[test]
    fn md_directive_errors() {
        let (_, errs) = p("md ONLYNAME\nmd A <-- f.md <-- pdf\nmd 1BAD <-- f.md");
        assert_eq!(errs.len(), 3, "{errs:?}");
        assert!(errs[0].contains("expected `md NAME <-- FILE"));
        assert!(errs[1].contains("expected `html`"));
        assert!(errs[2].contains("not a valid JS identifier"));
    }

    #[test]
    fn unicode_identifiers_accepted() {
        let (cfg, errs) = p("var NÄME = a.js -> const NÄME");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(cfg.entries[0].name(), "NÄME");
    }

    #[test]
    fn decl_must_be_const() {
        let (_, errs) = p("var X = a.js -> let X");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("expected `const NAME`"));
    }

    #[test]
    fn merge_config_parses_vars_and_langs_in_order() {
        let (cfg, errs) = parse_merge(
            "# merge\nout site/all.js\nvars betterPotions <-- bp --> src/_wikiData.js\nlangs <-- texts <-- texts/languages.json\nvars boss <-- ../boss --> out/_wikiData.js",
            Path::new("base"),
        );
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(cfg.out, Path::new("base/site/all.js"));
        assert_eq!(cfg.entries.len(), 3);
        assert_eq!(cfg.entries[0].name(), "betterPotions");
        assert_eq!(cfg.entries[1].name(), "langs"); // config order preserved
        assert_eq!(cfg.entries[2].name(), "boss");
        match &cfg.entries[0] {
            MergeEntry::Project { source, .. } => {
                assert!(source.ends_with("src/_wikiData.js"));
                assert!(source.starts_with("base"));
            }
            _ => panic!("expected project"),
        }
        match &cfg.entries[1] {
            MergeEntry::Langs {
                texts_dir,
                languages_json,
                ..
            } => {
                assert!(texts_dir.ends_with("texts"));
                assert!(languages_json.ends_with("texts/languages.json"));
            }
            _ => panic!("expected langs"),
        }
    }

    #[test]
    fn merge_config_vars_from_config_form() {
        let (cfg, errs) = parse_merge(
            "vars betterPotions <-- W:/Projects/betterPotions <-- jsdata.cfg\nvars boss <-- boss --> out/_wikiData.js",
            Path::new("base"),
        );
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(cfg.entries.len(), 2);
        match &cfg.entries[0] {
            MergeEntry::ProjectFromConfig { name, config, .. } => {
                assert_eq!(name, "betterPotions");
                assert!(config.ends_with("jsdata.cfg"));
            }
            other => panic!("expected ProjectFromConfig, got {other:?}"),
        }
        // the `-->` form is still a plain Project
        assert!(matches!(cfg.entries[1], MergeEntry::Project { .. }));
    }

    #[test]
    fn merge_config_vars_missing_second_arrow() {
        let (_, errs) = parse_merge("vars A <-- proj", Path::new("b"));
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("missing `--> FILE` or `<-- CONFIG`"));
    }

    #[test]
    fn merge_config_errors_collected() {
        let (cfg, errs) = parse_merge(
            "vars A <-- p --> f\nvars A <-- q --> g\nvars 1B <-- p --> f\nvars C p f\nvars D <-- p\nlangs texts lj\nlet X = 1",
            Path::new("b"),
        );
        assert_eq!(errs.len(), 6, "{errs:?}");
        assert!(errs[0].contains("duplicate name `A`"));
        assert!(errs[1].contains("not a valid JS identifier"));
        assert!(errs[2].contains("expected `NAME <-- PROJECT --> FILE`"));
        assert!(errs[3].contains("missing `--> FILE` or `<-- CONFIG`"));
        assert!(errs[4].contains("expected `langs <-- TEXTS"));
        assert!(errs[5].contains("unknown directive"));
        assert_eq!(cfg.entries.len(), 1); // only the first `A`
    }

    #[test]
    fn merge_config_duplicate_langs() {
        let (_, errs) = parse_merge("langs <-- t --> j\nlangs <-- t2 <-- j2", Path::new("b"));
        // first line has no second `<--` (uses `-->`), second is a dup only if first parsed;
        // here first errors on missing second `<--`, second succeeds
        assert!(
            errs.iter().any(|e| e.contains("missing second `<--`")),
            "{errs:?}"
        );
    }
}
