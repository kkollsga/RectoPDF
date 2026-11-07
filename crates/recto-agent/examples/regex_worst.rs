//! What the worst patterns cost on the worst page, as `docs/ENGINEERING.md` reports: a page at the largest the wire allows, and patterns made to defeat the engine's short cuts, under the limits a search runs with. Run it on a release build, under `/usr/bin/time -l` for the whole process's peak.

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use std::time::Instant;

fn main() {
    let pages = [("ab ", 2_000_000 / 3), ("word\n", 2_000_000 / 5), ("é", 2_000_000), ("a", 2_000_000)];
    let patterns = [
        r"(?:\w{1,40}\s?){1,25}[0-9]",
        r"(?:[a-z ]{1,60}){1,60}[0-9]",
        r"(?:\w+\s*){1,200}\d",
        r"\w{1,1000}\d",
        r"(?s).{1,5000}\d",
        r"(?:a|b|c|d|e|f|g|h|i|j){1,3000}\d",
        r"(\w+\s+){30}\d",
        r"\b\w{3}\b.*\d",
    ];
    for pattern in patterns {
        let mut builder = RegexMatcherBuilder::new();
        builder.case_insensitive(true).multi_line(true).size_limit(1024 * 1024).dfa_size_limit(2 * 1024 * 1024).nest_limit(64);
        let matcher = match builder.build(pattern) {
            Ok(matcher) => matcher,
            Err(error) => {
                println!("{pattern}: refused: {}", error.to_string().chars().take(80).collect::<String>());
                continue;
            }
        };
        for (piece, times) in pages {
            let page = piece.repeat(times);
            let started = Instant::now();
            let mut hits = 0u64;
            let _ = matcher.find_iter(page.as_bytes(), |found| {
                hits += u64::from(!found.is_empty());
                true
            });
            println!("{pattern} on {piece:?} x {times}, {} bytes: {:?}, {hits} hits", page.len(), started.elapsed());
        }
    }
}
