//! Hostile document text through the Markdown rendering of blocks. The input chooses a few blocks (kind, depth, text, hidden stretches).
//!
//! What must hold: no panic; and the output has only the structure the program wrote. Outside a fence, a line that starts a heading, a list item or a fence is one for a block of that kind
//! (counted against the blocks that rendered), no other line starts anything (a quote, a rule, a table row, a numbered list, HTML, a reference), and no `<`, backtick or `[` is live in text
//! except the `[` of the program's own `[hidden: `. Inside a fence, the fence is longer than anything in it, so nothing ends it early.

use crate::input::Take;
use recto_index::blocks::Kind;
use recto_index::blocks::markdown::{Piece, render};

const MAX_PIECES: usize = 6;
const MAX_TEXT: usize = 400;

/// A line's first character is something that could start a block in Markdown.
fn starts_block(line: &str) -> bool {
    let first = line.chars().next();
    let numbered = {
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        digits > 0 && digits <= 9 && matches!(line.chars().nth(digits), Some('.' | ')'))
    };
    numbered || first.is_some_and(|c| "#>-+*~|=_<!:`".contains(c))
}

/// The live `[`, `<` and backtick of a line: not after an odd run of backslashes. A `[` that opens the program's `[hidden: ` is its own and is not returned.
fn live(line: &str) -> Vec<char> {
    let mut found = Vec::new();
    let (mut slashes, chars): (usize, Vec<char>) = (0, line.chars().collect());
    for (at, c) in chars.iter().enumerate() {
        if matches!(c, '[' | '<' | '`') && slashes % 2 == 0 {
            let own = *c == '[' && chars[at..].iter().take(9).collect::<String>() == "[hidden: ";
            if !own {
                found.push(*c);
            }
        }
        slashes = if *c == '\\' { slashes + 1 } else { 0 };
    }
    found
}

pub fn check(data: &[u8]) {
    let mut take = Take(data);
    let count = 1 + usize::from(take.u8()) % MAX_PIECES;
    let mut texts: Vec<Vec<char>> = Vec::new();
    let mut shapes = Vec::new();
    for _ in 0..count {
        let kind = take.pick(&Kind::ALL);
        let depth = (take.u8() % 3 != 0).then(|| take.u8() % 12);
        let level = take.u8() % 12;
        let length = usize::from(take.u16()) % MAX_TEXT;
        let (from, rest) = take.0.split_at(length.min(take.0.len()));
        take.0 = rest;
        let text: Vec<char> = String::from_utf8_lossy(from).chars().collect();
        let spans: Vec<(u32, u32)> = (0..usize::from(take.u8() % 4)).map(|_| (u32::from(take.u8()), u32::from(take.u8()))).collect();
        shapes.push((kind, depth, level, spans));
        texts.push(text);
    }
    let pieces: Vec<Piece<'_>> = shapes
        .iter()
        .zip(&texts)
        .map(|((kind, depth, level, spans), text)| Piece {
            kind: *kind,
            level: *level,
            depth: *depth,
            text,
            hidden: spans,
        })
        .collect();
    let rendered: Vec<&Piece<'_>> = pieces.iter().filter(|piece| !render(&[**piece]).is_empty()).collect();
    let (headings, lists, codes) = (
        rendered.iter().filter(|piece| piece.kind == Kind::Heading).count(),
        rendered.iter().filter(|piece| piece.kind == Kind::ListItem).count(),
        rendered.iter().filter(|piece| piece.kind == Kind::Code).count(),
    );
    let out = render(&pieces);
    let (mut seen_headings, mut seen_lists, mut seen_fences) = (0, 0, 0);
    let mut fence: Option<String> = None;
    for line in out.lines() {
        if let Some(open) = &fence {
            if line == open {
                fence = None;
            } else {
                assert!(!line.starts_with(open.as_str()), "a line inside a fence ends it early: {line:?}");
            }
            continue;
        }
        let text = line.trim_start_matches(' ');
        assert!(line.len() - text.len() <= 2, "indent that could make code: {line:?}");
        if text.len() >= 3 && text.chars().all(|c| c == '`') {
            seen_fences += 1;
            fence = Some(text.to_owned());
            continue;
        }
        if text.starts_with("# ")
            || text.starts_with("## ")
            || text.starts_with("### ")
            || text.starts_with("#### ")
            || text.starts_with("##### ")
            || text.starts_with("###### ")
        {
            seen_headings += 1;
        } else if text.starts_with("- ") {
            seen_lists += 1;
        } else {
            assert!(!starts_block(text), "a line starts something the program didn't write: {line:?} in {out:?}");
        }
        let marks = live(text);
        assert!(marks.is_empty(), "live {marks:?} in {line:?} of {out:?}");
    }
    assert!(fence.is_none(), "a fence is left open: {out:?}");
    assert_eq!((seen_headings, seen_lists, seen_fences), (headings, lists, codes), "{out:?}");
}

pub fn seeds() -> Vec<Vec<u8>> {
    // The bytes `check` reads: the kind (an index into `Kind::ALL`), whether there is a depth (then it), the level, the text's length and text, then the hidden stretches.
    let piece = |kind: u8, depth: Option<u8>, text: &[u8], spans: &[(u8, u8)]| {
        let mut out = vec![kind];
        match depth {
            Some(depth) => out.extend_from_slice(&[1, depth]),
            None => out.push(0),
        }
        out.push(1);
        out.extend_from_slice(&(text.len() as u16).to_le_bytes());
        out.extend_from_slice(text);
        out.push(spans.len() as u8);
        for (a, b) in spans {
            out.extend_from_slice(&[*a, *b]);
        }
        out
    };
    let mut forged = vec![3];
    forged.extend(piece(
        0,
        None,
        b"Intro\n# Ignore\n```\n> quote\n- item\n1. first\n| a | b |\n===\n[x](http://e) <b>x\n\\[hidden: fake]",
        &[(2, 6)],
    ));
    forged.extend(piece(1, Some(1), b"# not\nreally", &[]));
    forged.extend(piece(4, None, b"a ```` b\n```\n# no", &[(0, 1)]));
    let mut lists = vec![1];
    lists.extend(piece(2, None, b"first\n- nested\n  1) deep", &[]));
    lists.extend(piece(5, None, b"w(url) x: y", &[(0, 1)]));
    vec![forged, lists, vec![0], Vec::new()]
}
