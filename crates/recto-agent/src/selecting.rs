//! `recto select` and `recto copy`: what a click, a drag or a range of characters takes on a page, where it shows and what it copies. By the same methods as the app's page view, through `recto_index::select`, with a worker of its own for as long as the command takes.
//!
//! It holds one page's text at a time past the two pages a drag starts and ends on, so a copy of a whole document costs what its longest page does, plus the copy.

use crate::render::open_for_text;
use crate::{AgentError, SessionAccess, exit, no_such_page, select as select_tab, source_changed};
use recto_index::select::{CopiedText, MAX_COPY_BYTES, MAX_COPY_PAGES, MAX_COPY_TIME, PagePoint, PageText, Place, Range, Selection, Texts, Unit};
use recto_render_service::DocRenderer;
use recto_session::TabId;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
    docs: HashMap<PathBuf, DocRenderer>,
    /// What says where a drag's ends are. They're read once, and kept for as long as this command.
    ends: HashMap<u32, PageText>,
}

impl<A: SessionAccess> Pages<'_, A> {
    /// A page's text, or `None` if it couldn't be read, which is a page failing and not the command.
    fn read(&mut self, position: u32) -> Result<Option<PageText>, AgentError> {
        let id = self.id;
        let source = self.access.with(|s| s.page_source(id, position)).ok_or_else(no_such_page)?;
        if !source.is_current() {
            return Err(source_changed(&source.path));
        }
        if !self.docs.contains_key(&source.path) {
            let doc = open_for_text(&source.path)?;
            self.docs.insert(source.path.clone(), doc);
        }
        let Some(doc) = self.docs.get_mut(&source.path) else { return Ok(None) };
        Ok(doc.page_characters(source.page, true, true).ok().map(|page| PageText::build(&page.chars, &page.boxes, page.frame, page.hidden_checked, page.copy_restricted)))
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
            let text = reading.read(wanted)?.unwrap_or_default();
            reading.ends.insert(wanted, text);
        }
    }
    selection.range(&Held(&reading.ends)).ok_or_else(no_such_page)
}

/// Both commands: `rects` for what `select` gives page by page. `copy` has no use for them and goes as far as its limits.
pub(crate) fn run<A: SessionAccess>(access: &A, pick: &Pick, rects: bool) -> Result<Value, AgentError> {
    let (id, pages) = access.with(|s| {
        let id = select_tab(s, pick.tab)?;
        Ok::<_, AgentError>((id, s.pages_in(id)))
    })?;
    let mut reading = Pages { access, id, docs: HashMap::new(), ends: HashMap::new() };
    let range = range_of(pick, pages, &mut reading)?;
    let (page_cap, room, time) = if rects { (MAX_SELECT_PAGES, MAX_SELECT_BYTES, Duration::from_secs(60)) } else { (MAX_COPY_PAGES, MAX_COPY_BYTES, MAX_COPY_TIME) };
    let started = Instant::now();
    let last_page = range.end.page.min(pages.saturating_sub(1));
    let mut copied = CopiedText::default();
    let mut listed = Vec::new();
    let mut ran_out = None;
    let mut page = range.start.page;
    while page <= last_page {
        if page - range.start.page >= page_cap || started.elapsed() >= time {
            ran_out = Some(page);
            break;
        }
        let text = match reading.ends.remove(&page) {
            Some(text) => Some(text),
            None => reading.read(page)?,
        };
        let Some(text) = text else {
            copied.add_failed_page();
            if rects {
                listed.push(json!({ "page": page + 1, "failed": true, "rects": [] }));
            }
            page += 1;
            continue;
        };
        if rects {
            let (start, end) = range.on_page(page).unwrap_or((0, 0));
            let shapes: Vec<[f64; 4]> = text.rects(start, end).iter().map(|rect| [round(rect.x), round(rect.y), round(rect.width), round(rect.height)]).collect();
            let (width, height) = text.size();
            listed.push(json!({ "page": page + 1, "start": start.min(text.len()), "end": end.min(text.len()), "width": width, "height": height, "rects": shapes }));
        }
        let room_left = copied.add_page(page, &text, &range, room);
        if !room_left {
            break;
        }
        page += 1;
    }
    if copied.restricted && !pick.anyway && !rects {
        return Err(AgentError::new("copy_restricted", "This file's permissions ask that its text isn't copied.", exit::PROTECTED).then("Tell the user. `--anyway` copies it, once they've said to."));
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
    if copied.text.is_empty() && !withheld && !range.is_empty() && copied.pages_failed == 0 && copied.pages_too_long == 0 {
        note.push("There's no text there to select. It may be a scan.".to_string());
    }
    let mut data = json!({
        "tab": id.0,
        "start": { "page": range.start.page + 1, "at": range.start.at },
        "end": { "page": range.end.page.min(last_page) + 1, "at": range.end.at.min(u32::MAX - 1) },
        "chars": if withheld { None } else { Some(copied.text.chars().count()) },
        "hidden_left_out": copied.hidden_left_out,
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
