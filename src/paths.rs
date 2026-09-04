//! Expansion of paths written in `jsdata.cfg`: `~`, `%VAR%`, `${VAR}`,
//! `$VAR`, and `${VAR:-fallback}`.
//!
//! The `:-` form is what lets one config extract from either tree. A [Vertion]
//! build exports `VERTION_OUTPUT` pointing at what it just wrote, so
//!
//! ```text
//! var PASSIVES = ${VERTION_OUTPUT:-./src}/BP/scripts/passives.js -> const PASSIVES
//! ```
//!
//! reads the freshly built (already version-filtered) tree when `jsdata` runs
//! from a `vertion build`, and `./src` when it's run by hand.
//!
//! An expanded **absolute** path overrides the config's directory through normal
//! `Path::join` semantics — which is exactly why `$VERTION_OUTPUT` sources work
//! while `out` stays in the project.
//!
//! [Vertion]: https://github.com/vertX-dev/vertion

use std::fmt;

/// A variable was referenced with no value and no `:-` fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandError {
    pub var: String,
    /// How it was written, for the message (`$FOO`, `${FOO}`, `%FOO%`).
    pub form: String,
}

impl fmt::Display for ExpandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is not set (use `${{{}:-./src}}` to give it a default outside a vertion build)",
            self.form, self.var
        )
    }
}

impl std::error::Error for ExpandError {}

/// True if the string contains anything this module would expand. Lets the
/// parser skip untouched lines entirely.
pub fn has_references(input: &str) -> bool {
    input.contains('%') || input.contains('$') || input.starts_with('~')
}

/// Expand a path written in the config. An unset variable with no `:-` fallback
/// is an error — a path silently containing `%VERTION_OUTPUT%` would fail later
/// as a confusing "file not found".
pub fn expand(input: &str) -> Result<String, ExpandError> {
    let mut out = String::new();

    // A leading `~`, but only as a whole path segment — `~foo` is a filename.
    let mut rest = match input.strip_prefix('~') {
        Some(tail) if tail.is_empty() || tail.starts_with('/') || tail.starts_with('\\') => {
            match home_dir() {
                Some(home) => out.push_str(&home),
                None => out.push('~'),
            }
            tail
        }
        _ => input,
    };

    // One left-to-right pass. Deliberately not two passes (`%` then `$`): a
    // variable whose *value* contains `$X` must not be expanded again.
    while let Some(pos) = rest.find(['%', '$']) {
        out.push_str(&rest[..pos]);
        let sigil = rest[pos..].chars().next().expect("find returned a match");
        let after = &rest[pos + sigil.len_utf8()..];
        rest = match sigil {
            '%' => take_percent(after, &mut out)?,
            _ => take_dollar(after, &mut out)?,
        };
    }
    out.push_str(rest);
    Ok(out)
}

/// Handle everything after an opening `%`. Returns the unconsumed remainder.
fn take_percent<'a>(after: &'a str, out: &mut String) -> Result<&'a str, ExpandError> {
    // `%%` is a literal percent.
    if let Some(tail) = after.strip_prefix('%') {
        out.push('%');
        return Ok(tail);
    }
    match after.find('%') {
        Some(end) => {
            let name = &after[..end];
            push_value(out, name, None, &format!("%{name}%"))?;
            Ok(&after[end + 1..])
        }
        // Unterminated — a bare `%` in a path is legal, keep it.
        None => {
            out.push('%');
            out.push_str(after);
            Ok("")
        }
    }
}

/// Handle everything after a `$`. Returns the unconsumed remainder.
fn take_dollar<'a>(after: &'a str, out: &mut String) -> Result<&'a str, ExpandError> {
    if let Some(braced) = after.strip_prefix('{') {
        return match braced.find('}') {
            Some(end) => {
                let inner = &braced[..end];
                let (name, fallback) = match inner.split_once(":-") {
                    Some((n, f)) => (n, Some(f)),
                    None => (inner, None),
                };
                push_value(out, name, fallback, &format!("${{{name}}}"))?;
                Ok(&braced[end + 1..])
            }
            // Unterminated `${` — keep it verbatim.
            None => {
                out.push('$');
                Ok(after)
            }
        };
    }
    let name_len = after
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(after.len());
    if name_len == 0 {
        out.push('$');
        return Ok(after);
    }
    let name = &after[..name_len];
    push_value(out, name, None, &format!("${name}"))?;
    Ok(&after[name_len..])
}

/// Resolve one reference. An empty value counts as unset *for choosing the
/// fallback* (POSIX `:-` semantics) but is still a legitimate expansion to
/// nothing when no fallback was given.
fn push_value(
    out: &mut String,
    name: &str,
    fallback: Option<&str>,
    form: &str,
) -> Result<(), ExpandError> {
    let value = std::env::var(name).ok();
    match (&value, fallback) {
        (Some(v), _) if !v.is_empty() => out.push_str(v),
        (_, Some(f)) => out.push_str(f),
        (Some(_), None) => {} // set but empty, no fallback → expands to nothing
        (None, None) => {
            return Err(ExpandError {
                var: name.to_string(),
                form: form.to_string(),
            });
        }
    }
    Ok(())
}

fn home_dir() -> Option<String> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Env vars are process-global; give every test its own name.
    fn with_var<T>(name: &str, value: &str, f: impl FnOnce() -> T) -> T {
        std::env::set_var(name, value);
        let out = f();
        std::env::remove_var(name);
        out
    }

    #[test]
    fn expands_every_form() {
        with_var("WD_TEST_ENV", "VALUE", || {
            assert_eq!(expand("%WD_TEST_ENV%/x").unwrap(), "VALUE/x");
            assert_eq!(expand("$WD_TEST_ENV/x").unwrap(), "VALUE/x");
            assert_eq!(expand("${WD_TEST_ENV}/x").unwrap(), "VALUE/x");
        });
    }

    #[test]
    fn fallback_used_only_when_unset_or_empty() {
        with_var("WD_TEST_FB", "REAL", || {
            assert_eq!(expand("${WD_TEST_FB:-./src}/BP").unwrap(), "REAL/BP");
        });
        assert_eq!(expand("${WD_TEST_FB:-./src}/BP").unwrap(), "./src/BP");
        with_var("WD_TEST_FB", "", || {
            assert_eq!(expand("${WD_TEST_FB:-./src}/BP").unwrap(), "./src/BP");
        });
    }

    #[test]
    fn unset_without_fallback_is_an_error() {
        let err = expand("%WD_MISSING_XYZ%/a").unwrap_err();
        assert_eq!(err.var, "WD_MISSING_XYZ");
        assert!(err.to_string().contains("%WD_MISSING_XYZ%"));
        assert!(expand("$WD_MISSING_XYZ/a").is_err());
        assert!(expand("${WD_MISSING_XYZ}/a").is_err());
    }

    #[test]
    fn set_but_empty_without_fallback_expands_to_nothing() {
        with_var("WD_TEST_EMPTY", "", || {
            assert_eq!(expand("a/${WD_TEST_EMPTY}b").unwrap(), "a/b");
        });
    }

    #[test]
    fn double_percent_is_a_literal() {
        assert_eq!(expand("100%%/x").unwrap(), "100%/x");
    }

    #[test]
    fn unterminated_references_are_left_alone() {
        assert_eq!(expand("50% off").unwrap(), "50% off");
        assert_eq!(expand("${OPEN/x").unwrap(), "${OPEN/x");
        assert_eq!(expand("cost $ x").unwrap(), "cost $ x");
    }

    #[test]
    fn a_value_is_not_re_expanded() {
        with_var("WD_TEST_NESTED", "$HOME/%APPDATA%", || {
            assert_eq!(expand("${WD_TEST_NESTED}/x").unwrap(), "$HOME/%APPDATA%/x");
        });
    }

    #[test]
    fn plain_paths_pass_through() {
        assert_eq!(
            expand("src/BP/scripts/a.js").unwrap(),
            "src/BP/scripts/a.js"
        );
        assert!(!has_references("src/BP/scripts/a.js"));
        assert!(has_references("${VERTION_OUTPUT:-./src}/BP"));
    }

    #[test]
    fn a_path_with_an_arrow_survives() {
        // The config DSL splits on `->` before expanding; make sure a directory
        // literally named `weird->dir` isn't mangled by expansion either.
        assert_eq!(expand("weird->dir/a.js").unwrap(), "weird->dir/a.js");
    }
}
