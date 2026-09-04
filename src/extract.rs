//! Structure-aware mini-lexer that slices the initializer of a top-level
//! `const NAME = ...` (or `export const NAME = ...`) out of JS source,
//! verbatim. No AST: tracks bracket depth, `'`/`"` strings with escapes,
//! template literals with nested `${}`, `//` and `/* */` comments, and a
//! regex-vs-division heuristic. Heuristic misfires surface as loud
//! "unterminated" errors, never silent corruption.

/// Last significant token class — drives the regex-vs-division choice and
/// the ASI (no-semicolon) termination heuristic.
#[derive(PartialEq, Clone, Copy)]
enum Prev {
    Start,
    Op,    // operator, opener, `,`, `;`, `=`, keyword like `return`
    Value, // identifier, literal, closer — an expression could end here
}

/// Top-level declaration matcher state.
#[derive(PartialEq)]
enum Decl {
    Idle,
    Const, // just saw `const` at depth 0
    Named, // saw `const NAME` — an `=` next starts the capture
}

/// Template-literal nesting: alternating text / `${}` substitution frames.
enum Ctx {
    Template,
    Sub { entry_depth: i64 },
}

/// Next-line first chars that continue an expression across a newline
/// (mirrors real ASI: `a \n + b`, `a \n .b()`, and the classic `(`/`[`/`` ` ``
/// hazards are all continuations).
const CONTINUES: &[u8] = b".+-*/%<>&|^?:,=([`";

/// Chars whose *trailing* position never ends an expression — covered by
/// `Prev::Op` instead of a byte set.
///
/// Shared engine: every top-level `const` / `export const` in source order as
/// `(name, verbatim-initializer)`. `extract` and `extract_all` are thin views.
fn scan_pos(src: &str) -> Result<Vec<Decl3>, String> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src); // BOM
    let b = src.as_bytes();
    let len = b.len();

    let mut out: Vec<Decl3> = Vec::new();
    let mut i = 0usize;
    let mut depth: i64 = 0; // single counter for () [] {}
    let mut ctx: Vec<Ctx> = Vec::new();
    let mut prev = Prev::Start;
    let mut decl = Decl::Idle;
    let mut pending: Option<String> = None; // name after `const`, awaiting `=`
    let mut cap_start: Option<usize> = None;
    let mut cap_name: Option<String> = None; // name whose initializer we're slicing
    let mut sig_end = 0usize; // end of last significant token (skips trailing comments)

    while i < len {
        // template-literal text mode
        if matches!(ctx.last(), Some(Ctx::Template)) {
            match b[i] {
                b'\\' => i += 2,
                b'`' => {
                    ctx.pop();
                    prev = Prev::Value;
                    i += 1;
                    sig_end = i;
                }
                b'$' if i + 1 < len && b[i + 1] == b'{' => {
                    ctx.push(Ctx::Sub { entry_depth: depth });
                    i += 2;
                }
                _ => i += 1,
            }
            continue;
        }

        let top = depth == 0 && ctx.is_empty();
        match b[i] {
            b' ' | b'\t' | b'\r' => i += 1,
            b'\n' => {
                // ASI: a newline at depth 0 ends the initializer when what we
                // have looks complete and the next token can't continue it.
                if let Some(start) = cap_start {
                    if top && prev == Prev::Value {
                        let end = sig_end.max(start);
                        if !src[start..end].trim().is_empty()
                            && !matches!(next_significant(b, i + 1), Some(c) if CONTINUES.contains(&c))
                        {
                            push_decl(&mut out, &mut cap_name, src, start, end)?;
                            cap_start = None;
                            decl = Decl::Idle;
                        }
                    }
                }
                i += 1;
            }
            b'/' if i + 1 < len && b[i + 1] == b'/' => {
                while i < len && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < len && b[i + 1] == b'*' => match find(b, i + 2, b"*/") {
                Some(p) => i = p + 2,
                None => {
                    return Err(format!(
                        "unterminated /* comment (line {})",
                        line_of(src, i)
                    ))
                }
            },
            b'/' => {
                if prev == Prev::Value {
                    // division
                    i += 1;
                    prev = Prev::Op;
                } else {
                    i = skip_regex(src, b, i)?;
                    prev = Prev::Value;
                }
                decl = Decl::Idle;
                sig_end = i;
            }
            b'\'' | b'"' => {
                i = skip_string(src, b, i)?;
                prev = Prev::Value;
                decl = Decl::Idle;
                sig_end = i;
            }
            b'`' => {
                ctx.push(Ctx::Template);
                decl = Decl::Idle;
                i += 1;
                sig_end = i;
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                prev = Prev::Op;
                if cap_start.is_none() {
                    decl = Decl::Idle;
                }
                i += 1;
                sig_end = i;
            }
            b')' | b']' => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!(
                        "unbalanced `{}` (line {})",
                        b[i] as char,
                        line_of(src, i)
                    ));
                }
                prev = Prev::Value;
                i += 1;
                sig_end = i;
            }
            b'}' => {
                // `}` closing a `${` substitution returns to template text
                if let Some(Ctx::Sub { entry_depth }) = ctx.last() {
                    if depth == *entry_depth {
                        ctx.pop();
                        i += 1;
                        continue;
                    }
                }
                depth -= 1;
                if depth < 0 {
                    return Err(format!("unbalanced `}}` (line {})", line_of(src, i)));
                }
                prev = Prev::Value;
                i += 1;
                sig_end = i;
            }
            b';' | b',' => {
                if let Some(start) = cap_start {
                    if top {
                        push_decl(&mut out, &mut cap_name, src, start, i)?;
                        cap_start = None;
                    }
                }
                prev = Prev::Op;
                decl = Decl::Idle;
                i += 1;
                sig_end = i;
            }
            b'=' => {
                // exactly `=` (not ==, =>) after `const NAME` starts the capture
                if decl == Decl::Named
                    && top
                    && cap_start.is_none()
                    && !(i + 1 < len && (b[i + 1] == b'=' || b[i + 1] == b'>'))
                {
                    cap_start = Some(i + 1);
                    cap_name = pending.take();
                }
                decl = Decl::Idle;
                prev = Prev::Op;
                i += 1;
                sig_end = i;
            }
            c if c == b'_' || c == b'$' || c.is_ascii_alphabetic() || c >= 0x80 => {
                let start = i;
                while i < len
                    && (b[i] == b'_'
                        || b[i] == b'$'
                        || b[i].is_ascii_alphanumeric()
                        || b[i] >= 0x80)
                {
                    i += 1;
                }
                let word = &src[start..i];
                // keywords a regex may directly follow
                prev = if matches!(
                    word,
                    "return"
                        | "typeof"
                        | "case"
                        | "in"
                        | "of"
                        | "new"
                        | "delete"
                        | "void"
                        | "instanceof"
                        | "do"
                        | "else"
                        | "yield"
                        | "await"
                        | "throw"
                ) {
                    Prev::Op
                } else {
                    Prev::Value
                };
                if cap_start.is_none() && top {
                    if word == "const" {
                        decl = Decl::Const;
                    } else if decl == Decl::Const {
                        pending = Some(word.to_string());
                        decl = Decl::Named;
                    } else {
                        // ponytail: `const A = 1, B = 2` — only the first
                        // declarator is captured; siblings are skipped
                        decl = Decl::Idle;
                    }
                }
                sig_end = i;
            }
            b'0'..=b'9' => {
                while i < len && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                    i += 1;
                }
                prev = Prev::Value;
                decl = Decl::Idle;
                sig_end = i;
            }
            _ => {
                // operators: + - * % < > & | ^ ! ? : ~ . @ # \
                prev = Prev::Op;
                decl = Decl::Idle;
                i += 1;
                sig_end = i;
            }
        }
    }

    if let Some(start) = cap_start {
        if depth == 0 && ctx.is_empty() {
            push_decl(&mut out, &mut cap_name, src, start, sig_end.max(start))?;
        } else {
            let n = cap_name.as_deref().unwrap_or("?");
            return Err(format!(
                "unterminated initializer for `{n}` (EOF at depth {depth})"
            ));
        }
    }
    Ok(out)
}

/// One top-level declaration: name, verbatim initializer, and the byte offset
/// its initializer starts at (used to locate the enclosing version block).
pub struct Decl3 {
    pub name: String,
    pub value: String,
    pub offset: usize,
}

fn push_decl(
    out: &mut Vec<Decl3>,
    cap_name: &mut Option<String>,
    src: &str,
    start: usize,
    end: usize,
) -> Result<(), String> {
    let s = src[start..end].trim();
    let name = cap_name.take().unwrap_or_default();
    if s.is_empty() {
        return Err(format!("empty initializer for `{name}`"));
    }
    out.push(Decl3 {
        name,
        value: s.to_string(),
        offset: start,
    });
    Ok(())
}

fn scan(src: &str) -> Result<Vec<(String, String)>, String> {
    Ok(scan_pos(src)?
        .into_iter()
        .map(|d| (d.name, d.value))
        .collect())
}

/// Every top-level declaration with its position, in source order.
pub fn scan_positions(src: &str) -> Result<Vec<Decl3>, String> {
    scan_pos(src)
}

/// Slice the verbatim initializer of the top-level `const NAME` (or
/// `export const NAME`). First match wins.
pub fn extract(src: &str, name: &str) -> Result<String, String> {
    for (n, v) in scan(src)? {
        if n == name {
            return Ok(v);
        }
    }
    Err(format!("`const {name}` not found at top level"))
}

/// Every top-level `const` / `export const`, in source order — used by
/// `--pull-all` to re-nest a generated module's exports under one object.
/// ponytail: pulls all top-level consts; a generated `_wikiData.js` has only
/// `export const`s, so the distinction never bites — revisit if pointed at
/// hand-written modules with private helper consts.
pub fn extract_all(src: &str) -> Result<Vec<(String, String)>, String> {
    scan(src)
}

/// First significant byte at/after `i`, skipping whitespace and comments.
fn next_significant(b: &[u8], mut i: usize) -> Option<u8> {
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            // An unterminated block comment means nothing significant follows.
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => i = find(b, i + 2, b"*/")? + 2,
            c => return Some(c),
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

/// `i` is at the opening quote; returns the index just past the closing one.
fn skip_string(src: &str, b: &[u8], start: usize) -> Result<usize, String> {
    let quote = b[start];
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2, // also covers escaped line continuations
            b'\n' => break,
            c if c == quote => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err(format!(
        "unterminated string (line {})",
        line_of(src, start)
    ))
}

/// `i` is at the opening `/`; returns the index just past the flags.
fn skip_regex(src: &str, b: &[u8], start: usize) -> Result<usize, String> {
    let mut i = start + 1;
    let mut in_class = false;
    while i < b.len() && b[i] != b'\n' {
        match b[i] {
            b'\\' => i += 2,
            b'[' => {
                in_class = true;
                i += 1;
            }
            b']' => {
                in_class = false;
                i += 1;
            }
            b'/' if !in_class => {
                i += 1;
                while i < b.len() && b[i].is_ascii_alphabetic() {
                    i += 1;
                }
                return Ok(i);
            }
            _ => i += 1,
        }
    }
    Err(format!(
        "unterminated regex literal (line {}) — possible regex-vs-division misparse",
        line_of(src, start)
    ))
}

fn line_of(src: &str, i: usize) -> usize {
    src.as_bytes()[..i.min(src.len())]
        .iter()
        .filter(|&&c| c == b'\n')
        .count()
        + 1
}

#[cfg(test)]
mod tests {
    use super::{extract, extract_all};

    #[test]
    fn extract_all_collects_every_top_level_const_in_order() {
        let src = "// header\nexport const A = { x: 1 };\nexport const B = Object.keys(A).length;\nconst C = `t ${A}`\n";
        let all = extract_all(src).unwrap();
        assert_eq!(
            all,
            vec![
                ("A".to_string(), "{ x: 1 }".to_string()),
                ("B".to_string(), "Object.keys(A).length".to_string()),
                ("C".to_string(), "`t ${A}`".to_string()),
            ]
        );
    }

    #[test]
    fn extract_all_ignores_nested_and_commented_consts() {
        let src = "export const A = 1;\nfunction f() { const HIDDEN = 2; }\n// const ALSO = 3;\nexport const B = 4;\n";
        let names: Vec<_> = extract_all(src)
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names, vec!["A", "B"]);
    }

    #[test]
    fn simple() {
        assert_eq!(extract("const A = 1;", "A").unwrap(), "1");
    }

    #[test]
    fn export_const() {
        assert_eq!(
            extract("export const A = { b: 1 };", "A").unwrap(),
            "{ b: 1 }"
        );
    }

    #[test]
    fn nested_object_multiline() {
        let src = "const CFG = {\n  a: { b: [1, 2, { c: 3 }] },\n  d: 'x',\n};\nconst OTHER = 1;";
        assert_eq!(
            extract(src, "CFG").unwrap(),
            "{\n  a: { b: [1, 2, { c: 3 }] },\n  d: 'x',\n}"
        );
    }

    #[test]
    fn template_with_nested_substitution() {
        let src = "const T = `x ${ { a: \"}\" }.a } y ${`inner ${1 + 2}`}`;";
        assert_eq!(
            extract(src, "T").unwrap(),
            "`x ${ { a: \"}\" }.a } y ${`inner ${1 + 2}`}`"
        );
    }

    #[test]
    fn comment_containing_closing_brace() {
        let src = "const O = {\n  // }; not the end\n  a: 1, /* }; still not */\n};";
        assert_eq!(
            extract(src, "O").unwrap(),
            "{\n  // }; not the end\n  a: 1, /* }; still not */\n}"
        );
    }

    #[test]
    fn regex_literal_with_slash_in_class() {
        let src = "const R = /ab[/c]\\/d/g;";
        assert_eq!(extract(src, "R").unwrap(), "/ab[/c]\\/d/g");
    }

    #[test]
    fn regex_with_brace_and_quote() {
        let src = "const R = /x{2,3}\"'/;\nconst A = 1;";
        assert_eq!(extract(src, "R").unwrap(), "/x{2,3}\"'/");
        assert_eq!(extract(src, "A").unwrap(), "1");
    }

    #[test]
    fn division_not_regex() {
        assert_eq!(
            extract("const D = a / b + c / d;", "D").unwrap(),
            "a / b + c / d"
        );
    }

    #[test]
    fn asi_no_semicolons() {
        let src = "const A = 1\nconst B = 'two'\nconst C = [3]";
        assert_eq!(extract(src, "A").unwrap(), "1");
        assert_eq!(extract(src, "B").unwrap(), "'two'");
        assert_eq!(extract(src, "C").unwrap(), "[3]");
    }

    #[test]
    fn multiline_continuation() {
        assert_eq!(
            extract("const A = 1 +\n  2\nconst B = 0", "A").unwrap(),
            "1 +\n  2"
        );
        assert_eq!(
            extract("const A = foo\n  .bar()\n  .baz\nconst B = 0", "A").unwrap(),
            "foo\n  .bar()\n  .baz"
        );
    }

    #[test]
    fn trailing_line_comment_not_captured() {
        assert_eq!(
            extract("const A = 1 // note\nconst B = 2", "A").unwrap(),
            "1"
        );
        assert_eq!(extract("const A = 1 // eof", "A").unwrap(), "1");
    }

    #[test]
    fn depth_zero_only() {
        let src = "function f() {\n  const A = 99;\n}\nconst A = 1;";
        assert_eq!(extract(src, "A").unwrap(), "1");
        let src2 = "if (x) { const B = 1; }";
        assert!(extract(src2, "B").unwrap_err().contains("not found"));
    }

    #[test]
    fn const_in_string_or_comment_ignored() {
        let src = "const S = \"const A = 5;\";\n// const A = 6;\nconst A = 7;";
        assert_eq!(extract(src, "A").unwrap(), "7");
    }

    #[test]
    fn string_with_escapes_and_braces() {
        assert_eq!(
            extract("const S = \"a\\\";{\" + 'b\\'}';", "S").unwrap(),
            "\"a\\\";{\" + 'b\\'}'"
        );
    }

    #[test]
    fn multi_declarator_first_only() {
        assert_eq!(extract("const A = 1, B = 2;", "A").unwrap(), "1");
        assert!(extract("const A = 1, B = 2;", "B").is_err()); // known ceiling
    }

    #[test]
    fn not_found() {
        assert!(extract("let A = 1;", "A")
            .unwrap_err()
            .contains("not found"));
    }

    #[test]
    fn unterminated_is_loud() {
        assert!(extract("const A = {", "A")
            .unwrap_err()
            .contains("unterminated"));
        assert!(extract("const A = 'oops", "A")
            .unwrap_err()
            .contains("unterminated string"));
    }

    #[test]
    fn eof_without_terminator() {
        assert_eq!(extract("const A = { x: 1 }", "A").unwrap(), "{ x: 1 }");
        assert_eq!(extract("const A = 42", "A").unwrap(), "42");
    }

    #[test]
    fn arrow_function_initializer() {
        let src = "const F = (a, b) => { return a >= b ? {a} : [b]; };";
        assert_eq!(
            extract(src, "F").unwrap(),
            "(a, b) => { return a >= b ? {a} : [b]; }"
        );
    }

    #[test]
    fn equality_not_capture_start() {
        // `NAME ==` must not start a capture; the real decl comes later
        let src = "const FLAG = A == 1;\nconst A = 2;";
        assert_eq!(extract(src, "A").unwrap(), "2");
    }

    #[test]
    fn bom_stripped() {
        assert_eq!(extract("\u{feff}const A = 1;", "A").unwrap(), "1");
    }

    #[test]
    fn unicode_in_strings_and_idents() {
        let src = "const NÄME = 'ünïcode';\nconst A = `héllo ${wörld}`;";
        assert_eq!(extract(src, "A").unwrap(), "`héllo ${wörld}`");
        assert_eq!(extract(src, "NÄME").unwrap(), "'ünïcode'");
    }

    #[test]
    fn regex_after_return_inside_function() {
        // depth tracking must survive a regex in statement position
        let src = "function f() { return /}{\"/.test(x); }\nconst A = 1;";
        assert_eq!(extract(src, "A").unwrap(), "1");
    }
}
