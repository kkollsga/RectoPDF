//! What the worst patterns cost on the worst page, as `docs/ENGINEERING.md` reports. One pass of the engine can't be stopped: a deadline only gets a look between matches. So this measures two things under the limits a search runs with: how far past a deadline a search goes on pages at the most a pattern is run over, for patterns that match over and over, and how long the worst pass with no match takes. Run it on a release build, under `/usr/bin/time -l` for the whole process's peak.
//!
//! `regex_worst [size-limit-bytes] [page-bytes] [deadline-ms] [pattern...]`

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use std::time::{Duration, Instant};

const PATTERNS: &[&str] = &[
    r"(?:a|b|c|d|e|f|g|h|i|j){1,3000}\d|.",
    r"\w+\d|\w",
    r"(?:a|b|c|d|e|f|g|h|i|j){1,3000}\d",
    r"(?:a|b|c|d|e|f|g|h|i|j){1,500}\d",
    r"[a-z]{1,3000}\d",
    r"(?:\w+\s*){1,20}\d",
    r"\w{1,200}\d",
    r"(?s).{1,800}\d",
    r"a*a*a*a*a*a*a*a*\d",
    r"(a|aa|aaa|aaaa){1,100}\d|x*",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let size_limit = args.first().and_then(|arg| arg.parse().ok()).unwrap_or(1024 * 1024);
    let page_bytes: usize = args.get(1).and_then(|arg| arg.parse().ok()).unwrap_or(512 * 1024);
    let deadline_ms = args.get(2).and_then(|arg| arg.parse().ok()).unwrap_or(100);
    let chosen: Vec<&str> = if args.len() > 3 {
        args[3..].iter().map(String::as_str).collect()
    } else {
        PATTERNS.to_vec()
    };
    let pages = [("a", page_bytes), ("ab ", page_bytes / 3), ("word\n", page_bytes / 5), ("é", page_bytes / 2)];
    for pattern in chosen {
        let mut builder = RegexMatcherBuilder::new();
        builder
            .case_insensitive(true)
            .multi_line(true)
            .size_limit(size_limit)
            .dfa_size_limit(2 * 1024 * 1024)
            .nest_limit(64);
        let started = Instant::now();
        let matcher = match builder.build(pattern) {
            Ok(matcher) => matcher,
            Err(error) => {
                println!("{pattern}: refused: {}", error.to_string().chars().take(70).collect::<String>());
                continue;
            }
        };
        println!("{pattern}: compiled in {:?}", started.elapsed());
        for (piece, times) in pages {
            let page = piece.repeat(times);
            let started = Instant::now();
            let deadline = started + Duration::from_millis(deadline_ms);
            let (mut hits, mut cut) = (0u64, false);
            let _ = matcher.find_iter(page.as_bytes(), |found| {
                hits += u64::from(!found.is_empty());
                if Instant::now() >= deadline {
                    cut = true;
                    return false;
                }
                true
            });
            println!(
                "    {piece:?} x {times}: {:?}, {hits} hits{}",
                started.elapsed(),
                if cut { ", stopped at the deadline" } else { "" }
            );
        }
    }
}
