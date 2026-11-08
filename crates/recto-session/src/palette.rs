//! **⌘K's matching and order**: which of the palette's items a query finds, and in what order. Pure rules, so the app, `recto palette` and a test all rank the same way.
//!
//! A query matches an item when its letters appear in the item's words in order, case ignored, as in any Mac or editor palette: `zi` finds *Zoom In*, `hsb` *Hide Sidebar*. What matters most is that letters run together and start words, so the item the reader is spelling comes first. Every query's cost is bounded: [`MAX_QUERY_CHARS`] of it is looked at, and an item's words up to [`MAX_TEXT_CHARS`].

/// The longest query looked at. Past this nobody's spelling a command.
pub const MAX_QUERY_CHARS: usize = 32;
/// The longest part of an item looked at. A tab's name can be long, and comes from a file; what's past this isn't matched.
pub const MAX_TEXT_CHARS: usize = 128;
/// Items listed for one query.
pub const MAX_RESULTS: usize = 50;

const MATCHED: i32 = 1;
const RUN: i32 = 6;
const WORD_START: i32 = 8;
const FIRST: i32 = 12;
const GAP: i32 = 1;
/// Below any real score: no match here.
const NONE: i32 = i32::MIN / 4;

/// How well `query` matches `text`, or `None` if its letters aren't all there in order. Higher is better. Spaces in the query are ignored, so words can be typed apart or together.
///
/// **Its cost is bounded:** one pass to see the letters are there at all, and then the best placing of them by dynamic programming, [`MAX_QUERY_CHARS`] by [`MAX_TEXT_CHARS`] steps, 4,096 at most, whatever the item says.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let wanted: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).take(MAX_QUERY_CHARS).collect();
    if wanted.is_empty() {
        return Some(0);
    }
    let letters: Vec<char> = text.chars().take(MAX_TEXT_CHARS).collect();
    let lower: Vec<char> = letters.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    // Most items don't have the letters at all: one pass says so.
    let mut need = wanted.iter().peekable();
    for c in &lower {
        if need.peek() == Some(&c) {
            need.next();
        }
    }
    if need.peek().is_some() {
        return None;
    }
    let starts_word = |at: usize| at == 0 || !letters[at - 1].is_alphanumeric() || (letters[at].is_uppercase() && letters[at - 1].is_lowercase());
    let bonus = |at: usize| MATCHED + if starts_word(at) { WORD_START } else { 0 };
    // best[i]: the best score with the query's letters so far placed, the last at text position i.
    let mut best: Vec<i32> = lower.iter().enumerate().map(|(at, c)| if *c == wanted[0] { bonus(at) + if at == 0 { FIRST } else { 0 } } else { NONE }).collect();
    let mut next = vec![NONE; lower.len()];
    for want in &wanted[1..] {
        // The best earlier placing, less a point for each letter skipped since, carried along.
        let mut carried = NONE;
        for at in 0..lower.len() {
            let before = if at > 0 { best[at - 1] } else { NONE };
            next[at] = if lower[at] == *want && at > 0 {
                let run = if before > NONE { before + RUN } else { NONE };
                bonus(at) + run.max(carried)
            } else {
                NONE
            };
            carried = (carried - GAP).max(before - GAP).max(NONE);
        }
        std::mem::swap(&mut best, &mut next);
    }
    let found = best.into_iter().max().filter(|total| *total > NONE / 2)?;
    // A shorter item, all else equal, is more likely what's meant.
    Some(found * 4 - i32::try_from(letters.len()).unwrap_or(0) / 8)
}

/// The items a query finds, best first, as indexes into `items` with their scores, at most [`MAX_RESULTS`]. An item is its words to match. Equal scores keep the items' own order, so an empty query lists them as given.
pub fn rank<'a>(query: &str, items: impl IntoIterator<Item = &'a str>) -> Vec<(usize, i32)> {
    let mut scored: Vec<(i32, usize)> = items.into_iter().enumerate().filter_map(|(at, text)| score(query, text).map(|score| (score, at))).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(MAX_RESULTS).map(|(score, at)| (at, score)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a palette that finds only exact words, so `zi` doesn't find Zoom In, or that finds letters out of order.
    #[test]
    fn letters_in_order_match_and_out_of_order_dont() {
        assert!(score("zi", "Zoom In").is_some());
        assert!(score("hsb", "Hide Sidebar").is_some());
        assert!(score("zoom in", "Zoom In").is_some(), "spaces typed apart");
        assert!(score("iz", "Zoom In").is_none());
        assert!(score("q", "Zoom In").is_none());
        assert_eq!(score("", "anything"), Some(0));
    }

    /// Failure: the item being spelled comes below others that only happen to hold its letters, so Return runs the wrong command.
    #[test]
    fn what_is_being_spelled_comes_first() {
        let items = ["Close Window", "Copy", "Clear Edit History", "Zoom Out", "Show Agent Icon", "Save As…"];
        assert_eq!(items[rank("copy", items)[0].0], "Copy");
        assert_eq!(items[rank("cw", items)[0].0], "Close Window");
        assert_eq!(items[rank("save", items)[0].0], "Save As…");
        assert_eq!(items[rank("zo", items)[0].0], "Zoom Out");
        assert_eq!(rank("", items).into_iter().map(|(at, _)| at).collect::<Vec<_>>(), (0..items.len()).collect::<Vec<_>>(), "nothing typed keeps the given order");
    }

    /// Failure: a hostile tab name, very long or full of the query's letters, makes ranking slow on the UI thread (review: 0.53 ms an item with a 256-letter name and a 64-letter query, so seconds over 10,000 tabs), or a list holds every item there is.
    #[test]
    fn its_cost_and_its_list_are_bounded() {
        let long = "a".repeat(100_000);
        assert!(score(&"a".repeat(10_000), &long).is_some());
        let hostile: Vec<String> = (0..10_000).map(|_| "ab".repeat(128)).collect();
        let query = "ab".repeat(32);
        let started = std::time::Instant::now();
        let found = rank(&query, hostile.iter().map(String::as_str));
        let took = started.elapsed();
        assert_eq!(found.len(), MAX_RESULTS);
        // About 4,096 steps an item: a few milliseconds a thousand in a release build. Generous, for a debug build under load.
        assert!(took < std::time::Duration::from_secs(5), "{took:?} for 10,000 hostile names");
        let many: Vec<String> = (0..1000).map(|at| format!("tab {at}")).collect();
        assert_eq!(rank("tab", many.iter().map(String::as_str)).len(), MAX_RESULTS);
    }
}
