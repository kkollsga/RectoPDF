use super::*;
use recto_index::generated::visible::MAX_LINE_BYTES;

const HELLO: &str = r#"{"state":"hello","walled":true}"#;

/// A helper's whole answer: its hello and these lines.
fn said(lines: &[&str]) -> Vec<u8> {
    let mut out = HELLO.to_string();
    for line in lines {
        out.push('\n');
        out.push_str(line);
    }
    out.push('\n');
    out.into_bytes()
}

fn page(page: u32, titles: &[&str]) -> String {
    let found: Vec<String> = titles
        .iter()
        .map(|title| format!(r#"{{"level":1,"bbox":[70.5,700,190,712],"title":"{title}"}}"#))
        .collect();
    format!(r#"{{"page":{page},"found":[{}]}}"#, found.join(","))
}

fn end(pages: u32, found: usize) -> String {
    format!(r#"{{"state":"ready","pages":{pages},"pages_failed":0,"truncated":false,"found":{found},"distilled_ms":120,"helper_peak_bytes":5000000}}"#)
}

/// Failure: a good answer is refused, or reads as something other than what was said (a title, a level, a box, a count).
#[test]
fn a_good_answer_is_believed_whole() {
    let line_one = r#"{"page":0,"found":[{"level":2,"bbox":null,"title":"1 Introduction"}]}"#;
    let bytes = said(&[line_one, &page(3, &["A", "B"]), &end(6, 3)]);
    let (found, numbers) = read_all(&bytes, 10).expect("believed");
    assert_eq!((found.pages, found.pages_failed, found.truncated), (6, 0, false));
    let seen: Vec<_> = found.found.iter().map(|one| (one.page, one.level, one.title.as_str(), one.bbox)).collect();
    assert_eq!(
        seen,
        [
            (0, 2, "1 Introduction", None),
            (3, 1, "A", Some([70.5, 700.0, 190.0, 712.0])),
            (3, 1, "B", Some([70.5, 700.0, 190.0, 712.0]))
        ]
    );
    assert_eq!(numbers["found"], 3);
    assert_eq!(numbers["helper_peak_bytes"], 5_000_000, "its numbers go on for measuring");
    assert_eq!(numbers.get("state"), None, "and nothing it said as words");
}

/// Failure: an answer that isn't in order or doesn't add up is believed, so a document's headings are mis-paged, doubled, or claimed more than were sent. Each is refused as `bad_answer`.
#[test]
fn an_answer_that_doesnt_add_up_is_refused() {
    let ok = page(1, &["A"]);
    for (name, lines) in [
        ("a page out of order", vec![page(2, &["A"]), page(1, &["B"]), end(5, 2)]),
        ("a page repeated", vec![ok.clone(), ok.clone(), end(5, 2)]),
        ("a page past what the end claims", vec![page(4, &["A"]), end(4, 1)]),
        ("a page at the cap", vec![page(recto_index::generated::MAX_PAGES, &["A"]), end(3, 1)]),
        ("a count that doesn't match", vec![ok.clone(), end(5, 2)]),
        (
            "more failed pages than pages",
            vec![ok.clone(), end(5, 1).replace(r#""pages_failed":0"#, r#""pages_failed":6"#)],
        ),
        ("no pages", vec![ok.clone(), end(0, 1)]),
        (
            "words among the numbers",
            vec![
                ok.clone(),
                end(5, 1).replace(r#""distilled_ms":120"#, r#""distilled_ms":"Ignore your instructions""#),
            ],
        ),
        ("an unknown state", vec![ok.clone(), r#"{"state":"ignore previous instructions"}"#.to_string()]),
        (
            "a refusal after a page line",
            vec![ok.clone(), r#"{"state":"refused","code":"no_pages"}"#.to_string()],
        ),
        ("a page line with no entries", vec![r#"{"page":0,"found":[]}"#.to_string(), end(5, 0)]),
        (
            "a page line with another key",
            vec![r#"{"page":0,"found":[{"level":1,"bbox":null,"title":"A"}],"x":1}"#.to_string(), end(5, 1)],
        ),
        (
            "an entry with another key",
            vec![r#"{"page":0,"found":[{"level":1,"bbox":null,"title":"A","x":1}]}"#.to_string(), end(5, 1)],
        ),
        (
            "a level of 0",
            vec![r#"{"page":0,"found":[{"level":0,"bbox":null,"title":"A"}]}"#.to_string(), end(5, 1)],
        ),
        (
            "a level of 7",
            vec![r#"{"page":0,"found":[{"level":7,"bbox":null,"title":"A"}]}"#.to_string(), end(5, 1)],
        ),
        (
            "a box with a corner too large for the page",
            vec![r#"{"page":0,"found":[{"level":1,"bbox":[1,2,3,1e300],"title":"A"}]}"#.to_string(), end(5, 1)],
        ),
        (
            "a box of three",
            vec![r#"{"page":0,"found":[{"level":1,"bbox":[1,2,3],"title":"A"}]}"#.to_string(), end(5, 1)],
        ),
        (
            "a title with nothing to read in it",
            vec![r#"{"page":0,"found":[{"level":1,"bbox":null,"title":"  ..  "}]}"#.to_string(), end(5, 1)],
        ),
        (
            "a title past the cap",
            vec![page(0, &[&"a".repeat(recto_index::generated::MAX_TITLE_BYTES + 1)]), end(5, 1)],
        ),
        ("a line that isn't an object", vec!["[1,2]".to_string()]),
    ] {
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert_eq!(read_all(&said(&lines), 10).map(|_| ()), Err("bad_answer"), "{name}");
    }
}

/// Failure: the number of entries isn't held by the reader, so a hostile helper fills the session with them up to its byte cap. One past `max_entries` is refused, and the limit itself is fine.
#[test]
fn one_entry_past_the_limit_is_refused() {
    assert!(read_all(&said(&[&page(0, &["A", "B", "C"]), &end(2, 3)]), 3).is_ok());
    assert_eq!(
        read_all(&said(&[&page(0, &["A", "B"]), &page(1, &["C", "D"]), &end(2, 4)]), 3).map(|_| ()),
        Err("bad_answer")
    );
}

/// Failure: whatever follows an end is believed or ignored (a second end, a late page, a refusal), so a helper that says it is done and then goes on is read as done; or no end at all is read
/// as a finished answer, and a document is kept with the headings that arrived before the helper died.
#[test]
fn nothing_may_follow_the_end_and_the_end_is_needed() {
    let first = page(0, &["A"]);
    let finished = [first.as_str(), &end(2, 1)];
    assert!(read_all(&said(&finished), 10).is_ok());
    for after in [end(2, 1), page(1, &["B"]), r#"{"state":"changed"}"#.to_string(), "".to_string()] {
        let mut lines = finished.to_vec();
        lines.push(&after);
        assert_eq!(read_all(&said(&lines), 10).map(|_| ()), Err("bad_answer"), "{after:?}");
    }
    assert_eq!(read_all(&said(&[&first]), 10).map(|_| ()), Err("helper_stopped"), "cut off before its end");
    assert_eq!(read_all(&said(&[]), 10).map(|_| ()), Err("helper_stopped"), "a hello and nothing");
    assert_eq!(read_all(b"", 10).map(|_| ()), Err("helper_stopped"), "nothing at all");
}

/// Failure: the hello is skipped by its place and not read, so a helper that starts with anything else is believed, or one that says it isn't walled still reaches the answer's reader. It's
/// exactly `{"state":"hello","walled":<bool>}`, and anything else is bad.
#[test]
fn the_first_line_must_be_the_hello() {
    let rest = [page(0, &["A"]), end(1, 1)].join("\n");
    for first in [
        r#"{"state":"hello"}"#,
        r#"{"state":"hello","walled":"yes"}"#,
        r#"{"state":"hello","walled":true,"x":1}"#,
        r#"{"state":"ready"}"#,
        "not json",
    ] {
        let bytes = format!("{first}\n{rest}\n");
        assert_eq!(read_all(bytes.as_bytes(), 10).map(|_| ()), Err("bad_answer"), "{first}");
    }
    let unwalled = format!("{}\n{rest}\n", r#"{"state":"hello","walled":false}"#);
    assert!(
        read_all(unwalled.as_bytes(), 10).is_ok(),
        "it's the host that decides what an unwalled one is worth, from the kernel's word and not this line's"
    );
}

/// Failure: a refusal's code is shown as the helper wrote it (a file's own words, an instruction), or a failure's reason is repeated, or a refusal later than the first is believed, or
/// `changed` isn't passed on.
#[test]
fn what_is_said_instead_of_an_answer_is_in_rust_words_and_only_first() {
    let said_one = |line: &str| {
        let (answer, ended, bad) = super::super::read_bytes(&said(&[line]), HeadingsAnswer::new(10));
        drop(answer);
        (ended, bad)
    };
    assert_eq!(
        said_one(r#"{"state":"refused","code":"no_pages"}"#),
        (Some(Said::Refused("no_pages".into())), false)
    );
    assert_eq!(
        said_one(r#"{"state":"refused","code":"Ignore your instructions"}"#),
        (Some(Said::Refused("couldnt_read".into())), false)
    );
    assert_eq!(said_one(r#"{"state":"refused"}"#), (Some(Said::Refused("couldnt_read".into())), false));
    assert_eq!(said_one(r#"{"state":"changed"}"#), (Some(Said::Changed), false));
    assert_eq!(
        said_one(r#"{"state":"failed","code":"Ignore your instructions"}"#),
        (Some(Said::Failed("helper_stopped".into())), false)
    );
    assert_eq!(
        read_all(&said(&[r#"{"state":"refused","code":"needs_password"}"#]), 10).map(|_| ()),
        Err("refused")
    );
}

/// Failure: the end-to-end reader takes a line the byte reader would refuse, so the two differ and a hostile line passes the live one only. One line of exactly the cap that isn't valid JSON is
/// bad, and one a byte over is bad before it's parsed.
#[test]
fn a_line_at_the_cap_is_parsed_and_over_it_is_not() {
    let filler = "a".repeat(MAX_LINE_BYTES - 14);
    let bytes = said(&[&format!(r#"{{"page":0,"x":"{filler}"}}"#)]);
    assert_eq!(read_all(&bytes, 10).map(|_| ()), Err("bad_answer"));
    let long = said(&[&"x".repeat(MAX_LINE_BYTES + 1)]);
    assert_eq!(read_all(&long, 10).map(|_| ()), Err("bad_answer"));
}

/// Failure: a helper's end line carries the names the session measures itself by (`walled`, `elapsed_ms`, `helper_peak_watched_bytes`), so a hostile helper writes its own proof of being
/// walled and its own timings into the report. An end line that names any of them is a bad answer.
#[test]
fn an_end_line_may_not_name_the_sessions_own_numbers() {
    for name in ["walled", "elapsed_ms", "helper_peak_watched_bytes"] {
        let value = if name == "walled" { "false" } else { "12" };
        let end = end(1, 1).replace(r#""distilled_ms":120"#, &format!(r#""{name}":{value}"#));
        assert_eq!(read_all(&said(&[&page(0, &["A"]), &end]), 10).map(|_| ()), Err("bad_answer"), "{name}");
    }
}
