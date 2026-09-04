//! Choosing between **duplicate declarations** of the same `const`.
//!
//! A source file may carry several versions of one declaration, each gated by a
//! [Vertion] marker block:
//!
//! ```js
//! //version 1.0 2.0 *
//! export const CRAFT_COST = { legacy: true };
//! //version 1.0 2.0 *
//! //version 2.0 *
//! export const CRAFT_COST = { reworked: true };
//! //version 2.0 *
//! ```
//!
//! Extraction alone can't choose between them: the filter **removes marker
//! lines**, so by the time text reaches the extractor the blocks are anonymous
//! and the first match wins — which is the *oldest*. This module reads the
//! **unfiltered** source, works out which declarations are valid at the target
//! version, and picks the newest of those; the caller then takes that one out of
//! the filtered text.
//!
//! [Vertion]: https://github.com/vertX-dev/vertion

use vertion::config::CommentStyle;
use vertion::filter::{parse_version, FilterMode};
use vertion::parser::{detect_marker, Marker, MarkerKind};

use crate::extract;

/// One declaration of a name, and the version it becomes available at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Position among all declarations of this name, in source order.
    pub index: usize,
    /// Highest version among the enclosing marker blocks. `None` = the base
    /// (unversioned) region, which counts as the oldest.
    pub since: Option<String>,
    /// Whether every enclosing block passes the filter (Vertion's ancestor rule).
    pub passes: bool,
    /// The verbatim initializer, for reporting.
    pub value: String,
}

impl Candidate {
    /// How this candidate's version reads in a report.
    pub fn label(&self) -> &str {
        self.since.as_deref().unwrap_or("base")
    }
}

/// Every declaration of `name` in `src`, with the version each is gated at and
/// whether it survives the filter.
///
/// `filter` must be **the same filter that produced the text the caller will
/// index into**. `None` means no filtering at all, so every declaration passes
/// — not "an unbounded version", which would wrongly reject range blocks like
/// `//version 1.0 2.0 *` (no upper bound can sit inside them).
pub fn candidates(
    src: &str,
    name: &str,
    style: CommentStyle,
    filter: Option<(&FilterMode, &[String])>,
) -> Result<Vec<Candidate>, String> {
    let decls = extract::scan_positions(src)?;
    let mine: Vec<_> = decls.iter().filter(|d| d.name == name).collect();
    if mine.is_empty() {
        return Ok(Vec::new());
    }
    let stacks = marker_stacks(src, style);
    let mut out = Vec::with_capacity(mine.len());
    for (index, decl) in mine.into_iter().enumerate() {
        let stack = stacks
            .get(line_of(src, decl.offset))
            .cloned()
            .unwrap_or_default();
        // Ancestor rule: every enclosing block must pass on its own. A range
        // block (`//version 1.0 2.0 *`) has its own predicate — treating it as a
        // plain `1.0` marker would keep it alive past its upper bound.
        let passes = match filter {
            None => true,
            Some((mode, tag_filter)) => stack.iter().all(|m| match &m.to {
                Some(to) => {
                    vertion::filter::passes_range_marker(&m.version, to, &m.tags, mode, tag_filter)
                }
                None => vertion::filter::passes(&m.version, &m.tags, mode, tag_filter),
            }),
        };
        // "Newest" = highest version in the enclosing chain. A tag-only marker
        // carries no version, so it never makes a block newer.
        let since = stack
            .iter()
            .filter(|m| !m.is_tag_only())
            .filter_map(|m| {
                parse_version(&m.version)
                    .ok()
                    .map(|v| (v, m.version.clone()))
            })
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, raw)| raw);
        out.push(Candidate {
            index,
            since,
            passes,
            value: decl.value.clone(),
        });
    }
    Ok(out)
}

/// Which surviving declaration to take: the newest one that passes.
///
/// Returns the winner's rank **among the passing candidates**, because that is
/// what indexes into the filtered text — the filter deleted the others, and
/// preserves the order of what's left. `None` means nothing passes.
pub fn choose(candidates: &[Candidate]) -> Option<usize> {
    let passing: Vec<&Candidate> = candidates.iter().filter(|c| c.passes).collect();
    if passing.is_empty() {
        return None;
    }
    let key = |c: &Candidate| c.since.as_deref().and_then(|s| parse_version(s).ok());
    let mut best = 0usize;
    for (rank, c) in passing.iter().enumerate() {
        // Highest `since` wins; a later declaration breaks a tie (the newer
        // edit). `None` (base region) sorts oldest.
        let better = match (key(c), key(passing[best])) {
            (Some(a), Some(b)) => a >= b,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => true,
        };
        if better {
            best = rank;
        }
    }
    Some(best)
}

/// Every distinct version a marker in `src` mentions — the points at which
/// content can appear (a marker's `from`) or disappear (a range's `to`).
///
/// Deliberately file-wide rather than per-declaration: markers *inside* a
/// declaration matter as much as the ones around it, and a source usually
/// mentions only a handful of distinct versions however many markers it has.
/// Sorted ascending; unparseable tokens are skipped.
pub fn version_points(src: &str, style: CommentStyle) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for line in src.lines() {
        let m = match detect_marker(line, style) {
            MarkerKind::Versioned(m) | MarkerKind::InlineRange(m) => m,
            _ => continue,
        };
        for token in [Some(m.version.clone()), m.to.clone()]
            .into_iter()
            .flatten()
        {
            if parse_version(&token).is_ok() && !tokens.contains(&token) {
                tokens.push(token);
            }
        }
    }
    tokens.sort_by(|a, b| match (parse_version(a), parse_version(b)) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        _ => a.cmp(b),
    });
    tokens
}

/// The enclosing marker stack for every line, mirroring Vertion's pairing: a
/// `Versioned` marker closes the block on top when `(version, to)` match, and a
/// tag-only marker closes on a matching tag list. Inline range markers apply to
/// the following line only and never open a block, so they're skipped.
fn marker_stacks(src: &str, style: CommentStyle) -> Vec<Vec<Marker>> {
    let mut stack: Vec<Marker> = Vec::new();
    let mut per_line = Vec::new();
    for line in src.lines() {
        match detect_marker(line, style) {
            MarkerKind::Versioned(m) => {
                let closes = stack
                    .last()
                    .map(|t| t.version == m.version && t.to == m.to)
                    .unwrap_or(false);
                if closes {
                    stack.pop();
                } else {
                    stack.push(m);
                }
            }
            MarkerKind::TagOnly(m) => {
                let closes = stack
                    .last()
                    .map(|t| t.is_tag_only() && t.tags == m.tags)
                    .unwrap_or(false);
                if closes {
                    stack.pop();
                } else {
                    stack.push(m);
                }
            }
            _ => {}
        }
        per_line.push(stack.clone());
    }
    per_line
}

fn line_of(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUP: &str = concat!(
        "//version 1.0 2.0 *\n",
        "export const C = { legacy: true };\n",
        "//version 1.0 2.0 *\n",
        "//version 2.0 *\n",
        "export const C = { reworked: true };\n",
        "//version 2.0 *\n",
    );

    fn mode(spec: &str) -> FilterMode {
        vertion::filter::parse_filter(&[spec.to_string()]).unwrap()
    }

    fn cands(src: &str, spec: &str) -> Vec<Candidate> {
        candidates(
            src,
            "C",
            CommentStyle::DoubleSlash,
            Some((&mode(spec), &[])),
        )
        .unwrap()
    }

    #[test]
    fn a_single_declaration_needs_no_choosing() {
        let c = cands("export const C = 1;\n", "2.0");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].since, None);
        assert_eq!(choose(&c), Some(0));
    }

    #[test]
    fn an_absent_name_has_no_candidates() {
        assert!(cands("export const OTHER = 1;\n", "2.0").is_empty());
        assert_eq!(choose(&[]), None);
    }

    #[test]
    fn picks_the_newest_valid_declaration() {
        // At 2.5 only the 2.0 block survives — the range block ends at 2.0.
        let c = cands(DUP, "2.5");
        assert_eq!(c.len(), 2);
        assert!(!c[0].passes, "{c:?}");
        assert!(c[1].passes, "{c:?}");
        assert_eq!(c[1].since.as_deref(), Some("2.0"));
        assert_eq!(choose(&c), Some(0)); // rank among survivors
    }

    #[test]
    fn picks_the_old_one_at_an_old_version() {
        let c = cands(DUP, "1.5");
        assert!(c[0].passes && !c[1].passes, "{c:?}");
        assert_eq!(choose(&c), Some(0));
    }

    #[test]
    fn when_both_survive_the_newer_block_wins() {
        // Overlapping cumulative blocks: both pass at 2.5, so `since` decides.
        let src = concat!(
            "//version 1.0 *\n",
            "export const C = 1;\n",
            "//version 1.0 *\n",
            "//version 2.0 *\n",
            "export const C = 2;\n",
            "//version 2.0 *\n",
        );
        let c = cands(src, "2.5");
        assert!(c[0].passes && c[1].passes, "{c:?}");
        assert_eq!(c[0].since.as_deref(), Some("1.0"));
        assert_eq!(c[1].since.as_deref(), Some("2.0"));
        assert_eq!(choose(&c), Some(1));
    }

    #[test]
    fn a_versioned_declaration_beats_the_base_one() {
        let src = concat!(
            "export const C = 0;\n",
            "//version 2.0 *\n",
            "export const C = 2;\n",
            "//version 2.0 *\n",
        );
        let c = cands(src, "2.5");
        assert_eq!(c[0].since, None, "base region is oldest");
        assert_eq!(c[1].since.as_deref(), Some("2.0"));
        assert_eq!(choose(&c), Some(1));
        // …but below 2.0 only the base one exists.
        let c = cands(src, "1.0");
        assert!(c[0].passes && !c[1].passes, "{c:?}");
        assert_eq!(choose(&c), Some(0));
    }

    #[test]
    fn nothing_valid_yields_no_choice() {
        let src = concat!(
            "//version 3.0 *\nexport const C = 1;\n//version 3.0 *\n",
            "//version 4.0 *\nexport const C = 2;\n//version 4.0 *\n",
        );
        let c = cands(src, "1.0");
        assert!(c.iter().all(|c| !c.passes), "{c:?}");
        assert_eq!(choose(&c), None);
    }

    #[test]
    fn tags_gate_a_duplicate_too() {
        let src = concat!(
            "//version 2.0 [vanilla]*\n",
            "export const C = 1;\n",
            "//version 2.0 [vanilla]*\n",
            "//version 2.0 [tools]*\n",
            "export const C = 2;\n",
            "//version 2.0 [tools]*\n",
        );
        // Tags are opt-in: with no tag active neither tagged declaration
        // survives, so there is nothing to choose between.
        let none = candidates(
            src,
            "C",
            CommentStyle::DoubleSlash,
            Some((&mode("2.5"), &[])),
        )
        .unwrap();
        assert!(!none[0].passes && !none[1].passes, "{none:?}");
        assert_eq!(choose(&none), None);

        // The `*` wildcard admits both, and the newest one wins.
        let all = candidates(
            src,
            "C",
            CommentStyle::DoubleSlash,
            Some((&mode("2.5"), &["*".to_string()])),
        )
        .unwrap();
        assert!(all[0].passes && all[1].passes, "wildcard keeps both");

        let tools = candidates(
            src,
            "C",
            CommentStyle::DoubleSlash,
            Some((&mode("2.5"), &["tools".to_string()])),
        )
        .unwrap();
        assert!(!tools[0].passes && tools[1].passes, "{tools:?}");
        assert_eq!(choose(&tools), Some(0));
    }

    #[test]
    fn nested_blocks_follow_the_ancestor_rule() {
        // The inner block passes at 2.5 but its parent doesn't, so neither does it.
        let src = concat!(
            "//version 9.0 *\n",
            "//version 2.0 *\n",
            "export const C = 1;\n",
            "//version 2.0 *\n",
            "//version 9.0 *\n",
            "export const C = 2;\n",
        );
        let c = cands(src, "2.5");
        assert!(!c[0].passes, "ancestor rule ignored: {c:?}");
        assert!(c[1].passes);
        assert_eq!(choose(&c), Some(0));
    }
}
