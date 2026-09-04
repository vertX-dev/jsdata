//! Markdown → HTML for the `md ... <-- html` directive. Thin wrapper over
//! pulldown-cmark with the extensions a wiki actually wants (tables,
//! strikethrough, task lists). Input is the developer's own `.md` files, so
//! the output is NOT sanitized — raw HTML in the source passes through.
//! ponytail: trusted authored content; add sanitization only if the wiki ever
//! renders contributor-submitted markdown.

use pulldown_cmark::{html, Options, Parser};

pub fn to_html(md: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(md, opts);
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

#[cfg(test)]
mod tests {
    use super::to_html;

    #[test]
    fn headings_emphasis_code() {
        let html = to_html("# Title\n\nSome **bold** and `code`.");
        assert!(html.contains("<h1>Title</h1>"), "{html}");
        assert!(html.contains("<strong>bold</strong>"), "{html}");
        assert!(html.contains("<code>code</code>"), "{html}");
    }

    #[test]
    fn table_extension_enabled() {
        let html = to_html("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains("<td>1</td>"), "{html}");
    }
}
