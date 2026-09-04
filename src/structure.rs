//! `--structure`: print what a config yields, without writing anything.
//!
//! Two questions it answers that the generated module can't:
//!
//! - **Shape.** What is each entry, roughly? A number reads as its value; an
//!   object or array as how many entries it holds; `langs` as locales × keys.
//! - **Versions.** When a `const` is declared several times behind version
//!   markers, which versions exist and which one *this* run selects. That's
//!   invisible in the output module, where only the winner survives.

use std::fmt::Write as _;
use std::path::Path;

use crate::config::Entry;
use crate::version::VersionSpec;
use crate::{effective_version, entry_version, lang, pick, version, Options};

/// A one-line description of a value: kind plus a size or the value itself.
pub fn summarize(value: &str) -> String {
    let v = value.trim();
    let mut chars = v.chars();
    match chars.next() {
        Some('{') => match count_entries(v) {
            Some(n) => format!("object, {n} {}", plural(n, "entry", "entries")),
            None => "object".to_string(),
        },
        Some('[') => match count_entries(v) {
            Some(n) => format!("array, {n} {}", plural(n, "item", "items")),
            None => "array".to_string(),
        },
        Some('"') | Some('\'') | Some('`') => {
            let body = v.len().saturating_sub(2);
            let lines = v.lines().count();
            if lines > 1 {
                format!("string, {lines} lines")
            } else {
                format!("string, {body} chars")
            }
        }
        _ if v == "true" || v == "false" => format!("boolean {v}"),
        _ if v.parse::<f64>().is_ok() => format!("number {v}"),
        _ => {
            // An expression, a call, a reference — show a clipped form.
            let one = v.split_whitespace().collect::<Vec<_>>().join(" ");
            format!("expression {}", clip(&one, 40))
        }
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { one } else { many }.to_string()
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Count top-level members of an object or array literal.
///
/// Counts **members**, not commas: the first content after each depth-1 comma
/// starts one. Counting commas + 1 would report a phantom entry for the trailing
/// comma that most of these literals end with. Strings, templates and comments
/// are skipped so punctuation inside them never counts. `None` if the literal
/// never closes.
fn count_entries(v: &str) -> Option<usize> {
    let b = v.as_bytes();
    let mut depth = 0usize;
    let mut members = 0usize;
    let mut in_member = false;
    let mut i = 0usize;
    // A member begins at the first non-space content after a depth-1 comma.
    macro_rules! begin_member {
        () => {
            if depth == 1 && !in_member {
                members += 1;
                in_member = true;
            }
        };
    }
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' | b'`' => {
                begin_member!();
                i = skip_quoted(b, i)?;
                continue;
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                i = find(b, i + 2, b"*/")? + 2;
                continue;
            }
            b'{' | b'[' | b'(' => {
                // An object/array *as* a member (e.g. `[{...}, {...}]`).
                begin_member!();
                depth += 1;
            }
            b'}' | b']' | b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(members); // closed the outer literal
                }
            }
            b',' if depth == 1 => in_member = false,
            c if depth == 1 && !c.is_ascii_whitespace() => begin_member!(),
            _ => {}
        }
        i += 1;
    }
    None
}

/// `i` is at the opening quote; returns the index just past the closing one.
fn skip_quoted(b: &[u8], start: usize) -> Option<usize> {
    let quote = b[start];
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            c if c == quote => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

fn find(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= b.len() {
        return None;
    }
    b[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

/// Render the report for one config.
pub fn report(config_path: &Path, opts: &Options, merge: bool) -> Result<String, String> {
    if merge {
        return report_merge(config_path, opts);
    }
    let (cfg, errors) = crate::load(config_path)?;
    let mut out = String::new();
    let _ = writeln!(out, "{}", config_path.display());
    if let Some(v) = &cfg.version {
        let _ = writeln!(out, "  config default: {}", describe(v));
    }
    if !opts.version.is_empty() {
        let _ = writeln!(out, "  command line:   {}", describe(&opts.version));
    }
    let _ = writeln!(out);

    for entry in &cfg.entries {
        let eff = effective_version(&[
            entry_version(entry),
            Some(&opts.version),
            cfg.version.as_ref(),
        ]);
        let _ = write!(out, "{}", render_entry(entry, eff.as_ref()));
    }

    if !errors.is_empty() {
        let _ = writeln!(out, "\n{} config error(s):", errors.len());
        for e in &errors {
            let _ = writeln!(out, "  {e}");
        }
    }
    Ok(out)
}

fn report_merge(config_path: &Path, opts: &Options) -> Result<String, String> {
    let text = std::fs::read_to_string(config_path)
        .map_err(|e| format!("cannot read {}: {e}", config_path.display()))?;
    let dir = config_path.parent().unwrap_or(Path::new(""));
    let (cfg, errors) = crate::config::parse_merge(&text, dir);

    let mut out = String::new();
    let _ = writeln!(out, "{} (merge)", config_path.display());
    let _ = writeln!(out);
    for entry in &cfg.entries {
        match entry {
            crate::config::MergeEntry::ProjectFromConfig {
                name,
                config,
                version: pin,
            } => {
                let eff =
                    effective_version(&[pin.as_ref(), Some(&opts.version), cfg.version.as_ref()]);
                let at = eff.as_ref().map(describe).unwrap_or("unfiltered".into());
                let _ = writeln!(out, "{name}  <-- {}  [{at}]", config.display());
                match crate::load(config) {
                    Ok((sub, _)) => {
                        for e in &sub.entries {
                            let inner = effective_version(&[
                                entry_version(e),
                                eff.as_ref(),
                                sub.version.as_ref(),
                            ]);
                            let _ = write!(out, "{}", indent(&render_entry(e, inner.as_ref())));
                        }
                    }
                    Err(e) => {
                        let _ = writeln!(out, "    error: {e}");
                    }
                }
                let _ = writeln!(out);
            }
            other => {
                let _ = writeln!(out, "{}  (pre-built)", other.name());
            }
        }
    }
    if !errors.is_empty() {
        let _ = writeln!(out, "{} config error(s):", errors.len());
        for e in &errors {
            let _ = writeln!(out, "  {e}");
        }
    }
    Ok(out)
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("  {l}\n"))
        .collect::<Vec<_>>()
        .join("")
}

fn describe(v: &VersionSpec) -> String {
    match (&v.spec, &v.tags) {
        (Some(s), Some(t)) if !t.is_empty() => format!("{s} [{}]", t.join(",")),
        (Some(s), _) => s.clone(),
        (None, Some(t)) if !t.is_empty() => format!("[{}]", t.join(",")),
        _ => "unfiltered".to_string(),
    }
}

/// One row of a declaration's version timeline.
struct Row {
    /// The version this row starts at (`base` for the unversioned region).
    at: String,
    summary: String,
    selected: bool,
}

/// How a declaration's value changes across versions.
///
/// Version points come from **every** marker in the file — the ones around a
/// declaration (duplicate declarations, one per version) and the ones *inside*
/// it (a list that grows an entry, without duplicating the whole literal). Each
/// point is evaluated through the ordinary extraction path, then consecutive
/// points with an identical value collapse, so only real changes show.
///
/// This is reporting only: extraction already yields the right value by
/// filtering at the effective version.
fn timeline(raw: &str, source: &Path, decl: &str, eff: Option<&VersionSpec>) -> Vec<Row> {
    let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("");
    let tags = eff.and_then(|v| v.tags.clone());
    let at_version = |spec: Option<&str>| -> Option<String> {
        let probe = spec.map(|s| VersionSpec {
            spec: Some(s.to_string()),
            tags: tags.clone(),
        });
        crate::extract_declaration_for_report(raw, source, decl, probe.as_ref())
    };

    // `base` first, then each marker version in ascending order.
    let mut points: Vec<Option<String>> = vec![Some(BASE.to_string())];
    points.extend(
        pick::version_points(raw, version::style_for(ext))
            .into_iter()
            .map(Some),
    );

    let mut rows: Vec<Row> = Vec::new();
    let mut previous: Option<Option<String>> = None;
    for point in &points {
        let value = at_version(point.as_deref());
        if previous.as_ref() == Some(&value) {
            continue; // unchanged since the last point — not a new version
        }
        previous = Some(value.clone());
        let at = match point.as_deref() {
            Some(BASE) | None => "base".to_string(),
            Some(p) => p.to_string(),
        };
        rows.push(Row {
            at,
            summary: value
                .as_deref()
                .map(summarize)
                .unwrap_or_else(|| "not present".into()),
            selected: false,
        });
    }

    // Mark the row in force at the effective version: the last point at or
    // below it, or the newest row when nothing is pinned.
    let effective = eff.and_then(|v| v.spec.clone());
    let selected = match &effective {
        None => rows.len().checked_sub(1),
        Some(spec) => {
            let upper = spec.split_whitespace().last().unwrap_or(spec).to_string();
            rows.iter().rposition(|r| match (&r.at[..], &upper[..]) {
                ("base", _) => true,
                (a, b) => match (
                    vertion::filter::parse_version(a),
                    vertion::filter::parse_version(b),
                ) {
                    (Ok(x), Ok(y)) => x <= y,
                    _ => false,
                },
            })
        }
    };
    if let Some(i) = selected {
        rows[i].selected = true;
    }
    rows
}

/// The pseudo-version standing for "before any marker" — low enough that every
/// real block is excluded, so only unversioned content survives.
const BASE: &str = "0.0.0";

/// One entry's lines: a summary, plus a version breakdown when the value
/// changes across versions.
fn render_entry(entry: &Entry, eff: Option<&VersionSpec>) -> String {
    let mut out = String::new();
    let name = entry.name();
    match entry {
        Entry::Let { expr, .. } => {
            let _ = writeln!(out, "{name:<22}  {}", summarize(expr));
        }
        Entry::Md { source, html, .. } => {
            let kind = if *html { "html" } else { "markdown" };
            match std::fs::read_to_string(source) {
                Ok(t) => {
                    let _ = writeln!(out, "{name:<22}  {kind}, {} lines", t.lines().count());
                }
                Err(e) => {
                    let _ = writeln!(out, "{name:<22}  {kind}, unreadable ({e})");
                }
            }
        }
        Entry::Langs {
            texts_dir,
            languages_json,
            ..
        } => {
            let summary = match crate::resolve_langs(texts_dir, languages_json, eff) {
                Ok((js, _)) => {
                    let locales = count_entries(&js).unwrap_or(0);
                    let keys = lang::parse_languages_json(
                        &std::fs::read_to_string(languages_json).unwrap_or_default(),
                    )
                    .first()
                    .map(|loc| {
                        std::fs::read_to_string(texts_dir.join(format!("{loc}.lang")))
                            .map(|t| lang::parse_lang(&t).len())
                            .unwrap_or(0)
                    })
                    .unwrap_or(0);
                    format!("langs, {locales} locales, {keys} keys")
                }
                Err(e) => format!("langs, unreadable ({e})"),
            };
            let _ = writeln!(out, "{name:<22}  {summary}");
        }
        Entry::Var { source, decl, .. } => {
            let raw = match std::fs::read_to_string(source) {
                Ok(t) => t,
                Err(e) => {
                    let _ = writeln!(out, "{name:<22}  unreadable ({e})");
                    return out;
                }
            };
            let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("");
            let rows = timeline(&raw, source, decl, eff);
            match rows.len() {
                0 => {
                    let _ = writeln!(out, "{name:<22}  `const {decl}` not found");
                }
                1 => {
                    let _ = writeln!(out, "{name:<22}  {}", rows[0].summary);
                }
                n => {
                    let _ = writeln!(out, "{name:<22}  {n} versions");
                    for r in &rows {
                        let mark = if r.selected { "->" } else { "  " };
                        let _ = writeln!(out, "  {mark} {:<18}  {}", r.at, r.summary);
                    }
                }
            }
            let _ = ext;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_scalars() {
        assert_eq!(summarize("1.5"), "number 1.5");
        assert_eq!(summarize("42"), "number 42");
        assert_eq!(summarize("true"), "boolean true");
        assert_eq!(summarize("\"hello\""), "string, 5 chars");
    }

    #[test]
    fn counts_object_and_array_members() {
        assert_eq!(summarize("{ a: 1, b: 2, c: 3 }"), "object, 3 entries");
        assert_eq!(summarize("{ a: 1 }"), "object, 1 entry");
        assert_eq!(summarize("{}"), "object, 0 entries");
        assert_eq!(summarize("[1, 2, 3, 4]"), "array, 4 items");
        assert_eq!(summarize("[]"), "array, 0 items");
    }

    #[test]
    fn nested_commas_do_not_inflate_the_count() {
        assert_eq!(
            summarize("{ a: { x: 1, y: 2 }, b: [1, 2, 3] }"),
            "object, 2 entries"
        );
    }

    #[test]
    fn commas_in_strings_and_comments_are_ignored() {
        assert_eq!(summarize("{ a: \"x,y,z\", b: 2 }"), "object, 2 entries");
        assert_eq!(summarize("{ a: 1, // b, c\n  d: 2 }"), "object, 2 entries");
        assert_eq!(summarize("{ a: `x,${y},z`, b: 2 }"), "object, 2 entries");
        assert_eq!(summarize("{ a: 1, /* b, c */ d: 2 }"), "object, 2 entries");
    }

    #[test]
    fn a_trailing_comma_does_not_add_a_phantom_entry() {
        // `{ a: 1, b: 2, }` is 2 entries, not 3.
        assert_eq!(summarize("{ a: 1, b: 2, }"), "object, 2 entries");
    }

    #[test]
    fn expressions_are_clipped() {
        let s = summarize("Object.keys(PASSIVES).length");
        assert!(s.starts_with("expression Object.keys"), "{s}");
        assert!(summarize(&"x".repeat(80)).ends_with('…'));
    }

    #[test]
    fn an_unclosed_literal_degrades_to_the_bare_kind() {
        assert_eq!(summarize("{ a: 1"), "object");
    }
}
