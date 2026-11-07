//! `recto select` and `recto copy`: what a click, a drag or a range of characters takes on a page, where it shows and what it copies. By the same methods as the app's page view, through `recto_index::select`, with a worker of its own for as long as the command takes.
//!
//! It holds one page's text at a time past the two pages a drag starts and ends on, and the next [`chrome::NEIGHBOURS`] read ahead, so a copy of a whole document costs what its few longest pages do, plus the copy. Running heads, footers and page numbers are left out of body text by comparing each page's edges with its neighbours' (see `recto_index::chrome`): a page past either end of the pick is read for that without the hidden-text check, as only its edge lines are kept.

use crate::render::open_for_text;
use crate::{AgentError, SessionAccess, exit, no_such_page, select as select_tab, source_changed};
use recto_index::chrome::{self, EdgeLine};
use recto_index::select::{CopiedText, MAX_COPY_BYTES, MAX_COPY_PAGES, MAX_COPY_TIME, PagePoint, PageText, Place, Range, Selection, Texts, Unit, chrome_for, neighbour_pages};
use recto_render_service::DocRenderer;
use recto_session::TabId;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Workers kept through one command. A tab of many inserted files goes from one to the next, and there's no reason to hold on to the others.
const MAX_WORKERS: usize = 2;
/// Pages one `select` gives rectangles for. It's for looking at a stretch, and `copy` is for taking a long one.
const MAX_SELECT_PAGES: u32 = 32;
/// Text one `select` gives, which goes the same way as its rectangles.
const MAX_SELECT_BYTES: usize = 4 * 1024 * 1024;

/// What's been asked for, on the command line or the wire.
pub(crate) struct Pick<'a> {
    pub tab: &'a str,
    pub page: Option<u32>,
    pub to_page: Option<u32>,
    pub chars: Option<&'a str>,
    pub at: Option<&'a str>,
    pub to: Option<&'a str>,
    pub unit: Option<&'a str>,
    pub all: bool,
    pub anyway: bool,
}

fn bad(words: impl Into<String>) -> AgentError {
    AgentError::new("usage", words.into(), exit::USAGE)
}

fn point(word: &str, page: u32) -> Result<PagePoint, AgentError> {
    let parsed = word.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))).filter(|(x, y)| x.is_finite() && y.is_finite());
    let (x, y) = parsed.ok_or_else(|| bad("A place on a page is `X,Y`, in points from its top left, as `select` gives its rectangles."))?;
    Ok(PagePoint { page, x, y })
}

fn char_range(word: &str) -> Result<(u32, u32), AgentError> {
    let parsed = word.split_once('-').and_then(|(from, to)| Some((from.parse::<u32>().ok()?, to.parse::<u32>().ok()?)));
    parsed.filter(|(from, to)| from <= to).ok_or_else(|| bad("`--chars` is `START-END`, counted in characters of `recto text`, with the end not included."))
}

/// Pages as they're read, one worker to a file.
struct Pages<'a, A: SessionAccess> {
    access: &'a A,
    id: TabId,
    /// At most [`MAX_WORKERS`], the most recently used last. Dropping one ends its worker.
    docs: Vec<(PathBuf, DocRenderer)>,
    /// What says where a drag's ends are. They're read once, and kept for as long as this command.
    ends: HashMap<u32, PageText>,
    /// Those of them that couldn't be read, which has nothing to do with having no text.
    failed_ends: HashSet<u32>,
}

impl<A: SessionAccess> Pages<'_, A> {
    /// A page's text, or `None` if it couldn't be read, which is a page failing and not the command.
    fn read(&mut self, position: u32) -> Result<Option<PageText>, AgentError> {
        self.read_checked(position, true)
    }

    fn read_checked(&mut self, position: u32, hidden_check: bool) -> Result<Option<PageText>, AgentError> {
        let id = self.id;
        let source = self.access.with(|s| s.page_source(id, position)).ok_or_else(no_such_page)?;
        if !source.is_current() {
            return Err(source_changed(&source.path));
        }
        match self.docs.iter().position(|(path, _)| *path == source.path) {
            Some(at) => {
                let found = self.docs.remove(at);
                self.docs.push(found);
            }
            None => {
                if self.docs.len() >= MAX_WORKERS {
                    self.docs.remove(0);
                }
                self.docs.push((source.path.clone(), open_for_text(&source.path)?));
            }
        }
        let Some((_, doc)) = self.docs.last_mut() else { return Ok(None) };
        Ok(doc.page_characters(source.page, true, hidden_check).ok().map(|page| PageText::build(&page.chars, &page.boxes, page.frame, page.hidden_checked, page.copy_restricted)))
    }
}

struct Held<'a>(&'a HashMap<u32, PageText>);

impl Texts for Held<'_> {
    fn page_text(&self, page: u32) -> Option<&PageText> {
        self.0.get(&page)
    }
}

/// Works out what's picked, reading only the pages a drag starts and ends on.
fn range_of<A: SessionAccess>(pick: &Pick, pages: u32, reading: &mut Pages<A>) -> Result<Range, AgentError> {
    if pick.all {
        if pick.page.is_some() || pick.to_page.is_some() || pick.chars.is_some() || pick.at.is_some() {
            return Err(bad("`--all` is the whole document, so it takes no pages or places."));
        }
        if pages == 0 {
            return Err(no_such_page());
        }
        return Ok(Range::all(pages));
    }
    let page = pick.page.ok_or_else(|| bad("It needs `--page`, or `--all`."))?;
    let first = page.checked_sub(1).filter(|first| *first < pages).ok_or_else(no_such_page)?;
    let to_page = pick.to_page.unwrap_or(page);
    if to_page < page {
        return Err(bad("`--to-page` comes after `--page`. A drag that goes back up says where it starts with `--page` and where it ends with `--to-page`, the earlier page first, and comes out the same."));
    }
    let last = to_page.checked_sub(1).filter(|last| *last < pages).ok_or_else(no_such_page)?;
    if pick.chars.is_some() && pick.at.is_some() {
        return Err(bad("It's `--chars` or `--at`, not both."));
    }
    if pick.at.is_none() && (pick.to.is_some() || pick.unit.is_some()) {
        return Err(bad("`--to` and `--unit` go with `--at`."));
    }
    if let Some(word) = pick.chars {
        let (from, to) = char_range(word)?;
        return Ok(Range { start: Place { page: first, at: from }, end: Place { page: last, at: to } });
    }
    let Some(at) = pick.at else { return Ok(Range::pages(first, last)) };
    let unit = match pick.unit {
        None if pick.to.is_some() => Unit::Char,
        None | Some("word") => Unit::Word,
        Some("line") => Unit::Line,
        Some(_) => Unit::Char,
    };
    let clicks = match unit {
        Unit::Char => 1,
        Unit::Word => 2,
        Unit::Line => 3,
    };
    let mut selection = Selection::default();
    selection.press(point(at, first)?, clicks, false);
    if let Some(to) = pick.to {
        selection.drag(point(to, last)?);
    } else if last != first {
        return Err(bad("`--to-page` needs `--to`, to say where on that page."));
    }
    selection.release();
    for wanted in [first, last] {
        if !reading.ends.contains_key(&wanted) {
            let text = reading.read(wanted)?;
            if text.is_none() {
                reading.failed_ends.insert(wanted);
            }
            reading.ends.insert(wanted, text.unwrap_or_default());
        }
    }
    selection.range(&Held(&reading.ends)).ok_or_else(no_such_page)
}

/// Both commands: `rects` for what `select` gives page by page. `copy` has no use for them and goes as far as its limits.
pub(crate) fn run<A: SessionAccess>(access: &A, pick: &Pick, rects: bool) -> Result<Value, AgentError> {
    run_counted(access, pick, rects, &mut Counted::default())
}

/// What a command cost, for the tests that hold it to its limits.
#[derive(Debug, Default)]
struct Counted {
    reads: u32,
    most_workers: usize,
}

fn run_counted<A: SessionAccess>(access: &A, pick: &Pick, rects: bool, counted: &mut Counted) -> Result<Value, AgentError> {
    let (id, pages) = access.with(|s| {
        let id = select_tab(s, pick.tab)?;
        Ok::<_, AgentError>((id, s.pages_in(id)))
    })?;
    let mut reading = Pages { access, id, docs: Vec::new(), ends: HashMap::new(), failed_ends: HashSet::new() };
    let range = range_of(pick, pages, &mut reading)?;
    let (page_cap, room, time) = if rects { (MAX_SELECT_PAGES, MAX_SELECT_BYTES, Duration::from_secs(60)) } else { (MAX_COPY_PAGES, MAX_COPY_BYTES, MAX_COPY_TIME) };
    let started = Instant::now();
    let last_page = range.end.page.min(pages.saturating_sub(1));
    let mut copied = CopiedText::default();
    let mut listed = Vec::new();
    let mut ran_out = None;
    let (mut start_at, mut end_at) = (None, None);
    let mut page = range.start.page;
    // Pages read ahead for their edges, which the copy reaches next, and every edge read so far that a page still to come compares with.
    let mut ahead: HashMap<u32, PageText> = HashMap::new();
    let mut edges: HashMap<u32, Vec<EdgeLine>> = HashMap::new();
    while page <= last_page {
        if page - range.start.page >= page_cap || started.elapsed() >= time {
            ran_out = Some(page);
            break;
        }
        let text = match (reading.ends.remove(&page), ahead.remove(&page)) {
            (Some(_), _) if reading.failed_ends.contains(&page) => None,
            (Some(text), _) | (None, Some(text)) => Some(text),
            (None, None) => {
                counted.reads += 1;
                reading.read(page)?
            }
        };
        counted.most_workers = counted.most_workers.max(reading.docs.len());
        let Some(text) = text else {
            copied.add_failed_page();
            if rects {
                listed.push(json!({ "page": page + 1, "failed": true, "rects": [] }));
            }
            edges.insert(page, Vec::new());
            page += 1;
            continue;
        };
        // Its first page already says so, and the same goes for the rest: the permissions are the file's. Before anything else is read.
        if text.copy_restricted() && !pick.anyway && !rects {
            return Err(AgentError::new("copy_restricted", "This file's permissions ask that its text isn't copied.", exit::PROTECTED).then("Tell the user. `--anyway` copies it, once they've said to. Each use goes in the activity log."));
        }
        edges.insert(page, text.edge_lines().to_vec());
        for other in neighbour_pages(page, pages) {
            if edges.contains_key(&other) {
                continue;
            }
            // One the copy reaches is read as it would be then, and kept. One past the pick is read only for its edges, and a failure there is only less to compare with.
            let reaches = other > page && other <= last_page && !reading.ends.contains_key(&other);
            counted.reads += 1;
            let read = if reaches { reading.read(other)? } else { reading.read_checked(other, false).ok().flatten() };
            counted.most_workers = counted.most_workers.max(reading.docs.len());
            edges.insert(other, read.as_ref().map(|text| text.edge_lines().to_vec()).unwrap_or_default());
            if reaches && let Some(text) = read {
                ahead.insert(other, text);
            }
        }
        edges.retain(|seen, _| *seen + chrome::NEIGHBOURS >= page);
        let neighbours: Vec<&[EdgeLine]> = neighbour_pages(page, pages).filter_map(|other| edges.get(&other).map(Vec::as_slice)).collect();
        let leave_out = chrome_for(page, &text, &range, &neighbours);
        if rects {
            let (start, end) = range.on_page(page).unwrap_or((0, 0));
            let shapes: Vec<[f64; 4]> = text.rects(start, end).iter().map(|rect| [round(rect.x), round(rect.y), round(rect.width), round(rect.height)]).collect();
            let (width, height) = text.size();
            listed.push(json!({ "page": page + 1, "start": start.min(text.len()), "end": end.min(text.len()), "width": width, "height": height, "rects": shapes }));
        }
        if page == range.start.page {
            start_at = Some(range.start.at.min(text.len()));
        }
        if page == range.end.page {
            end_at = Some(range.end.at.min(text.len()));
        }
        let room_left = copied.add_page(page, &text, &range, room, &leave_out);
        if !room_left {
            break;
        }
        page += 1;
    }
    let withheld = copied.restricted && !pick.anyway;
    let stopped_at = copied.stopped_at.or(ran_out);
    let mut note = Vec::new();
    if withheld {
        note.push("This file's permissions ask that its text isn't copied, so the text is left out. `--anyway` includes it, once the user has said to.".to_string());
    }
    if copied.hidden_left_out > 0 {
        note.push(format!("{} characters of text that looks hidden are left out.", copied.hidden_left_out));
    }
    if copied.chrome_left_out > 0 {
        note.push(format!("{} characters of running heads, footers and page numbers are left out, as they repeat on the pages around. Pick only them to copy them.", copied.chrome_left_out));
    }
    if copied.text.is_empty() && !withheld && !range.is_empty() && copied.pages_failed == 0 && copied.pages_too_long == 0 {
        note.push("There's no text there to select. It may be a scan.".to_string());
    }
    let mut data = json!({
        "tab": id.0,
        "start": { "page": range.start.page + 1, "at": start_at },
        "end": { "page": range.end.page.min(last_page) + 1, "at": end_at },
        "chars": if withheld { None } else { Some(copied.text.chars().count()) },
        "hidden_left_out": copied.hidden_left_out,
        "chrome_left_out": copied.chrome_left_out,
        "pages_read": copied.pages_read,
        "pages_unchecked": copied.pages_unchecked,
        "pages_failed": copied.pages_failed,
        "pages_too_long": copied.pages_too_long,
        "stopped_at_page": stopped_at.map(|page| page + 1),
        "restricted": copied.restricted,
        "note": (!note.is_empty()).then(|| note.join(" ")),
        "untrusted": { "text": if withheld { None } else { Some(&copied.text) } },
    });
    if rects {
        data["pages"] = json!(listed);
    }
    Ok(data)
}

fn round(value: f32) -> f64 {
    (f64::from(value) * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Direct, Level, exit, run as run_command};
    use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
    use recto_session::Session;
    use recto_testpdf::{Page, TempPdf, pages, text_lines};
    use std::cell::RefCell;

    fn words(line: &str, count: usize) -> Vec<u8> {
        pages(&(0..count).map(|_| Page::new(text_lines(&[line]))).collect::<Vec<_>>())
    }

    fn opened(file: &TempPdf) -> Session<crate::DrawableLoader> {
        let mut session = Session::new(crate::DrawableLoader);
        run_command(&mut session, Level::Control, &Command::Open { path: file.0.clone(), page: None }).unwrap();
        session
    }

    fn whole<'a>() -> Pick<'a> {
        Pick { tab: "active", page: None, to_page: None, chars: None, at: None, to: None, unit: None, all: true, anyway: false }
    }

    fn restricted(count: usize) -> Vec<u8> {
        let mut document = lopdf::Document::load_mem(&words("Words that ask not to be copied", count)).unwrap();
        document.trailer.set("ID", lopdf::Object::Array(vec![lopdf::Object::string_literal("0123456789abcdef"), lopdf::Object::string_literal("0123456789abcdef")]));
        let state = EncryptionState::try_from(EncryptionVersion::V2 { document: &document, owner_password: "owner", user_password: "", key_length: 128, permissions: Permissions::all() - Permissions::COPYABLE }).unwrap();
        document.encrypt(&state).unwrap();
        let mut out = Vec::new();
        document.save_to(&mut out).unwrap();
        out
    }

    /// Failure: `recto copy --all` on a file that asks not to be copied reads all of it before refusing, for minutes on a long file, when its first page already says so.
    #[test]
    fn a_restricted_file_is_refused_at_its_first_page() {
        let file = TempPdf::new("restricted.pdf", &restricted(12));
        let mut session = opened(&file);
        let mut counted = Counted::default();
        let access = Direct(RefCell::new(&mut session), None);
        let refused = run_counted(&access, &whole(), false, &mut counted).unwrap_err();
        assert_eq!((refused.code, refused.exit), ("copy_restricted", exit::PROTECTED));
        assert_eq!(counted.reads, 1, "it knew at the first page");
    }

    /// Failure: a tab with many PDFs inserted keeps a worker running for each through `copy --all`, at up to hundreds of MiB each, since none is let go.
    #[test]
    fn a_copy_over_many_files_keeps_few_workers() {
        let file = TempPdf::new("main.pdf", &words("The main file", 1));
        let added: Vec<TempPdf> = (0..6).map(|at| TempPdf::new("added.pdf", &words(&format!("Added file {at}"), 1))).collect();
        let mut session = opened(&file);
        for one in &added {
            run_command(&mut session, Level::Control, &Command::PageInsert { tab: "active".into(), path: one.0.clone(), at: 1 }).unwrap();
        }
        let mut counted = Counted::default();
        let access = Direct(RefCell::new(&mut session), None);
        let answer = run_counted(&access, &whole(), false, &mut counted).unwrap();
        let text = answer["untrusted"]["text"].as_str().unwrap();
        for at in 0..6 {
            assert!(text.contains(&format!("Added file {at}")), "{text}");
        }
        assert_eq!(counted.reads, 7);
        assert!(counted.most_workers <= MAX_WORKERS, "{}", counted.most_workers);
    }

    /// Failure: the page a drag ends on can't be read, and the answer says "it may be a scan", which sends someone looking for the wrong thing. It says the page couldn't be read.
    #[test]
    fn an_end_page_that_cant_be_read_isnt_called_a_scan() {
        let file = TempPdf::new("shrinks.pdf", &words("Some words", 3));
        let mut session = opened(&file);
        std::fs::write(&file.0, words("Some words", 1)).unwrap();
        let access = Direct(RefCell::new(&mut session), None);
        let answer = run(&access, &Pick { page: Some(3), at: Some("100,100"), all: false, ..whole() }, true).unwrap();
        assert_eq!(answer["pages_failed"], 1, "{answer}");
        assert!(answer["note"].is_null() || !answer["note"].as_str().unwrap().contains("scan"), "{answer}");
    }
}
