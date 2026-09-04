//! Version + tag filtering for the `version` / `versionTags` directives, the
//! `--- <spec> [tags]` per-directive pin, and the `--version` / `--tag` flags.
//! Reuses [Vertion]'s marker filter verbatim — jsdata never parses a version
//! marker itself, and tag matching is Vertion's (`tag_passes`): tags are
//! **opt-in**, so an **empty** filter activates no tags and every *tagged*
//! block is skipped, while an **untagged** block passes any filter. `*` is the
//! wildcard that admits every tag.
//!
//! [Vertion]: https://github.com/vertX-dev/vertion
//!
//! `spec` is Vertion's syntax, restricted here to plain and range forms:
//!   "2.1"        cumulative (base + everything <= 2.1)
//!   "2.1 2.3"    range      (base + 2.1..=2.3)

use vertion::config::{detect_comment_style, CommentStyle};
use vertion::filter::parse_filter;
use vertion::parser::{process_file, ProcessOptions};

/// Stand-in upper bound for a tags-only filter. Vertion's `parse_filter` takes
/// versions only — there is no "all versions" token — so a ceiling nothing will
/// ever reach plays that role.
const UNBOUNDED: &str = "999999.0.0";

/// A version filter: the spec, plus the tags applied alongside it.
///
/// The two halves resolve **independently** (see `lib::effective_version`), so a
/// `--- 2.4` pin that names no tags still inherits `--tag` / `versionTags`.
/// Write `--- 2.4 []` to mean "this entry, with no tags active" — which skips
/// every tagged block — or `--- 2.4 [*]` to admit them all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionSpec {
    /// `None` = inherit the version from the next level down; the tags may still
    /// be set (a tags-only pin, `--- [beta]`).
    pub spec: Option<String>,
    /// `None` = inherit tags; `Some(vec![])` = explicitly no tags active, which
    /// skips every tagged block. `Some(vec!["*"])` admits them all.
    pub tags: Option<Vec<String>>,
}

impl VersionSpec {
    /// A spec with no tag override — the shape every pre-tag caller wants.
    pub fn from_spec(spec: impl Into<String>) -> VersionSpec {
        VersionSpec {
            spec: Some(spec.into()),
            tags: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.spec.is_none() && self.tags.is_none()
    }

    /// Parse the text after ` --- ` on a directive, or the RHS of a `version`
    /// directive: `<spec>`, `<spec> [tags]`, or `[tags]` alone.
    ///
    /// Tags are comma- and/or whitespace-separated inside the brackets, matching
    /// Vertion's `[tag1,tag2]` marker syntax.
    pub fn parse(text: &str) -> Result<VersionSpec, String> {
        let text = text.trim();
        let (spec_part, tags) = match text.rfind('[') {
            Some(open) => {
                let rest = &text[open..];
                let close = rest
                    .find(']')
                    .ok_or_else(|| format!("`{text}`: unclosed `[` in the tag list"))?;
                if rest[close + 1..].trim() != "" {
                    return Err(format!("`{text}`: trailing text after the tag list"));
                }
                (&text[..open], Some(parse_tags(&rest[1..close])))
            }
            None => (text, None),
        };
        let spec_part = spec_part.trim();
        let spec = if spec_part.is_empty() {
            None
        } else {
            validate_spec(spec_part)?;
            Some(spec_part.to_string())
        };
        if spec.is_none() && tags.is_none() {
            return Err("empty version spec".to_string());
        }
        Ok(VersionSpec { spec, tags })
    }
}

/// Split a bracketed tag list. `[]` yields an empty vec — an explicit "no tags".
fn parse_tags(inner: &str) -> Vec<String> {
    inner
        .split([',', ' ', '\t'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

/// Reject a spec early, so a typo surfaces where it was written rather than as a
/// silent no-op at extraction time.
fn validate_spec(spec: &str) -> Result<(), String> {
    let tokens: Vec<String> = spec.split_whitespace().map(String::from).collect();
    if tokens.len() > 2 {
        return Err(format!(
            "version `{spec}`: use `x.y` or `x.y x1.y1` (plain or range)"
        ));
    }
    parse_filter(&tokens).map_err(|e| format!("version `{spec}`: {e}"))?;
    Ok(())
}

/// Comment style for a source path. `.lang` uses `#`; everything else defers
/// to Vertion's extension table (JS/TS → `//`).
pub fn style_for(ext: &str) -> CommentStyle {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "lang" => CommentStyle::Hash,
        e => detect_comment_style(e),
    }
}

/// The `(FilterMode, tag_filter)` pair a [`VersionSpec`] denotes — the same
/// values [`filter_source`] uses, exposed so the duplicate picker can evaluate
/// markers itself.
pub fn filter_parts(version: &VersionSpec) -> Result<(vertion::filter::FilterMode, Vec<String>), String> {
    let tokens: Vec<String> = match &version.spec {
        Some(s) => s.split_whitespace().map(String::from).collect(),
        None => vec![UNBOUNDED.to_string()],
    };
    if tokens.len() > 2 {
        let s = version.spec.as_deref().unwrap_or_default();
        return Err(format!(
            "version `{s}`: use `x.y` or `x.y x1.y1` (plain or range)"
        ));
    }
    let mode = parse_filter(&tokens).map_err(|e| {
        let s = version.spec.as_deref().unwrap_or_default();
        format!("version `{s}`: {e}")
    })?;
    Ok((mode, version.tags.clone().unwrap_or_default()))
}

/// Strip version blocks that don't pass `version` from `source`, using the
/// comment style for `ext`. A `VersionSpec` with no `spec` filters on tags
/// alone, at the "everything" version.
pub fn filter_source(source: &str, ext: &str, version: &VersionSpec) -> Result<String, String> {
    // Tags-only (`--- [beta]`, or `versionTags` with no `version`) filters on
    // tags but bounds nothing on version — see UNBOUNDED.
    let (mode, tag_filter) = filter_parts(version)?;
    let lines: Vec<String> = source.lines().map(String::from).collect();
    let opts = ProcessOptions {
        tag_filter: &tag_filter,
        ..ProcessOptions::default()
    };
    let res = process_file(&lines, style_for(ext), &mode, opts);
    let mut out = res.lines.join("\n");
    if source.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str =
        "export const P = {\n  base: 1,\n  //version 2.1 *\n  next: 2,\n  //version 2.1 *\n};\n";
    const TAGGED: &str = "export const P = {\n  base: 1,\n  //version 2.1 [beta]*\n  beta: 2,\n  //version 2.1 [beta]*\n  //version 2.1 [ui]*\n  ui: 3,\n  //version 2.1 [ui]*\n};\n";

    fn spec(s: &str) -> VersionSpec {
        VersionSpec::from_spec(s)
    }

    #[test]
    fn cumulative_below_strips_the_block() {
        let out = filter_source(SRC, "js", &spec("2.0")).unwrap();
        assert!(out.contains("base: 1"));
        assert!(!out.contains("next: 2"), "{out}");
    }

    #[test]
    fn cumulative_at_or_above_keeps_it() {
        let out = filter_source(SRC, "js", &spec("2.1")).unwrap();
        assert!(out.contains("next: 2"), "{out}");
    }

    #[test]
    fn range_form() {
        let out = filter_source(SRC, "js", &spec("2.1 2.3")).unwrap();
        assert!(out.contains("next: 2"), "{out}");
    }

    #[test]
    fn lang_uses_hash_style() {
        let src = "a=1\n#version 2.1 *\nb=2\n#version 2.1 *\n";
        assert!(!filter_source(src, "lang", &spec("2.0"))
            .unwrap()
            .contains("b=2"));
        assert!(filter_source(src, "lang", &spec("2.1"))
            .unwrap()
            .contains("b=2"));
    }

    #[test]
    fn bad_spec_is_loud() {
        assert!(filter_source(SRC, "js", &spec("1.2 1.3 1.4")).is_err());
        assert!(filter_source(SRC, "js", &spec("notaversion")).is_err());
    }

    #[test]
    fn tags_select_blocks_vertion_style() {
        // Tags are opt-in (Vertion's `tag_passes`): no filter activates no
        // tags, so every *tagged* block is skipped — untagged content stays.
        let none = filter_source(TAGGED, "js", &spec("2.1")).unwrap();
        assert!(none.contains("base: 1"), "{none}");
        assert!(!none.contains("beta: 2") && !none.contains("ui: 3"), "{none}");

        // `*` is the wildcard that admits every tag.
        let all = filter_source(
            TAGGED,
            "js",
            &VersionSpec {
                spec: Some("2.1".into()),
                tags: Some(vec!["*".into()]),
            },
        )
        .unwrap();
        assert!(all.contains("beta: 2") && all.contains("ui: 3"), "{all}");

        // A filter keeps matching tags and drops the rest.
        let only_beta = filter_source(
            TAGGED,
            "js",
            &VersionSpec {
                spec: Some("2.1".into()),
                tags: Some(vec!["beta".into()]),
            },
        )
        .unwrap();
        assert!(only_beta.contains("beta: 2"), "{only_beta}");
        assert!(!only_beta.contains("ui: 3"), "{only_beta}");
        // …and untagged content always survives.
        assert!(only_beta.contains("base: 1"), "{only_beta}");
    }

    #[test]
    fn a_tags_only_spec_does_not_bound_the_version() {
        let v = VersionSpec {
            spec: None,
            tags: Some(vec!["beta".into()]),
        };
        let out = filter_source(TAGGED, "js", &v).unwrap();
        assert!(out.contains("beta: 2"), "{out}");
        assert!(!out.contains("ui: 3"), "{out}");
    }

    #[test]
    fn parse_accepts_every_pin_shape() {
        assert_eq!(VersionSpec::parse("2.4").unwrap(), spec("2.4"));
        assert_eq!(VersionSpec::parse("2.1 2.3").unwrap(), spec("2.1 2.3"));
        assert_eq!(
            VersionSpec::parse("2.4 [beta,ui]").unwrap(),
            VersionSpec {
                spec: Some("2.4".into()),
                tags: Some(vec!["beta".into(), "ui".into()]),
            }
        );
        // whitespace-separated tags, matching Vertion's marker leniency
        assert_eq!(
            VersionSpec::parse("2.4 [beta ui]").unwrap().tags,
            Some(vec!["beta".into(), "ui".into()])
        );
        // tags only — inherits the version
        assert_eq!(
            VersionSpec::parse("[beta]").unwrap(),
            VersionSpec {
                spec: None,
                tags: Some(vec!["beta".into()]),
            }
        );
        // explicit "no tags"
        assert_eq!(
            VersionSpec::parse("2.4 []").unwrap().tags,
            Some(Vec::<String>::new())
        );
    }

    #[test]
    fn parse_rejects_junk() {
        assert!(VersionSpec::parse("").is_err());
        assert!(VersionSpec::parse("2.4 [beta").is_err());
        assert!(VersionSpec::parse("2.4 [beta] junk").is_err());
        assert!(VersionSpec::parse("1.2 1.3 1.4").is_err());
        assert!(VersionSpec::parse("notaversion").is_err());
    }
}
