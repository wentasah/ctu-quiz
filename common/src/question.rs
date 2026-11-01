//! Parsing of a question written in Markdown.
//!
//! The format is the one described in the assignment: the file starts with the
//! question itself (arbitrary Markdown), and the possible answers are the
//! top-level list items that follow a Markdown horizontal rule (`---`). The
//! `question.md` file next to this crate is a complete example.
//!
//! Splitting the file at the horizontal rule is done textually (not through the
//! Markdown event stream) so that the rule is recognized even when it directly
//! follows a paragraph, where CommonMark would read it as a setext heading.
//! Horizontal rules inside fenced code blocks are ignored.
//!
//! An answer may say how many points it is worth: `- [x] ...` is a correct
//! answer worth 1 point, `- [ ] ...` a wrong one worth 0, and `{2}`, `{0.5}`
//! or `{-1}` at the very end of the answer sets the points explicitly. Either
//! every answer has such a mark or none does. The marks are removed from the
//! text, so that students do not see them.
//!
//! Both the question and the answers may show images, written the Markdown
//! way: `![description](img/diagram.svg)`. The server cannot read the
//! teacher's files, so a local image travels with the question as an
//! [`Image`] and ends up in the HTML as a `data:` URL; [`local_images`] says
//! which images those are. Images given by an `http(s):` URL are left to the
//! browser to fetch.

use std::collections::BTreeMap;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd, html};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

use crate::highlight;

/// Answers are labelled `A`..`Z`, so this is the hard upper limit.
pub const MAX_ANSWERS: usize = 26;

/// A question with its possible answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    /// Text of the first Markdown heading, if the question has one.
    pub title: Option<String>,
    /// The question text, as written by the teacher (without the `---`).
    pub prompt_markdown: String,
    /// The question text rendered to HTML.
    pub prompt_html: String,
    /// The possible answers, in the order they appear in the file.
    pub answers: Vec<Answer>,
}

/// One possible answer, i.e. one top-level list item after the `---` rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// `A`, `B`, `C`, ... as shown to students and stored in the CSV.
    pub label: String,
    /// The list item as written, with the bullet removed and de-indented.
    pub markdown: String,
    /// The list item rendered to HTML.
    pub html: String,
    /// One-line plain-text rendering, used in the CSV and in the terminal.
    pub plain: String,
    /// Points for choosing this answer; `None` when the question is not
    /// scored. Never meant for the students while the question is open.
    #[serde(default)]
    pub points: Option<f64>,
}

impl Answer {
    /// Whether choosing this answer earns points.
    pub fn is_correct(&self) -> bool {
        self.points.is_some_and(|points| points > 0.0)
    }
}

/// An image that a question shows, sent along with the Markdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    /// `image/png`, `image/svg+xml`, ...; one of those in [`IMAGE_TYPES`].
    pub media_type: String,
    /// The contents of the file, Base64-encoded.
    pub data: String,
}

/// The local images of a question, keyed by the path as the Markdown has it.
pub type Images = BTreeMap<String, Image>;

/// File name extensions of the images a question may show, with their media
/// types. Browsers display all of these in an `<img>`, and none of them can
/// run a script there, not even an SVG.
pub const IMAGE_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("svg", "image/svg+xml"),
];

/// The media type of an image file, judged by the extension of its name.
pub fn image_type(path: &str) -> Option<&'static str> {
    let (_, extension) = path.rsplit_once('.')?;
    IMAGE_TYPES
        .iter()
        .find(|(known, _)| extension.eq_ignore_ascii_case(known))
        .map(|(_, media_type)| *media_type)
}

/// Reasons why a Markdown file is not a usable question.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error(
        "no `---` separator found: the possible answers have to follow a Markdown horizontal rule"
    )]
    MissingSeparator,
    #[error(
        "no list item found after the `---` separator: every possible answer has to be a top-level list item"
    )]
    NoAnswers,
    #[error("found only one possible answer, a question needs at least two")]
    TooFewAnswers,
    #[error("found {0} possible answers, at most 26 are supported")]
    TooManyAnswers(usize),
    #[error("the question text above the `---` separator is empty")]
    EmptyPrompt,
    #[error("answer {0} is empty")]
    EmptyAnswer(String),
    #[error("the image {0} is missing")]
    MissingImage(String),
    #[error("the image {0} is not a PNG, JPEG, GIF, WebP, AVIF or SVG file")]
    BadImage(String),
    #[error(
        "answer {0} has no `[x]`, `[ ]` or `{{points}}` mark; either every answer is marked or none is"
    )]
    UnmarkedAnswer(String),
}

/// Markdown extensions enabled for questions and answers.
fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

/// Parses a Markdown question file that shows no local images.
pub fn parse(source: &str) -> Result<Question, ParseError> {
    parse_with_images(source, &Images::new())
}

/// Parses a Markdown question file, taking the local images it shows from
/// `images`.
pub fn parse_with_images(source: &str, images: &Images) -> Result<Question, ParseError> {
    let (prompt_markdown, answers_source) =
        split_at_rule(source).ok_or(ParseError::MissingSeparator)?;

    let prompt_markdown = prompt_markdown.trim();
    if prompt_markdown.is_empty() {
        return Err(ParseError::EmptyPrompt);
    }

    let items = top_level_items(answers_source);
    match items.len() {
        0 => return Err(ParseError::NoAnswers),
        1 => return Err(ParseError::TooFewAnswers),
        n if n > MAX_ANSWERS => return Err(ParseError::TooManyAnswers(n)),
        _ => {}
    }

    let answers: Vec<Answer> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let (markdown, points) = take_points(item);
            if markdown.is_empty() {
                return Err(ParseError::EmptyAnswer(label(i)));
            }
            Ok(Answer {
                label: label(i),
                html: render_answer(markdown, images)?,
                plain: plain_text(markdown),
                markdown: markdown.to_owned(),
                points,
            })
        })
        .collect::<Result<_, _>>()?;

    let scored = answers.iter().any(|answer| answer.points.is_some());
    if let Some(answer) = answers
        .iter()
        .find(|answer| scored && answer.points.is_none())
    {
        return Err(ParseError::UnmarkedAnswer(answer.label.clone()));
    }

    Ok(Question {
        title: first_heading(prompt_markdown),
        prompt_html: render(prompt_markdown, images)?,
        prompt_markdown: prompt_markdown.to_owned(),
        answers,
    })
}

/// The local images the Markdown shows, i.e. the paths that have to be sent
/// as [`Images`] with it, each once and in the order they first appear.
pub fn local_images(source: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for event in Parser::new_ext(source, options()) {
        if let Event::Start(Tag::Image { dest_url, .. }) = event
            && is_local(&dest_url)
            && !paths.iter().any(|path| *path == *dest_url)
        {
            paths.push(dest_url.into_string());
        }
    }
    paths
}

/// Whether an image is a file next to the question, rather than something
/// with a URL of its own: `https://...`, `data:...` or `//host/...`.
fn is_local(url: &str) -> bool {
    if url.starts_with("//") {
        return false;
    }
    let Some((scheme, _)) = url.split_once(':') else {
        return true;
    };
    // A scheme starts with a letter and goes on with letters, digits, `+`,
    // `-` and `.`; a colon after anything else is part of a path.
    let mut chars = scheme.chars();
    !(chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)))
}

/// The `data:` URL of a local image.
fn data_url(path: &str, images: &Images) -> Result<String, ParseError> {
    let image = images
        .get(path)
        .ok_or_else(|| ParseError::MissingImage(path.to_owned()))?;
    // Both end up in an attribute of the HTML, so neither may contain
    // anything but what it is supposed to.
    let known = IMAGE_TYPES
        .iter()
        .any(|(_, known)| *known == image.media_type);
    let base64 = image
        .data
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b));
    if !known || !base64 {
        return Err(ParseError::BadImage(path.to_owned()));
    }
    Ok(format!("data:{};base64,{}", image.media_type, image.data))
}

/// Puts the possible answers in a random order.
///
/// The labels are handed out again afterwards, so `A` is still the first
/// answer on the screen and everybody - the projector, every phone and the
/// CSV - sees the same order. Asking the same file again shuffles anew,
/// which is the point: the correct answer is not always `B`.
pub fn shuffle_answers(question: &mut Question) {
    question.answers.shuffle(&mut rand::rng());
    for (index, answer) in question.answers.iter_mut().enumerate() {
        answer.label = label(index);
    }
}

/// The label of the `index`-th answer: `A`, `B`, `C`, ...
pub fn label(index: usize) -> String {
    debug_assert!(index < MAX_ANSWERS);
    char::from(b'A' + (index % MAX_ANSWERS) as u8).to_string()
}

impl Question {
    /// Title to show when the file contains no heading.
    pub fn title_or<'a>(&'a self, fallback: &'a str) -> &'a str {
        self.title.as_deref().unwrap_or(fallback)
    }

    /// Whether the answers carry points.
    pub fn is_scored(&self) -> bool {
        self.answers.iter().any(|answer| answer.points.is_some())
    }

    /// Plain-text answers, in order; handy for CSV headers and terminal output.
    pub fn plain_answers(&self) -> Vec<String> {
        self.answers.iter().map(|a| a.plain.clone()).collect()
    }

    /// Bytes of text the question holds, in all its forms: the Markdown, the
    /// HTML rendered from it and the plain text of the answers.
    pub fn size(&self) -> usize {
        let answers: usize = self
            .answers
            .iter()
            .map(|a| a.label.len() + a.markdown.len() + a.html.len() + a.plain.len())
            .sum();
        self.title.as_ref().map_or(0, String::len)
            + self.prompt_markdown.len()
            + self.prompt_html.len()
            + answers
    }
}

/// Splits the source at the first horizontal rule that is not inside a fenced
/// code block. Returns the text before and after the rule line.
fn split_at_rule(source: &str) -> Option<(&str, &str)> {
    let mut fence: Option<(char, usize)> = None;
    let mut offset = 0;

    for line in source.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let line = line.trim_end_matches(['\n', '\r']);

        match fence {
            Some((marker, len)) => {
                if closes_fence(line, marker, len) {
                    fence = None;
                }
            }
            None => {
                if let Some(open) = opens_fence(line) {
                    fence = Some(open);
                } else if is_thematic_break(line) {
                    return Some((&source[..line_start], &source[offset..]));
                }
            }
        }
    }
    None
}

/// Indentation of at most three spaces keeps a line out of an indented code
/// block, which is a precondition for both fences and thematic breaks.
fn strip_indent(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() <= 3 {
        Some(rest)
    } else {
        None
    }
}

fn opens_fence(line: &str) -> Option<(char, usize)> {
    let rest = strip_indent(line)?;
    let marker = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = rest.chars().take_while(|c| *c == marker).count();
    // An info string must not contain backticks, but we do not care here: any
    // run of three or more markers opens a block for our purposes.
    (len >= 3).then_some((marker, len))
}

fn closes_fence(line: &str, marker: char, open_len: usize) -> bool {
    let Some(rest) = strip_indent(line) else {
        return false;
    };
    let len = rest.chars().take_while(|c| *c == marker).count();
    len >= open_len && rest[len..].trim().is_empty()
}

/// `---`, `***`, `___`, also with spaces in between, as per CommonMark.
fn is_thematic_break(line: &str) -> bool {
    let Some(rest) = strip_indent(line) else {
        return false;
    };
    let Some(marker) = rest.chars().next().filter(|c| "-*_".contains(*c)) else {
        return false;
    };
    let mut count = 0;
    for c in rest.chars() {
        if c == marker {
            count += 1;
        } else if !c.is_whitespace() {
            return false;
        }
    }
    count >= 3
}

/// Source text of every top-level list item in `source`, with the bullet
/// removed. Nested lists stay part of the item they belong to.
fn top_level_items(source: &str) -> Vec<String> {
    let mut depth = 0usize;
    let mut items = Vec::new();

    for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::List(_)) => depth += 1,
            Event::End(TagEnd::List(_)) => depth = depth.saturating_sub(1),
            // The range of a list item spans the whole item, including any
            // nested blocks, and `depth` is still 1 when the item starts.
            Event::Start(Tag::Item) if depth == 1 => items.push(dedent_item(&source[range])),
            _ => {}
        }
    }
    items
}

/// Removes the marks that say how many points an answer is worth: a leading
/// task-list checkbox and a trailing `{points}`. Returns the rest of the
/// answer and its points, if it has any marks.
fn take_points(item: &str) -> (&str, Option<f64>) {
    let mut points = None;
    let mut rest = item;

    for (checkbox, value) in [("[x]", 1.0), ("[X]", 1.0), ("[ ]", 0.0)] {
        if let Some(after) = rest.strip_prefix(checkbox)
            && (after.is_empty() || after.starts_with(char::is_whitespace))
        {
            points = Some(value);
            rest = after.trim_start();
            break;
        }
    }

    // `{...}` counts only as a separate word at the very end, so that text
    // such as `a{2}` in a regular expression stays what it is.
    if let Some(before) = rest.strip_suffix('}')
        && let Some((text, number)) = before.rsplit_once('{')
        && (text.is_empty() || text.ends_with(char::is_whitespace))
        && let Some(value) = parse_points(number)
    {
        points = Some(value);
        rest = text.trim_end();
    }

    (rest, points)
}

/// `1`, `+2`, `-0.5`, ...; nothing that `f64` would also accept, such as
/// `inf` or `1e3`.
fn parse_points(number: &str) -> Option<f64> {
    let digits = number.strip_prefix(['+', '-']).unwrap_or(number);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, "0"));
    let is_digits = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
    if !is_digits(whole) || !is_digits(fraction) {
        return None;
    }
    number.parse().ok()
}

/// Removes the list bullet from the first line and the corresponding
/// indentation from the continuation lines.
fn dedent_item(item: &str) -> String {
    let mut lines = item.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };

    let after_indent = first.trim_start();
    let marker_len = bullet_len(after_indent).unwrap_or(0);
    let content_column = first.len() - after_indent.len() + marker_len;
    let mut out = after_indent[marker_len..].to_owned();

    for line in lines {
        out.push('\n');
        let strippable = line
            .char_indices()
            .take_while(|(i, c)| *c == ' ' && *i < content_column)
            .count();
        out.push_str(&line[strippable..]);
    }
    out.trim_end().to_owned()
}

/// Length of a `-`/`*`/`+`/`1.` bullet including the whitespace after it.
fn bullet_len(line: &str) -> Option<usize> {
    let mut chars = line.char_indices();
    let (_, first) = chars.next()?;

    let after_marker = if "-*+".contains(first) {
        1
    } else if first.is_ascii_digit() {
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        let delimiter = line[digits..].chars().next()?;
        if delimiter != '.' && delimiter != ')' {
            return None;
        }
        digits + 1
    } else {
        return None;
    };

    let spaces = line[after_marker..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .count();
    (spaces > 0 || line.len() == after_marker).then_some(after_marker + spaces)
}

fn render(markdown: &str, images: &Images) -> Result<String, ParseError> {
    let mut html = String::new();
    html::push_html(&mut html, rendered_events(markdown, images)?.into_iter());
    Ok(html)
}

/// The Markdown event stream, with every code block replaced by the HTML from
/// [`crate::highlight`], which is where the syntax highlighting happens.
///
/// Raw HTML written in the Markdown is turned into text, so that it ends up
/// escaped in the output: a question is written by a teacher, but it is shown
/// to everybody in the room, and `<script>` in a question must not run in
/// their browsers. The only HTML in the stream is the one this function puts
/// there itself.
///
/// Local images get their `data:` URL from `images`.
fn rendered_events<'a>(markdown: &'a str, images: &Images) -> Result<Vec<Event<'a>>, ParseError> {
    let mut events = Vec::new();
    // Language and contents of the code block currently being collected.
    let mut code: Option<(String, String)> = None;

    for event in events_without_comments(markdown) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match &kind {
                    // The info string may carry more than the language, as in
                    // "```bash {.numberLines}".
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((language, String::new()));
            }
            Event::Text(text) if code.is_some() => {
                if let Some((_, contents)) = &mut code {
                    contents.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((language, contents)) = code.take() {
                    events.push(Event::Html(
                        highlight::code_block(&language, &contents).into(),
                    ));
                }
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) if is_local(&dest_url) => events.push(Event::Start(Tag::Image {
                link_type,
                dest_url: data_url(&dest_url, images)?.into(),
                title,
                id,
            })),
            // Raw HTML, escaped rather than dropped, so that a teacher sees
            // what they wrote instead of the text silently disappearing.
            Event::Html(html) => events.push(Event::Text(html)),
            Event::InlineHtml(html) => events.push(Event::Text(html)),
            other => events.push(other),
        }
    }
    Ok(events)
}

/// The Markdown event stream without HTML comments (`<!-- ... -->`), which
/// are notes for the teacher, not for the people in the room.
///
/// The parser hands out an HTML block line by line, so a block is put
/// together first; this way a comment spanning several lines goes away
/// as a whole. A block that held nothing but comments disappears entirely.
fn events_without_comments(markdown: &str) -> Vec<Event<'_>> {
    let mut events = Vec::new();
    // Contents of the HTML block currently being collected.
    let mut block: Option<String> = None;

    for event in Parser::new_ext(markdown, options()) {
        match event {
            Event::Start(Tag::HtmlBlock) => block = Some(String::new()),
            Event::Html(html) if block.is_some() => {
                if let Some(contents) = &mut block {
                    contents.push_str(&html);
                }
            }
            Event::End(TagEnd::HtmlBlock) => {
                let html = strip_comments(&block.take().unwrap_or_default());
                if !html.trim().is_empty() {
                    events.push(Event::Start(Tag::HtmlBlock));
                    events.push(Event::Html(html.into()));
                    events.push(Event::End(TagEnd::HtmlBlock));
                }
            }
            Event::InlineHtml(html) => {
                let html = strip_comments(&html);
                if !html.is_empty() {
                    events.push(Event::InlineHtml(html.into()));
                }
            }
            other => events.push(other),
        }
    }
    events
}

/// Removes every `<!-- ... -->` from `html`. An unterminated comment runs to
/// the end, as it does in a browser.
fn strip_comments(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = match rest[start + 4..].find("-->") {
            Some(end) => &rest[start + 4 + end + 3..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// Renders an answer, unwrapping the paragraph of a single-paragraph answer so
/// that it can be placed inside a label or a table cell.
fn render_answer(markdown: &str, images: &Images) -> Result<String, ParseError> {
    let html = render(markdown, images)?;
    let trimmed = html.trim();
    if let Some(inner) = trimmed
        .strip_prefix("<p>")
        .and_then(|rest| rest.strip_suffix("</p>"))
        .filter(|inner| !inner.contains("<p>"))
    {
        return Ok(inner.to_owned());
    }
    Ok(trimmed.to_owned())
}

/// Collapses Markdown to a single line of plain text.
///
/// An image is its description, or the name of its file when it has none,
/// so that an answer that is nothing but an image still has a text.
fn plain_text(markdown: &str) -> String {
    let mut text = String::new();
    // Where the description of the current image starts, and its file name.
    let mut image: Option<(usize, String)> = None;
    for event in events_without_comments(markdown) {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) => {
                image = Some((text.len(), file_name(&dest_url)));
            }
            Event::End(TagEnd::Image) => {
                if let Some((start, name)) = image.take()
                    && text[start..].trim().is_empty()
                {
                    text.push_str(&name);
                }
            }
            Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => {
                text.push_str(&t)
            }
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            Event::End(TagEnd::Paragraph | TagEnd::Item | TagEnd::CodeBlock) => text.push(' '),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The last segment of an image's URL, `image` for a `data:` URL.
fn file_name(url: &str) -> String {
    if url.starts_with("data:") {
        return "image".to_owned();
    }
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    if name.is_empty() { "image" } else { name }.to_owned()
}

fn first_heading(markdown: &str) -> Option<String> {
    let mut parser = Parser::new_ext(markdown, options());
    while let Some(event) = parser.next() {
        if !matches!(event, Event::Start(Tag::Heading { .. })) {
            continue;
        }
        let mut title = String::new();
        for event in parser.by_ref() {
            match event {
                Event::Text(t) | Event::Code(t) => title.push_str(&t),
                Event::End(TagEnd::Heading(_)) => break,
                _ => {}
            }
        }
        let title = title.trim();
        if !title.is_empty() {
            return Some(title.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A question in the shape the teachers write them. The example shipped
    /// with the app is checked by the integration tests instead, which have
    /// the file itself at hand.
    const EXAMPLE: &str = "\
Consider the following BASH code:

``` bash
sum=0
cat numbers.txt | while read -r num; do
    sum=$((sum + num))
done
echo \"Total: $sum\"
```

What's wrong with it?

---

- In Bash, every stage of a pipeline runs in its own subshell.
  Modifications to variables inside the `while` loop affect the
  subshell, not the parent shell environment.

- It doesn't handle errors.

- It will work only in BASH 4.0 and newer.
";

    #[test]
    fn parses_the_example_question() {
        let q = parse(EXAMPLE).unwrap();
        assert_eq!(q.title, None);
        assert!(
            q.prompt_markdown
                .starts_with("Consider the following BASH code:")
        );
        assert!(q.prompt_markdown.ends_with("What's wrong with it?"));
        // The fenced code block survives into the HTML.
        assert!(q.prompt_html.contains("<code class=\"language-bash\">"));
        assert!(highlight::text_of(&q.prompt_html).contains("sum=$((sum + num))"));

        let labels: Vec<_> = q.answers.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["A", "B", "C"]);
        // A multi-line answer becomes a single line of HTML and plain text.
        assert!(q.answers[0].html.contains("<code>while</code>"));
        assert_eq!(
            q.answers[0].plain,
            "In Bash, every stage of a pipeline runs in its own subshell. \
             Modifications to variables inside the while loop affect the \
             subshell, not the parent shell environment."
        );
        assert_eq!(q.answers[1].plain, "It doesn't handle errors.");
        assert_eq!(
            q.answers[2].plain,
            "It will work only in BASH 4.0 and newer."
        );
    }

    #[test]
    fn code_blocks_keep_their_language_and_their_contents() {
        let q = parse("```c\nint x = 1 < 2;\n```\n\nQ?\n\n---\n\n- a\n- b\n").unwrap();
        assert!(
            q.prompt_html.contains("<code class=\"language-c\">"),
            "{}",
            q.prompt_html
        );
        // The `<` survived both the escaping and the highlighting.
        assert!(
            highlight::text_of(&q.prompt_html).contains("int x = 1 < 2;"),
            "{}",
            q.prompt_html
        );

        // An unlabelled block is rendered as well, just without highlighting.
        let q = parse("```\nplain\n```\n\nQ?\n\n---\n\n- a\n- b\n").unwrap();
        assert!(
            q.prompt_html.contains("<pre class=\"code\"><code>"),
            "{}",
            q.prompt_html
        );
        assert!(
            highlight::text_of(&q.prompt_html).contains("plain"),
            "{}",
            q.prompt_html
        );

        // Code in an answer is rendered the same way.
        let q = parse("Q?\n\n---\n\n- one\n\n  ```c\n  return 0;\n  ```\n\n- two\n").unwrap();
        assert!(
            q.answers[0].html.contains("language-c"),
            "{}",
            q.answers[0].html
        );
        assert_eq!(q.answers[0].plain, "one return 0;");
    }

    #[test]
    fn raw_html_in_a_question_is_escaped() {
        let q = parse(
            "Q <img src=x onerror=alert(1)>\n\n<script>alert(document.cookie)</script>\n\n\
             ---\n\n- <b onmouseover=alert(2)>bold</b>\n- plain\n",
        )
        .unwrap();

        assert!(!q.prompt_html.contains("<img"), "{}", q.prompt_html);
        assert!(!q.prompt_html.contains("<script"), "{}", q.prompt_html);
        assert!(
            q.prompt_html
                .contains("&lt;script&gt;alert(document.cookie)"),
            "{}",
            q.prompt_html
        );

        let answer = &q.answers[0];
        assert!(!answer.html.contains("<b "), "{}", answer.html);
        assert!(
            answer.html.contains("&lt;b onmouseover="),
            "{}",
            answer.html
        );
        assert_eq!(answer.plain, "<b onmouseover=alert(2)>bold</b>");
    }

    #[test]
    fn html_comments_are_left_out() {
        let q = parse(
            "<!-- a note\n\nfor the teacher -->\n\nQ <!-- inline --> here?\n\n\
             <!-- x --> <b>kept</b>\n\n---\n\n- one <!-- why -->\n- two\n",
        )
        .unwrap();

        assert!(!q.prompt_html.contains("note"), "{}", q.prompt_html);
        assert!(!q.prompt_html.contains("teacher"), "{}", q.prompt_html);
        assert!(!q.prompt_html.contains("inline"), "{}", q.prompt_html);
        assert!(!q.prompt_html.contains("&lt;!--"), "{}", q.prompt_html);
        assert!(
            q.prompt_html.contains("<p>Q  here?</p>"),
            "{}",
            q.prompt_html
        );
        // Other raw HTML next to a comment is still escaped.
        assert!(q.prompt_html.contains("&lt;b&gt;kept"), "{}", q.prompt_html);

        assert!(!q.answers[0].html.contains("why"), "{}", q.answers[0].html);
        assert_eq!(q.answers[0].plain, "one");
    }

    #[test]
    fn takes_the_title_from_the_first_heading() {
        let q = parse("# Pipelines\n\nWhich one?\n\n---\n\n- one\n- two\n").unwrap();
        assert_eq!(q.title.as_deref(), Some("Pipelines"));
        assert_eq!(q.title_or("fallback"), "Pipelines");
        assert_eq!(
            parse("Q?\n\n---\n\n- a\n- b\n")
                .unwrap()
                .title_or("file.md"),
            "file.md"
        );
    }

    #[test]
    fn ignores_a_rule_inside_a_code_block() {
        let q =
            parse("Look:\n\n```\n---\nnot a separator\n---\n```\n\n---\n\n- yes\n- no\n").unwrap();
        assert_eq!(q.answers.len(), 2);
        assert!(q.prompt_markdown.contains("not a separator"));
    }

    #[test]
    fn splits_at_a_rule_that_follows_a_paragraph_directly() {
        // CommonMark would read this as a setext heading; teachers mean a rule.
        let q = parse("Which one?\n---\n- one\n- two\n").unwrap();
        assert_eq!(q.prompt_markdown, "Which one?");
        assert_eq!(q.answers.len(), 2);
    }

    #[test]
    fn accepts_other_rule_and_bullet_styles() {
        let q = parse("Q?\n\n* * *\n\n1. first\n2. second\n3. third\n").unwrap();
        assert_eq!(q.answers.len(), 3);
        assert_eq!(q.answers[2].plain, "third");
        assert_eq!(parse("Q?\n\n___\n\n+ a\n+ b\n").unwrap().answers.len(), 2);
    }

    #[test]
    fn nested_items_belong_to_their_parent_answer() {
        let q = parse("Q?\n\n---\n\n- first\n  - nested\n  - also nested\n- second\n").unwrap();
        assert_eq!(q.answers.len(), 2);
        assert!(q.answers[0].html.contains("<ul>"));
        assert_eq!(q.answers[0].plain, "first nested also nested");
    }

    #[test]
    fn keeps_multi_paragraph_answers_as_blocks() {
        let q = parse("Q?\n\n---\n\n- one\n\n  still one\n\n- two\n").unwrap();
        assert_eq!(q.answers.len(), 2);
        assert!(q.answers[0].html.starts_with("<p>one</p>"));
        assert_eq!(q.answers[0].plain, "one still one");
    }

    #[test]
    fn rejects_unusable_files() {
        assert_eq!(
            parse("Just a question?\n").unwrap_err(),
            ParseError::MissingSeparator
        );
        assert_eq!(
            parse("Q?\n\n---\n\nnot a list\n").unwrap_err(),
            ParseError::NoAnswers
        );
        assert_eq!(
            parse("Q?\n\n---\n\n- lonely\n").unwrap_err(),
            ParseError::TooFewAnswers
        );
        assert_eq!(
            parse("\n---\n\n- a\n- b\n").unwrap_err(),
            ParseError::EmptyPrompt
        );

        let many = format!("Q?\n\n---\n\n{}", "- answer\n".repeat(27));
        assert_eq!(parse(&many).unwrap_err(), ParseError::TooManyAnswers(27));
    }

    #[test]
    fn shuffling_keeps_every_answer_and_hands_the_labels_out_again() {
        let source = format!(
            "Q?\n\n---\n\n{}",
            (0..8).fold(String::new(), |mut list, i| {
                list.push_str(&format!("- answer {i}\n"));
                list
            })
        );
        let original = parse(&source).unwrap();

        let mut orders = std::collections::HashSet::new();
        for _ in 0..50 {
            let mut question = original.clone();
            shuffle_answers(&mut question);

            // The labels are always A, B, C, ... down the screen.
            let labels: Vec<_> = question.answers.iter().map(|a| a.label.clone()).collect();
            assert_eq!(labels, (0..8).map(label).collect::<Vec<_>>());

            // Every answer is still there, exactly once, with its own text.
            let mut texts: Vec<_> = question.answers.iter().map(|a| a.plain.clone()).collect();
            orders.insert(texts.clone());
            texts.sort();
            let mut expected: Vec<_> = original.answers.iter().map(|a| a.plain.clone()).collect();
            expected.sort();
            assert_eq!(texts, expected);
            for answer in &question.answers {
                assert!(answer.html.contains(&answer.plain), "{answer:?}");
            }
        }

        // 8 answers, 50 tries: one single order would not be a shuffle.
        assert!(orders.len() > 1, "the order never changed");
    }

    #[test]
    fn checkboxes_and_braces_give_the_points() {
        let q = parse(
            "Q?\n\n---\n\n- [x] right\n- [ ] wrong\n- [X] partly {0.5}\n\
             - [ ] very wrong {-1}\n- also right {+2}\n",
        )
        .unwrap();
        assert!(q.is_scored());
        let points: Vec<_> = q.answers.iter().map(|a| a.points).collect();
        assert_eq!(
            points,
            [Some(1.0), Some(0.0), Some(0.5), Some(-1.0), Some(2.0)]
        );
        let correct: Vec<_> = q.answers.iter().map(Answer::is_correct).collect();
        assert_eq!(correct, [true, false, true, false, true]);

        // The marks are not part of what students see.
        for answer in &q.answers {
            assert!(!answer.markdown.contains(['[', '{']), "{answer:?}");
            assert!(!answer.html.contains("checkbox"), "{answer:?}");
        }
        assert_eq!(q.answers[2].plain, "partly");
        assert_eq!(q.answers[3].html, "very wrong");
    }

    #[test]
    fn points_of_a_multi_line_answer_are_at_its_end() {
        let q = parse(
            "Q?\n\n---\n\n- [x] one\n  line more\n  {3}\n\n\
             - [ ] two\n\n  ```\n  a{2}\n  ```\n",
        )
        .unwrap();
        assert_eq!(q.answers[0].points, Some(3.0));
        assert_eq!(q.answers[0].plain, "one line more");
        // The braces in the code block are code.
        assert_eq!(q.answers[1].points, Some(0.0));
        assert!(q.answers[1].plain.contains("a{2}"));
    }

    #[test]
    fn braces_that_are_no_points_stay_in_the_text() {
        let q = parse("Q?\n\n---\n\n- a{2}\n- {x}\n- {1e3}\n- `{1}`\n").unwrap();
        assert!(!q.is_scored());
        let texts: Vec<_> = q.answers.iter().map(|a| a.plain.as_str()).collect();
        assert_eq!(texts, ["a{2}", "{x}", "{1e3}", "{1}"]);
        // A checkbox needs a space after it to be one.
        assert!(!parse("Q?\n\n---\n\n- [x]y\n- b\n").unwrap().is_scored());
    }

    #[test]
    fn either_all_answers_are_marked_or_none() {
        assert_eq!(
            parse("Q?\n\n---\n\n- [x] right\n- forgotten\n").unwrap_err(),
            ParseError::UnmarkedAnswer("B".into())
        );
        assert_eq!(
            parse("Q?\n\n---\n\n- [x] right\n- [ ]\n").unwrap_err(),
            ParseError::EmptyAnswer("B".into())
        );
        assert!(!parse(EXAMPLE).unwrap().is_scored());
    }

    #[test]
    fn shuffled_answers_keep_their_points() {
        let mut q = parse("Q?\n\n---\n\n- [x] right\n- [ ] wrong\n- [ ] bad {-1}\n").unwrap();
        for _ in 0..10 {
            shuffle_answers(&mut q);
            for answer in &q.answers {
                let expected = match answer.plain.as_str() {
                    "right" => 1.0,
                    "wrong" => 0.0,
                    _ => -1.0,
                };
                assert_eq!(answer.points, Some(expected));
            }
        }
    }

    fn svg(data: &str) -> Image {
        Image {
            media_type: "image/svg+xml".to_owned(),
            data: data.to_owned(),
        }
    }

    #[test]
    fn local_images_become_data_urls() {
        let source = "Which?\n\n![Diagram](img/diagram.svg)\n\n---\n\n\
                      - ![](img/a.svg)\n- ![Blue](img/b.svg \"title\")\n\
                      - ![](https://example.com/c.png)\n- ![again](img/a.svg)\n";
        assert_eq!(
            local_images(source),
            ["img/diagram.svg", "img/a.svg", "img/b.svg"]
        );

        let images = Images::from([
            ("img/diagram.svg".to_owned(), svg("RA==")),
            ("img/a.svg".to_owned(), svg("QQ==")),
            ("img/b.svg".to_owned(), svg("Qg==")),
        ]);
        let q = parse_with_images(source, &images).unwrap();
        assert!(
            q.prompt_html
                .contains(r#"<img src="data:image/svg+xml;base64,RA==" alt="Diagram" />"#),
            "{}",
            q.prompt_html
        );
        assert_eq!(
            q.answers[0].html,
            r#"<img src="data:image/svg+xml;base64,QQ==" alt="" />"#
        );
        assert!(q.answers[1].html.contains(r#"title="title""#), "{q:?}");
        // A remote image is the browser's business.
        assert!(
            q.answers[2]
                .html
                .contains(r#"src="https://example.com/c.png""#),
            "{q:?}"
        );
        // The Markdown stays as it was written.
        assert!(q.prompt_markdown.contains("(img/diagram.svg)"));

        // An image without a description goes by its file name in plain text.
        let plain: Vec<_> = q.answers.iter().map(|a| a.plain.as_str()).collect();
        assert_eq!(plain, ["a.svg", "Blue", "c.png", "again"]);
    }

    #[test]
    fn local_images_have_to_be_there_and_be_images() {
        let source = "Q ![x](x.svg)?\n\n---\n\n- a\n- b\n";
        assert_eq!(
            parse(source).unwrap_err(),
            ParseError::MissingImage("x.svg".into())
        );

        let images = |image| Images::from([("x.svg".to_owned(), image)]);
        let html = Image {
            media_type: "text/html".to_owned(),
            data: "PGI+".to_owned(),
        };
        assert_eq!(
            parse_with_images(source, &images(html)).unwrap_err(),
            ParseError::BadImage("x.svg".into())
        );
        assert_eq!(
            parse_with_images(source, &images(svg("\" onerror=\"alert(1)"))).unwrap_err(),
            ParseError::BadImage("x.svg".into())
        );
    }

    #[test]
    fn tells_local_images_from_urls() {
        for local in [
            "a.png",
            "img/a.png",
            "../a.png",
            "/abs/a.png",
            "dir/a:b.png",
        ] {
            assert!(is_local(local), "{local}");
        }
        for remote in ["https://x/a.png", "data:image/png;base64,AA==", "//x/a.png"] {
            assert!(!is_local(remote), "{remote}");
        }
        assert_eq!(image_type("img/a.SVG"), Some("image/svg+xml"));
        assert_eq!(image_type("a.jpg"), Some("image/jpeg"));
        assert_eq!(image_type("a.txt"), None);
        assert_eq!(image_type("noextension"), None);
    }

    #[test]
    fn labels_run_from_a_to_z() {
        assert_eq!(label(0), "A");
        assert_eq!(label(25), "Z");
    }
}
