//! **⌘K's matching and order**: which of the palette's items a query finds, and in what order. Pure rules, so the app, `recto palette` and a test all rank the same way.
//!
//! A query matches an item when its letters appear in the item's words in order, case ignored, as in any Mac or editor palette: `zi` finds *Zoom In*, `hsb` *Hide Sidebar*. What matters most is that letters run together and start words, so the item the reader is spelling comes first. Every query's cost is bounded: [`MAX_QUERY_CHARS`] of it is looked at, and an item's words up to [`MAX_TEXT_CHARS`].

/// The longest query looked at. Past this nobody's spelling a command.
pub const MAX_QUERY_CHARS: usize = 64;
/// The longest part of an item looked at. A tab's name can be long, and comes from a file.
pub const MAX_TEXT_CHARS: usize = 256;
/// Items listed for one query.
pub const MAX_RESULTS: usize = 50;

const MATCHED: i32 = 1;
const RUN: i32 = 6;
const WORD_START: i32 = 8;
const FIRST: i32 = 12;
const GAP: i32 = 1;

/// How well `query` matches `text`, or `None` if its letters aren't all there in order. Higher is better. Spaces in the query are ignored, so words can be typed apart or together.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let wanted: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).take(MAX_QUERY_CHARS).flat_map(char::to_lowercase).collect();
    if wanted.is_empty() {
        return Some(0);
    }
    let letters: Vec<char> = text.chars().take(MAX_TEXT_CHARS).collect();
    let lower: Vec<char> = letters.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let starts_word = |at: usize| at == 0 || !letters[at - 1].is_alphanumeric() || (letters[at].is_uppercase() && letters[at - 1].is_lowercase());
    // Each place the first letter is could start the best match: from each, the rest is taken greedily, preferring a word's start nearby. At most MAX_TEXT_CHARS starts of MAX_TEXT_CHARS steps.
    let mut best = None;
    for start in (0..lower.len()).filter(|at| lower[*at] == wanted[0]) {
        let mut total = MATCHED + if start == 0 { FIRST } else { 0 } + if starts_word(start) { WORD_START } else { 0 };
        let mut last = start;
        let mut found = true;
        for want in &wanted[1..] {
            let next_run = lower.get(last + 1).filter(|c| *c == want).map(|_| last + 1);
            let next = next_run.or_else(|| (last + 1..lower.len()).filter(|at| lower[*at] == *want).find(|at| starts_word(*at))).or_else(|| (last + 1..lower.len()).find(|at| lower[*at] == *want));
            let Some(at) = next else {
                found = false;
                break;
            };
            total += MATCHED;
            if at == last + 1 {
                total += RUN;
            } else {
                total -= GAP * i32::try_from(at - last - 1).unwrap_or(i32::MAX / 4).min(20);
            }
            if starts_word(at) {
                total += WORD_START;
            }
            last = at;
        }
        if found {
            best = Some(best.map_or(total, |was: i32| was.max(total)));
        }
    }
    // A shorter item, all else equal, is more likely what's meant.
    best.map(|total| total * 4 - i32::try_from(letters.len()).unwrap_or(i32::MAX / 8).min(200) / 8)
}

/// The items a query finds, best first, as indexes into `items`, at most [`MAX_RESULTS`]. An item is its words to match. Equal scores keep the items' own order, so an empty query lists them as given.
pub fn rank<'a>(query: &str, items: impl IntoIterator<Item = &'a str>) -> Vec<usize> {
    let mut scored: Vec<(i32, usize)> = items.into_iter().enumerate().filter_map(|(at, text)| score(query, text).map(|score| (score, at))).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(MAX_RESULTS).map(|(_, at)| at).collect()
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
        assert_eq!(items[rank("copy", items)[0]], "Copy");
        assert_eq!(items[rank("cw", items)[0]], "Close Window");
        assert_eq!(items[rank("save", items)[0]], "Save As…");
        assert_eq!(items[rank("zo", items)[0]], "Zoom Out");
        assert_eq!(rank("", items), (0..items.len()).collect::<Vec<_>>(), "nothing typed keeps the given order");
    }

    /// Failure: a hostile tab name, very long or full of the query's letters, makes ranking slow, or a list holds every item there is.
    #[test]
    fn its_cost_and_its_list_are_bounded() {
        let long = "a".repeat(100_000);
        let started = std::time::Instant::now();
        assert!(score(&"a".repeat(10_000), &long).is_some());
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
        let many: Vec<String> = (0..1000).map(|at| format!("tab {at}")).collect();
        assert_eq!(rank("tab", many.iter().map(String::as_str)).len(), MAX_RESULTS);
    }
}
