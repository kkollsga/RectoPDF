use super::super::tests::{alive, folder, never, stand_in};
use super::super::{Ended, MAX_LINE_BYTES as HEADINGS_LINE_BYTES, READER_STACK, next_line, read_bytes, talk};
use super::*;
use recto_platform::helper::{self, Priority};
use recto_testpdf::blocks::BLOCKS_TEXT;
use std::sync::{Arc, Mutex};

const HELLO: &str = r#"{"state":"hello","walled":true}"#;

/// A helper's whole answer: its hello and these lines.
fn said(lines: &[String]) -> Vec<u8> {
    let mut out = HELLO.to_string();
    for line in lines {
        out.push('\n');
        out.push_str(line);
    }
    out.push('\n');
    out.into_bytes()
}

fn block(kind: &str, text: &str) -> String {
    format!(
        r#"{{"kind":"{kind}","level":0,"box":[10,20,300,60],"text":{},"continues":false}}"#,
        serde_json::to_string(text).unwrap()
    )
}

fn page(page: u32, blocks: &[String]) -> String {
    format!(r#"{{"page":{page},"blocks":[{}]}}"#, blocks.join(","))
}

fn end(pages: u32, blocks: usize) -> String {
    format!(r#"{{"state":"ready","pages":{pages},"pages_failed":0,"blocks":{blocks},"cut":0,"distilled_ms":120,"helper_peak_bytes":5000000}}"#)
}

/// Pages' texts, and what was put, kept where a test can read them after the job's thread has them.
#[derive(Clone, Default)]
struct Store {
    texts: Vec<Option<String>>,
    put: Arc<Mutex<Vec<PageBlocks>>>,
    /// Page and code: the store says stop at that page.
    stop_at: Option<(u32, &'static str)>,
}

impl PageStore for Store {
    fn text(&mut self, page: u32, chars: &mut Vec<char>) -> bool {
        match self.texts.get(page as usize).and_then(Option::as_ref) {
            Some(text) => {
                chars.extend(text.chars());
                true
            }
            None => false,
        }
    }

    fn put(&mut self, page: &PageBlocks) -> Result<(), &'static str> {
        if self.stop_at.is_some_and(|(at, _)| at == page.page) {
            return Err(self.stop_at.map_or("", |(_, code)| code));
        }
        self.put.lock().unwrap().push(page.clone());
        Ok(())
    }
}

/// The blocks document's pages as text, as the index would keep them, and the lines a helper would say for them.
fn blocks_document() -> (Store, Vec<String>) {
    let words = |page: usize, from: usize, to: usize| BLOCKS_TEXT[page][from..to].join(" ");
    let texts: Vec<Option<String>> = BLOCKS_TEXT.iter().map(|lines| Some(lines.join("\n"))).collect();
    let lines = vec![
        page(
            0,
            &[
                block("heading", BLOCKS_TEXT[0][0]),
                block("para", &words(0, 1, 4)),
                block("para", &words(0, 5, 8)),
            ],
        ),
        page(
            1,
            &[block("heading", BLOCKS_TEXT[1][0]), block("list_item", BLOCKS_TEXT[1][1]), block("table", "")],
        ),
        page(2, &[block("para", &words(2, 0, 3))]),
    ];
    (Store { texts, ..Store::default() }, lines)
}

fn all(lines: &[String]) -> Vec<String> {
    let mut all = lines.to_vec();
    all.push(end(3, 7));
    all
}

/// Failure: a page's blocks are put out of page order or twice, are mapped against another page's text, or are counted wrong: a table counted as stored, or not counted as the kind that isn't. On the blocks document: three pages are put, in order, each once, with the stored counts as the text allows and the table counted as `not_stored_kind`.
#[test]
fn an_answer_maps_each_page_in_order_and_puts_it_once() {
    let (store, lines) = blocks_document();
    let put = store.put.clone();
    let (_, tally, numbers) = read_all(&said(&all(&lines)), 3, store).expect("believed");
    let put = put.lock().unwrap();
    assert_eq!(put.iter().map(|page| page.page).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(put.iter().map(|page| page.blocks.len()).collect::<Vec<_>>(), [3, 2, 1]);
    assert_eq!(tally.pages, 3);
    assert_eq!(
        (
            tally.stored[Shape::Paragraph.index()],
            tally.stored[Shape::Heading.index()],
            tally.stored[Shape::ListItem.index()]
        ),
        (3, 2, 1)
    );
    assert_eq!(
        (tally.dropped[Shape::Table.index()], tally.why.not_stored_kind),
        (1, 1),
        "a table is counted as the kind that isn't stored"
    );
    assert_eq!(
        (numbers["candidates"].as_u64(), numbers["stored"].as_u64(), numbers["dropped"].as_u64()),
        (Some(7), Some(6), Some(1))
    );
    assert_eq!(numbers["stored_heading"], 2);
    assert_eq!(numbers["dropped_table"], 1);
    assert_eq!(numbers["pages_without_text"], 0);
    assert_eq!(numbers["helper_peak_bytes"], 5_000_000, "the helper's numbers go on for measuring");
    assert!(numbers["answer_bytes"].as_u64().unwrap() > 100 && numbers.get("state").is_none(), "{numbers}");
}

/// Failure: a gap, a repeat or a page out of order is believed, so a page's blocks are stored under another page, or a page is skipped and the end still adds up. Each ends as `bad_answer`, and nothing past the bad line is put.
#[test]
fn a_gap_a_repeat_or_a_page_out_of_order_is_refused() {
    let (store, lines) = blocks_document();
    for (name, order, put_before) in [
        ("a gap", [0usize, 2, 1], 1),
        ("a repeat", [0, 0, 1], 1),
        ("out of order", [1, 0, 2], 0),
        ("a first page that isn't the first", [2, 1, 0], 0),
    ] {
        let said = said(&order.iter().map(|at| lines[*at].clone()).chain([end(3, 7)]).collect::<Vec<_>>());
        let answer = BlocksAnswer::new(store.clone(), 3);
        let put = answer.store.put.clone();
        put.lock().unwrap().clear();
        let (_, terminal, bad) = read_bytes(&said, answer);
        assert!(bad && terminal.is_none(), "{name}: {terminal:?} {bad}");
        assert_eq!(put.lock().unwrap().len(), put_before, "{name}");
    }
}

/// Failure: a helper that skips its last page (two lines, then an end that says the index's own three) chooses the code: the end's page count is checked against the lines first, so it reads as `pages_differ`, which a hostile helper could use to look like a document that changed. The index's count is checked first, and a helper that agrees with it but sent fewer lines is a bad answer.
#[test]
fn a_helper_that_skips_its_last_page_is_a_bad_answer_not_pages_differ() {
    let (store, lines) = blocks_document();
    let skipped = said(&[lines[0].clone(), lines[1].clone(), end(3, 5)]);
    assert_eq!(read_all(&skipped, 3, store).map(|_| ()), Err("bad_answer"));
}

/// Failure: a helper that read more pages than the index has is believed, so blocks are stored for pages the document doesn't have, or the end's count is taken for the document's. Both the line form (a page past the count) and the end form (an end that claims another number) end as `pages_differ`.
#[test]
fn more_pages_than_the_index_has_is_pages_differ() {
    let (store, lines) = blocks_document();
    let past = said(&[lines[0].clone(), page(3, &[]), end(4, 3)]);
    assert_eq!(read_all(&past, 3, store.clone()).map(|_| ()), Err("pages_differ"), "a line past the count");
    let claims_more = said(&all(&lines).iter().map(|line| line.replace(r#""pages":3"#, r#""pages":4"#)).collect::<Vec<_>>());
    assert_eq!(
        read_all(&claims_more, 3, store.clone()).map(|_| ()),
        Err("pages_differ"),
        "an end that says more"
    );
    let claims_fewer = said(&[lines[0].clone(), lines[1].clone(), end(2, 5)]);
    assert_eq!(
        read_all(&claims_fewer, 3, store).map(|_| ()),
        Err("pages_differ"),
        "an end that stops short of the index's count"
    );
}

/// Failure: an end line that doesn't add up is believed, so a document's blocks are reported complete when the helper cut or lost some, or a helper puts its own words among the numbers or says one the session measures. Each is refused as `bad_answer`.
#[test]
fn the_end_must_add_up() {
    let (store, lines) = blocks_document();
    let good = end(3, 7);
    let thirty_three: String = (0..30).map(|n| format!(r#","n{n}":{n}"#)).collect();
    for (name, bad_end) in [
        ("blocks off by one", good.replace(r#""blocks":7"#, r#""blocks":8"#)),
        ("blocks one short", good.replace(r#""blocks":7"#, r#""blocks":6"#)),
        ("a session number", good.replace(r#""cut":0"#, r#""cut":0,"walled":true"#)),
        ("elapsed_ms", good.replace(r#""cut":0"#, r#""cut":0,"elapsed_ms":1"#)),
        ("33 entries", good.replace(r#""cut":0"#, &format!(r#""cut":0{thirty_three}"#))),
        ("a word value", good.replace(r#""cut":0"#, r#""cut":"none""#)),
        ("no cut", good.replace(r#""cut":0,"#, "")),
        ("more failed pages than pages", good.replace(r#""pages_failed":0"#, r#""pages_failed":4"#)),
        ("pages that isn't a number", good.replace(r#""pages":3"#, r#""pages":"3""#)),
    ] {
        let mut all = lines.clone();
        all.push(bad_end);
        assert_eq!(read_all(&said(&all), 3, store.clone()).map(|_| ()), Err("bad_answer"), "{name}");
    }
    // 32 entries is fine: state and 31 numbers.
    let thirty_one: String = (0..24).map(|n| format!(r#","n{n}":{n}"#)).collect();
    let mut all = lines;
    all.push(good.replace(r#""cut":0"#, &format!(r#""cut":0{thirty_one}"#)));
    assert!(read_all(&said(&all), 3, store).is_ok());
}

/// Failure: a refusal, a change or a failure said after a page line is believed (so a helper that has said some pages can take the answer back), or the one the headings job reads differs from this one. Only the first line may say one, for blocks and for headings alike, and a refusal's code is one of the known ones.
#[test]
fn only_the_first_line_may_refuse() {
    let (store, lines) = blocks_document();
    for (instead, want) in [
        (r#"{"state":"refused","code":"needs_password"}"#, "refused"),
        (r#"{"state":"refused","code":"anything I like"}"#, "refused"),
        (r#"{"state":"changed"}"#, "changed"),
        (r#"{"state":"failed","code":"oops"}"#, "helper_stopped"),
    ] {
        assert_eq!(read_all(&said(&[instead.to_string()]), 3, store.clone()).map(|_| ()), Err(want), "{instead}");
        let after_a_page = said(&[lines[0].clone(), instead.to_string()]);
        assert_eq!(
            read_all(&after_a_page, 3, store.clone()).map(|_| ()),
            Err("bad_answer"),
            "after a page: {instead}"
        );
        let headings_after_a_page = said(&[r#"{"page":0,"found":[{"level":1,"bbox":null,"title":"A"}]}"#.to_string(), instead.to_string()]);
        assert_eq!(
            super::super::headings::read_all(&headings_after_a_page, 10, 5).map(|_| ()),
            Err("bad_answer"),
            "headings, after a page: {instead}"
        );
        assert_eq!(
            super::super::headings::read_all(&said(&[instead.to_string()]), 10, 5).map(|_| ()),
            Err(want),
            "headings, first: {instead}"
        );
    }
    let (_, terminal, _) = read_bytes(
        &said(&[r#"{"state":"refused","code":"anything I like"}"#.to_string()]),
        BlocksAnswer::new(store, 3),
    );
    assert_eq!(terminal, Some(Said::Refused("couldnt_read".into())), "its own words are never repeated");
}

/// Failure: a page whose text can't be had is mapped against another page's characters or stops the job, or isn't said: it maps nothing, its blocks are counted as dropped and the page is counted, and the pages after it are read.
#[test]
fn a_page_without_text_maps_nothing_and_says_so() {
    let (mut store, lines) = blocks_document();
    store.texts[1] = None;
    let put = store.put.clone();
    let (_, tally, numbers) = read_all(&said(&all(&lines)), 3, store).expect("believed");
    assert_eq!(tally.pages_without_text, 1);
    assert_eq!(numbers["pages_without_text"], 1);
    let put = put.lock().unwrap();
    assert_eq!(
        (put.len(), put[1].page, put[1].blocks.len(), put[1].chars),
        (3, 1, 0, 0),
        "the page is put, empty"
    );
    assert_eq!(put[2].blocks.len(), 1, "the page after it is read");
    assert_eq!(tally.stored[Shape::Heading.index()], 1, "page 1's heading and list item stored nothing");
}

/// Failure: a store that says stop isn't obeyed or isn't heard: the job goes on past its page, the helper is left running, or the reason reaches the caller as `bad_answer` (what the answer's `Err` looks like to the runner) instead of the store's own code. A stand-in helper says three pages and would then sleep ten minutes; the store stops at page 1 with `too_large`: the job ends as that, the helper is gone within 5 s, and only page 0 was put.
#[test]
fn a_store_that_says_stop_ends_the_job_with_its_code() {
    let folder = folder("store-stop");
    let pid_file = folder.join("pid");
    let exe = stand_in(
        &folder,
        &format!(
            "echo $$ > '{}'\necho '{{\"state\":\"hello\",\"walled\":true}}'\nread job\necho '{}'\necho '{}'\necho '{}'\nexec /bin/sleep 600",
            pid_file.display(),
            page(0, &[]),
            page(1, &[]),
            page(2, &[])
        ),
    );
    let store = Store {
        stop_at: Some((1, "too_large")),
        ..Store::default()
    };
    let put = store.put.clone();
    let spawned = helper::spawn(&exe, &[], Priority::Inherited).unwrap();
    let (deadline, stop) = never();
    let started = Instant::now();
    let (ended, _) = talk(spawned, &job(), BlocksAnswer::new(store, 3), deadline, &stop, |_, _| Ok(()));
    assert!(matches!(&ended, Ended::Failed(code) if code == "too_large"), "{ended:?}");
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert_eq!(put.lock().unwrap().iter().map(|page| page.page).collect::<Vec<_>>(), [0]);
    let pid: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    assert!(!alive(pid), "the helper is gone");
}

/// A line of exactly `len` bytes for page `page`: one paragraph whose text is the filler.
fn page_of_len(page: u32, len: usize) -> String {
    let empty = self::page(page, &[block("para", "")]);
    let line = self::page(page, &[block("para", &"x".repeat(len - empty.len()))]);
    assert_eq!(line.len(), len);
    line
}

/// Failure: the 64 KiB cap that headings lines have is applied to blocks lines too, so a busy page's 300 KiB line ends the job as `bad_answer`; or the blocks cap isn't held and a line a byte over it is read; or the answer's total ceiling isn't the one for the document's pages (4 MiB and 64 KiB a page), so a ten-page document may say more than that, or a huge one is held to 4 MiB. At the cap a line is read and one byte more is refused; the total is held at its limit and one byte more.
#[test]
fn each_answer_has_its_own_caps() {
    let (mut buf, mut budget) = (Vec::new(), u64::MAX);
    let answer = BlocksAnswer::new(Store::default(), 3);
    assert_eq!((answer.max_line_bytes(), HEADINGS_LINE_BYTES), (512 * 1024, 64 * 1024));
    let at_the_cap = [vec![b'a'; answer.max_line_bytes()], vec![b'\n']].concat();
    assert_eq!(next_line(&mut &at_the_cap[..], &mut buf, &mut budget, answer.max_line_bytes()), Ok(true));
    let over = [vec![b'a'; answer.max_line_bytes() + 1], vec![b'\n']].concat();
    assert_eq!(next_line(&mut &over[..], &mut buf, &mut budget, answer.max_line_bytes()), Err("bad_answer"));
    // A 300 KiB page line is taken by the live reader, whose cap is the answer's.
    let (store, _) = blocks_document();
    let long = page_of_len(0, 300 * 1024);
    let (_, terminal, bad) = read_bytes(&said(&[long, end(1, 1)]), BlocksAnswer::new(store, 1));
    assert!(!bad, "a 300 KiB blocks line is within the cap");
    assert_eq!(terminal, Some(Said::End));
    // The total: 4 MiB and 64 KiB a page, up to 2 GiB.
    assert_eq!(BlocksAnswer::new(Store::default(), 0).max_answer_bytes(), 4 << 20);
    assert_eq!(BlocksAnswer::new(Store::default(), 10).max_answer_bytes(), (4 << 20) + 10 * (64 << 10));
    assert_eq!(BlocksAnswer::new(Store::default(), u32::MAX).max_answer_bytes(), 2 << 30);
    let (hello, pages) = (HELLO.len() as u64 + 1, 10);
    let limit = BlocksAnswer::new(Store::default(), pages).max_answer_bytes();
    let big = 512 * 1024;
    let first: Vec<String> = (0..9).map(|at| page_of_len(at, big)).collect();
    let used = hello + first.iter().map(|line| line.len() as u64 + 1).sum::<u64>();
    let last = (limit - used - 1) as usize;
    let at_the_limit: Vec<String> = first.iter().cloned().chain([page_of_len(9, last)]).collect();
    let over_the_limit: Vec<String> = first.iter().cloned().chain([page_of_len(9, last + 1)]).collect();
    let (_, _, bad) = read_bytes(&said(&at_the_limit), BlocksAnswer::new(Store::default(), pages));
    assert!(!bad, "the whole answer, at its limit, is read");
    let (_, _, bad) = read_bytes(&said(&over_the_limit), BlocksAnswer::new(Store::default(), pages));
    assert!(bad, "one byte over is refused");
}

/// Failure: a 512 KiB line of `[` overflows the reader thread's stack (serde_json recurses, and the blocks cap is eight times the headings one) and takes the whole app with it, instead of being refused as a bad line.
#[test]
fn a_512_kib_line_of_open_brackets_is_refused_on_the_readers_stack() {
    let read = std::thread::Builder::new()
        .stack_size(READER_STACK)
        .spawn(|| {
            let line = vec![b'['; MAX_LINE_BYTES];
            let mut answer = BlocksAnswer::new(Store::default(), 3);
            Answer::take(&mut answer, &line)
        })
        .unwrap()
        .join();
    assert_eq!(read.expect("no overflow"), Err("bad_answer"));
}
