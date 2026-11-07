//! Ranking pages by word counts: BM25, in pure functions, with no state and no I/O, so they can be fuzzed directly. It needs only each query word's count per page, each page's length, and how many pages have each word, all of which come from one pass over the text, so it needs no index and nothing more on disk.
//!
//! **Words,** as this cuts them up:
//! - A run of letters and digits, lower-cased by Unicode's rules, is a word. So `data-driven` is two, `don't` is two, and `1.5` is two.
//! - Each character of Han, Hiragana and Katakana is a word by itself, because those aren't written with spaces. So a search for `数学` counts `数` and `学`, wherever they are, and not the two side by side. It finds the right pages more often than not, and it's weak: nothing here knows a word of Chinese or Japanese.
//!
//! **What's weak about that,** plainly: no stemming, so `liability` doesn't count `liabilities`. No folding beyond case, so `cafe` misses `café`, and a decomposed `é` breaks its word in two, since it's a combining mark. Thai and other languages written with no spaces come out as one long word per run, and so find little. Hangul goes by its spaces, as Korean is written.
//!
//! **It isn't a judgement.** A page that repeats a word scores well for it, and a hostile document can stuff keywords to rank first, which no such count can stop. The score says how the words are distributed, and nothing about whether a page is any use or true.

/// The usual settings: how quickly more of a word stops counting, and how much a long page is marked down.
const K1: f64 = 1.2;
const B: f64 = 0.75;

/// Most words of a query. Each costs a comparison per word of every page.
pub const MAX_TERMS: usize = 16;

/// A character in the one lower case every search compares by, a page and a query alike. Beyond Unicode's rule for one character: final `ς` is `σ`, or `ΟΔΟΣ` wouldn't find `οδος`, and `İ` is `i`, or `istanbul` wouldn't find `İstanbul`.
pub fn lower(c: char) -> impl Iterator<Item = char> {
    let special = match c {
        'ς' | 'Σ' => Some('σ'),
        'İ' => Some('i'),
        _ => None,
    };
    special.into_iter().chain(c.to_lowercase().filter(move |_| special.is_none()))
}

/// Whether a character is written with no spaces between words, so it's a word by itself.
fn stands_alone(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30FF}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}' | '\u{20000}'..='\u{2FFFF}' | '\u{30000}'..='\u{3134F}')
}

/// Calls `found` with where each word of `text` is, in bytes. All-ASCII text, most of it, takes a shorter way.
pub fn each_word(text: &str, mut found: impl FnMut(usize, usize)) {
    if text.is_ascii() {
        let bytes = text.as_bytes();
        let mut start = None;
        for (at, byte) in bytes.iter().enumerate() {
            match (byte.is_ascii_alphanumeric(), start) {
                (true, None) => start = Some(at),
                (false, Some(from)) => {
                    found(from, at);
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(from) = start {
            found(from, bytes.len());
        }
        return;
    }
    let mut start = None;
    for (at, c) in text.char_indices() {
        if stands_alone(c) {
            if let Some(from) = start.take() {
                found(from, at);
            }
            found(at, at + c.len_utf8());
        } else if c.is_alphanumeric() {
            start.get_or_insert(at);
        } else if let Some(from) = start.take() {
            found(from, at);
        }
    }
    if let Some(from) = start {
        found(from, text.len());
    }
}

/// What a query's words are, lower-cased, each once, in the order given. At most [`MAX_TERMS`], and `dropped` says how many weren't kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Terms {
    pub terms: Vec<String>,
    pub dropped: u32,
}

pub fn terms_of(query: &str) -> Terms {
    let mut out = Terms::default();
    each_word(query, |start, end| {
        let word: String = query[start..end].chars().flat_map(lower).collect();
        if out.terms.contains(&word) {
            return;
        }
        if out.terms.len() >= MAX_TERMS {
            out.dropped += 1;
        } else {
            out.terms.push(word);
        }
    });
    out
}

/// Whether a word of a page is this lower-cased term, with nothing allocated: this runs for every word of every page.
fn same_word(word: &str, term: &str) -> bool {
    if word.is_ascii() && term.is_ascii() {
        return word.eq_ignore_ascii_case(term);
    }
    word.chars().flat_map(lower).eq(term.chars())
}

/// Which term this word is, if it's one of them.
fn which_term(word: &str, terms: &[String]) -> Option<usize> {
    terms.iter().position(|term| same_word(word, term))
}

/// How many words a page has, and adds how many of each term to `counts`, which has one place per term. Returns the page's length in words.
pub fn count_terms(text: &str, terms: &[String], counts: &mut [u32]) -> u32 {
    let mut length = 0u32;
    each_word(text, |start, end| {
        length = length.saturating_add(1);
        if let Some(term) = which_term(&text[start..end], terms) {
            counts[term] = counts[term].saturating_add(1);
        }
    });
    length
}

/// The first of these words on a page, in bytes, for a snippet.
pub fn first_term(text: &str, terms: &[String]) -> Option<std::ops::Range<usize>> {
    let mut first = None;
    each_word(text, |start, end| {
        if first.is_none() && which_term(&text[start..end], terms).is_some() {
            first = Some(start..end);
        }
    });
    first
}

/// What's known about the pages as a whole, for scoring any one of them.
#[derive(Clone, Copy, Debug)]
pub struct Collection {
    /// Pages with any text.
    pub pages: u64,
    /// Words on all of those.
    pub words: u64,
}

/// One page's score. `counts` and `pages_with` have one place per term: its count here, and how many pages have it. BM25, with its logarithm kept above zero so a word on most pages still counts for a little, as in Lucene.
pub fn score(collection: Collection, length: u32, counts: &[u32], pages_with: &[u32]) -> f64 {
    let average = if collection.pages == 0 { 1.0 } else { collection.words as f64 / collection.pages as f64 }.max(1.0);
    counts
        .iter()
        .zip(pages_with)
        .map(|(count, with)| {
            let (count, with) = (f64::from(*count), f64::from(*with));
            if count == 0.0 {
                return 0.0;
            }
            let rarity = (1.0 + (collection.pages as f64 - with + 0.5) / (with + 0.5)).ln();
            rarity * count * (K1 + 1.0) / (count + K1 * (1.0 - B + B * f64::from(length) / average))
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<&str> {
        let mut out = Vec::new();
        each_word(text, |start, end| out.push(&text[start..end]));
        out
    }

    /// Failure: words are cut differently for ASCII and everything else, or at their edges, so a word at a page's end or next to punctuation isn't counted, and a page ranks low for words it has.
    #[test]
    fn words_are_runs_of_letters_and_digits() {
        assert_eq!(words("The quick-brown fox, don't 1.5\nend"), ["The", "quick", "brown", "fox", "don", "t", "1", "5", "end"]);
        assert_eq!(words("Ærlig caffè ÉCOLE. naïve-word"), ["Ærlig", "caffè", "ÉCOLE", "naïve", "word"]);
        assert_eq!(words(""), Vec::<&str>::new());
        assert_eq!(words("   ,,, "), Vec::<&str>::new());
        assert_eq!(words("x"), ["x"]);
        assert_eq!(words("ascii é"), ["ascii", "é"], "the same with one non-ASCII letter in the text");
    }

    /// Failure: Chinese or Japanese, with no spaces to go by, comes out as one word a sentence long, so no query ever matches it. Each character stands alone, among other scripts as well.
    #[test]
    fn writing_with_no_spaces_comes_apart_by_character() {
        assert_eq!(words("数学の基礎"), ["数", "学", "の", "基", "礎"]);
        assert_eq!(words("カタカナ"), ["カ", "タ", "カ", "ナ"]);
        assert_eq!(words("abc数def 한국어 단어"), ["abc", "数", "def", "한국어", "단어"], "Hangul goes by its spaces");
        assert_eq!(terms_of("数学").terms, ["数", "学"]);
    }

    /// Failure: the same word in two cases counts as two terms, which counts it double, or a query of a thousand words costs a thousand comparisons a word on a hostile request. Each is kept once, up to the cap, and what's dropped is said.
    #[test]
    fn a_querys_terms_are_each_kept_once_and_capped() {
        assert_eq!(terms_of("Force force FORCE majeure").terms, ["force", "majeure"]);
        assert!(terms_of("!!! ,,,").terms.is_empty());
        let many = (0..40).map(|at| format!("w{at}")).collect::<Vec<_>>().join(" ");
        let terms = terms_of(&many);
        assert_eq!((terms.terms.len(), terms.dropped), (MAX_TERMS, 24));
        assert_eq!(terms_of("ÆRLIG").terms, ["ærlig"]);
        assert_eq!(terms_of("İ").terms, ["i"]);
    }

    /// Failure: counts miss a word in another case, or a letter whose lower case differs in length, so a query in capitals finds nothing, or counts a word inside a longer one.
    #[test]
    fn counts_go_by_whole_words_in_any_case() {
        let terms = vec!["force".to_string(), "ærlig".to_string(), "i".to_string()];
        let mut counts = vec![0; 3];
        let length = count_terms("FORCE majeure, force; enforce forced ÆRLIG İ", &terms, &mut counts);
        assert_eq!((length, counts.as_slice()), (7, [2, 1, 1].as_slice()));
        assert_eq!(first_term("no such thing here FORCE again", &terms), Some(19..24));
        assert_eq!(first_term("nothing", &terms), None);
    }

    /// Failure: a common word outscores a rare one, a long page wins for being long, or more of a word keeps counting without limit, so stuffing beats everything by far. These are what makes it BM25, on orders that hold whatever the settings.
    #[test]
    fn a_score_prefers_rare_words_short_pages_and_flattens_out() {
        let collection = Collection { pages: 100, words: 100_000 };
        let rare = score(collection, 1000, &[3], &[2]);
        let common = score(collection, 1000, &[3], &[90]);
        assert!(rare > common, "{rare} {common}");
        assert!(common > 0.0, "a word on most pages still counts for a little");
        assert!(score(collection, 200, &[3], &[10]) > score(collection, 5000, &[3], &[10]), "a short page is marked up");
        let (once, twice, a_thousand) = (score(collection, 1000, &[1], &[10]), score(collection, 1000, &[2], &[10]), score(collection, 1000, &[1000], &[10]));
        assert!(once < twice && twice < a_thousand);
        assert!(a_thousand < once * 4.0, "{once} {a_thousand}: stuffing is bounded, not free");
        assert_eq!(score(collection, 1000, &[0, 0], &[1, 1]), 0.0);
        assert!(score(collection, 1000, &[1, 1], &[10, 10]) > once, "both words beat one");
    }

    /// Failure: a page with no words, a collection with none, or a count at its largest makes a score NaN or infinite, which sorts anywhere, or panics on a hostile page.
    #[test]
    fn a_score_is_always_a_number() {
        for (pages, words, length, count, with) in [(0, 0, 0, 0, 0), (1, 0, 0, 1, 1), (u64::MAX, u64::MAX, u32::MAX, u32::MAX, u32::MAX), (1, 1, 0, u32::MAX, 1)] {
            assert!(score(Collection { pages, words }, length, &[count], &[with]).is_finite(), "{pages} {words} {length} {count} {with}");
        }
    }

    /// Failure: Greek: a query in capitals lower-cases its last `Σ` to `ς` and a page's to `σ`, so `ΟΔΟΣ` scores nothing on a page that says `ΟΔΟΣ`. And Turkish: `İstanbul` isn't found by `istanbul`, or by itself.
    #[test]
    fn a_query_and_a_page_fold_the_same_way() {
        let found = |query: &str, page: &str| {
            let terms = terms_of(query).terms;
            let mut counts = vec![0; terms.len()];
            count_terms(page, &terms, &mut counts);
            counts[0]
        };
        for (query, page) in [("ΟΔΟΣ", "ΟΔΟΣ"), ("ΟΔΟΣ", "οδος"), ("οδος", "ΟΔΟΣ"), ("οδοσ", "Οδος"), ("İstanbul", "İSTANBUL"), ("istanbul", "İstanbul"), ("İSTANBUL", "istanbul")] {
            assert_eq!(found(query, page), 1, "{query} on {page}");
        }
        assert_eq!(lower('Σ').collect::<String>(), "σ");
        assert_eq!(lower('Æ').collect::<String>(), "æ");
        assert_eq!(lower('x').collect::<String>(), "x");
    }
}
