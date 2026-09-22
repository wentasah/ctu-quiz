//! Syntax highlighting of fenced code blocks.
//!
//! The highlighter runs once, when a question is published, and emits CSS
//! classes (`hl-keyword`, `hl-string`, …) rather than inline colours, so that
//! `server/static/app.css` can pick the colours and have them work in both the
//! light and the dark theme.
//!
//! Without the `highlight` feature the same function produces a plain, escaped
//! code block, which is what the teacher's CLI is built with: it never renders
//! HTML, so it does not need the syntax definitions.

/// Prefix of every class the highlighter emits.
pub const CLASS_PREFIX: &str = "hl-";

/// Renders one fenced code block as HTML.
///
/// `language` is the info string of the fence (`bash` in ```` ``` bash ````)
/// and may be empty or unknown, in which case the code is only escaped.
pub fn code_block(language: &str, code: &str) -> String {
    let body = highlight(language, code).unwrap_or_else(|| escape_text(code));
    match language {
        "" => format!("<pre class=\"code\"><code>{body}</code></pre>"),
        language => format!(
            "<pre class=\"code\"><code class=\"language-{}\">{body}</code></pre>",
            escape_text(language)
        ),
    }
}

/// Escapes text for an HTML element or a quoted attribute.
pub fn escape_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Loads the syntax definitions ahead of the first question.
///
/// They are a few megabytes that have to be unpacked, and doing that while a
/// lecture hall waits for a question would be a shame. Without the `highlight`
/// feature this does nothing.
pub fn warm_up() {
    #[cfg(feature = "highlight")]
    let _ = &*syntaxes::SYNTAXES;
}

/// Whether the build can highlight at all.
pub const fn is_available() -> bool {
    cfg!(feature = "highlight")
}

#[cfg(not(feature = "highlight"))]
fn highlight(_language: &str, _code: &str) -> Option<String> {
    None
}

#[cfg(feature = "highlight")]
fn highlight(language: &str, code: &str) -> Option<String> {
    use syntect::{
        html::{ClassStyle, ClassedHTMLGenerator},
        util::LinesWithEndings,
    };

    if language.is_empty() {
        return None;
    }
    let syntaxes = &*syntaxes::SYNTAXES;
    let syntax = syntaxes.find_syntax_by_token(language)?;

    let mut generator = ClassedHTMLGenerator::new_with_class_style(
        syntax,
        syntaxes,
        ClassStyle::SpacedPrefixed {
            prefix: CLASS_PREFIX,
        },
    );
    for line in LinesWithEndings::from(code) {
        // A failure here means the syntax definition itself is broken; fall
        // back to the plain code block rather than showing half of it.
        generator
            .parse_html_for_line_which_includes_newline(line)
            .ok()?;
    }
    Some(generator.finalize())
}

#[cfg(feature = "highlight")]
mod syntaxes {
    use std::sync::LazyLock;

    use syntect::parsing::SyntaxSet;

    /// The syntax definitions bundled with syntect, unpacked once.
    pub static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
}

/// The text of an HTML fragment, with all tags removed.
///
/// Highlighted code is full of `<span>`s, so the tests compare the text.
#[cfg(test)]
pub(crate) fn text_of(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(character),
            _ => {}
        }
    }
    text.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_without_a_language_is_only_escaped() {
        let html = code_block("", "a < b && c > d\n");
        assert_eq!(
            html,
            "<pre class=\"code\"><code>a &lt; b &amp;&amp; c &gt; d\n</code></pre>"
        );
    }

    #[test]
    fn an_unknown_language_keeps_its_class() {
        let html = code_block("klingon", "nuqneH <\n");
        assert!(html.contains("<code class=\"language-klingon\">"), "{html}");
        assert!(html.contains("nuqneH &lt;"), "{html}");
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn the_languages_of_the_course_are_highlighted() {
        for (language, code, expected_class) in [
            (
                "bash",
                "# comment\nwhile read -r x; do :; done\n",
                "hl-comment",
            ),
            ("c", "int main(void) { return 0; /* ok */ }\n", "hl-storage"),
            ("rust", "fn main() { let x = 1; }\n", "hl-keyword"),
            ("python", "def f(x):\n    return \"s\"\n", "hl-string"),
        ] {
            let html = code_block(language, code);
            assert!(
                html.contains(&format!("class=\"{expected_class}")),
                "{language} is not highlighted: {html}"
            );
            // Highlighting must not lose or reorder any of the code.
            assert!(
                text_of(&html).contains(code.trim_end()),
                "{language}: {:?}",
                text_of(&html)
            );
        }
    }
}
