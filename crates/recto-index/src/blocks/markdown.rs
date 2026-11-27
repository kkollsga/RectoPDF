//! **A run of blocks as Markdown**, for an agent to read. Rust decides every word and mark of it, and the document decides none of the structure:
//! text from a page is data, so a line of it can't become a heading, a fence, a list, a quote or a table just by how it starts.
//!
//! - A heading is `#` as many times as its contents depth says plus one (so a top-level entry is `#`), clamped to 1 to 6; without a depth, its level.
//! - A list item is `- `, and its further lines are indented to stay in it.
//! - Code is fenced with backticks, one more than the longest run of backticks inside it (at least three), so nothing in it can end the fence.
//! - Paragraphs, captions and footnotes are paragraphs.
//! - **Document text is escaped:** in text (not in code) `\`, `[`, `]`, `<` and a backtick get a backslash wherever they are, so a page can't make a link, an image, an autolink, HTML, inline
//!   code or a reference, or take an escape away with a backslash of its own. A line's first character (after the space the line began with, which is dropped: four spaces would make code) gets one too when it is one
//!   of `# > - + * ~ | = _ ! :`, or digits before a `.` or `)`. In code a `[hidden:` is escaped, so a page can't forge the mark below.
//! - **Hidden text is marked as `recto text` marks it:** its words are inside `[hidden: ...]`, in place, with `]` and `\` escaped inside so it can't close early.

use super::{Block, Kind};

/// One block to render, with its text from the page.
#[derive(Clone, Copy, Debug)]
pub struct Piece<'a> {
    pub kind: Kind,
    /// The block's own level, used for a heading with no contents depth.
    pub level: u8,
    /// The depth of the contents entry this heading is (0 for a chapter), if it is one.
    pub depth: Option<u8>,
    /// The block's characters of the page's text.
    pub text: &'a [char],
    /// Hidden stretches, in characters of `text`, `[start, end)`: see [`spans_within`].
    pub hidden: &'a [(u32, u32)],
}

/// The page's hidden spans that fall inside a block, moved to count from the block's start.
#[must_use]
pub fn spans_within(block: &Block, page_spans: &[(u32, u32)]) -> Vec<(u32, u32)> {
    page_spans
        .iter()
        .filter_map(|&(start, end)| {
            let (from, to) = (start.max(block.start), end.min(block.end));
            (from < to).then_some((from - block.start, to - block.start))
        })
        .collect()
}

/// The `#` count of a heading: its contents depth plus one, else its level, in 1 to 6.
#[must_use]
pub fn heading_hashes(depth: Option<u8>, level: u8) -> usize {
    depth.map_or(usize::from(level), |depth| usize::from(depth) + 1).clamp(1, 6)
}

/// The pieces as Markdown, one block after another with a blank line between. A block with nothing but space in it leaves no trace.
#[must_use]
pub fn render(pieces: &[Piece<'_>]) -> String {
    let mut out = String::new();
    for piece in pieces {
        let Some(block) = render_piece(piece) else { continue };
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&block);
    }
    out
}

fn render_piece(piece: &Piece<'_>) -> Option<String> {
    let lines = lines_of(piece, piece.kind != Kind::Code);
    if lines.iter().all(String::is_empty) {
        return None;
    }
    Some(match piece.kind {
        Kind::Heading => format!(
            "{} {}",
            "#".repeat(heading_hashes(piece.depth, piece.level)),
            lines.iter().filter(|line| !line.is_empty()).cloned().collect::<Vec<_>>().join(" ")
        ),
        Kind::ListItem => {
            let mut text = String::from("- ");
            text.push_str(&lines.join("\n  "));
            text
        }
        Kind::Code => {
            let body = lines.join("\n");
            let fence = "`".repeat(longest_run(&body, '`').max(2) + 1);
            format!("{fence}\n{body}\n{fence}")
        }
        Kind::Paragraph | Kind::Caption | Kind::Footnote => lines.join("\n"),
    })
}

fn longest_run(text: &str, ch: char) -> usize {
    let (mut best, mut run) = (0, 0);
    for c in text.chars() {
        run = if c == ch { run + 1 } else { 0 };
        best = best.max(run);
    }
    best
}

/// The block's lines, each rendered: line ends other than `\n` are spaces; with `escape` on (not in code), space at a line's ends is dropped and its start is escaped; hidden stretches are marked either way.
fn lines_of(piece: &Piece<'_>, escape: bool) -> Vec<String> {
    let mut hidden = vec![false; piece.text.len()];
    for &(start, end) in piece.hidden {
        let end = (end as usize).min(hidden.len());
        for flag in hidden.iter_mut().take(end).skip(start as usize) {
            *flag = true;
        }
    }
    let mut lines: Vec<Vec<(char, bool)>> = vec![Vec::new()];
    for (&ch, &is_hidden) in piece.text.iter().zip(&hidden) {
        if ch == '\n' {
            lines.push(Vec::new());
            continue;
        }
        let ch = if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') { ' ' } else { ch };
        if let Some(line) = lines.last_mut() {
            line.push((ch, is_hidden));
        }
    }
    lines.iter().map(|line| render_line(line, escape)).collect()
}

fn render_line(line: &[(char, bool)], escape: bool) -> String {
    let mut line = line;
    if escape {
        while let [(c, _), rest @ ..] = line
            && c.is_whitespace()
        {
            line = rest;
        }
        while let [rest @ .., (c, _)] = line
            && c.is_whitespace()
        {
            line = rest;
        }
    }
    let mut out = String::new();
    let mut at = 0;
    while at < line.len() {
        let is_hidden = line[at].1;
        let run_end = line[at..].iter().position(|(_, flag)| *flag != is_hidden).map_or(line.len(), |len| at + len);
        let run: String = line[at..run_end].iter().map(|(c, _)| *c).collect();
        if is_hidden {
            out.push_str("[hidden: ");
            out.push_str(&inline(run.trim_end(), escape));
            out.push(']');
        } else {
            let mut visible = inline(&run, escape);
            if at == 0 && escape {
                visible = escape_start(&visible);
            } else if at > 0 && escape && (visible.starts_with('(') || visible.starts_with(':')) {
                // Right after the program's own `]`, `(` would make the mark a link and `:` a reference.
                visible.insert(0, '\\');
            }
            out.push_str(&visible);
        }
        at = run_end;
    }
    out
}

/// Document text made inert inside a line: `\`, `[`, `]`, `<` and a backtick get a backslash in text (so no link, image, autolink, HTML, inline code or reference, and no backslash of the page's own
/// can take the escape of the next character away). In code only a `[hidden:` is escaped: the fence holds the rest, and the mark still can't be forged.
fn inline(text: &str, escape: bool) -> String {
    if !escape {
        return text.replace("[hidden:", "\\[hidden:");
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '[' | ']' | '<' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A backslash where a line's first characters would start a block.
fn escape_start(text: &str) -> String {
    let mut chars = text.chars();
    let Some(first) = chars.next() else { return String::new() };
    if "#>-+*~|=_!:".contains(first) {
        return format!("\\{text}");
    }
    if first.is_ascii_digit() {
        let digits = text.chars().take_while(char::is_ascii_digit).count();
        if let Some(delimiter) = text.chars().nth(digits)
            && (delimiter == '.' || delimiter == ')')
        {
            let split: usize = text.char_indices().nth(digits).map_or(text.len(), |(at, _)| at);
            return format!("{}\\{}", &text[..split], &text[split..]);
        }
    }
    text.to_owned()
}
