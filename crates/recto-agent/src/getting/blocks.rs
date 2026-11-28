//! `get --grain paragraph|section|chapter`: the glue between the pure unit builder (`recto_index::blocks::unit`) and a session. It opens the finished corpus, the stored blocks, the contents and the file once each,
//! reads pages for the builder one at a time (the index first, a live read when the index didn't check the page for hidden text, and those for at most [`LIVE_BUDGET`]), and turns the places the builder names into
//! references. Everything that decides is in the builder; nothing here is kept between calls.
//!
//! Sections come from the tab's contents: the file's own when `verdict::judge` says `Fine`, else generated ones **if they are already made** (an ask does one thing: `get` never starts them).

use super::{Asked, NO_PAGE_TEXT, PageRead, changed, index_is_enough, read_live};
use crate::commands::getting::{Form, Grain as AskedGrain};
use crate::locating::Located;
use crate::reading::TabFile;
use crate::{AgentError, Code, Event, SessionAccess, exit};
use recto_core::locator::{self, Locator, Referrer, ScanCaps, Target};
use recto_core::{Toc, nav::Frame, verdict};
use recto_index::blocks::sections::Sections;
use recto_index::blocks::unit::{self, Ask, Grain, Mark, Outcome, Pages, Why};
use recto_index::blocks::{Block, markdown};
use recto_index::corpus::{CorpusKey, PageKind, ReadyCorpus, StoredBlocks};
use recto_session::TabId;
use recto_session::identify::Opened;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Time given to reading pages live for one answer. Past it reading stops, the answer is cut and says where it goes on.
const LIVE_BUDGET: Duration = Duration::from_secs(5);
/// The sealed page's buffer is given back past this many bytes.
const BIG_BUFFER: usize = 1 << 20;

/// What a blocks grain comes to.
pub(super) enum Reached {
    Answer(Box<Value>),
    /// The page is answered, with this `why` and (when blocks were refused or failed) Rust's words for it.
    Widen {
        why: &'static str,
        note: Option<&'static str>,
        /// The reference's page as it was read for the builder, when it was: the page answer is made from it, and nothing is read or drawn twice.
        read: Option<PageRead>,
    },
}

const NO_CONTENTS_NOTE: &str = "The file has no usable contents of its own, and none were made from its headings yet: `recto contents --generated` makes them.";

fn widen(why: &'static str, read: Option<PageRead>) -> Result<Reached, AgentError> {
    Ok(Reached::Widen { why, note: None, read })
}

/// The contents sections are cut by, and which they are: the file's own if they are good, else generated ones that have been made.
fn contents_for<A: SessionAccess>(access: &A, file: &TabFile, key: CorpusKey) -> Option<(Toc, &'static str)> {
    if file.renderer_only() {
        return None;
    }
    let loader = access.with(|s| s.loader().clone());
    if let Ok(facts) = recto_session::DocLoader::load(&loader, file.path())
        && !facts.renderer_only
        && !facts.toc.entries().is_empty()
        && matches!(verdict::judge(&facts.toc, facts.page_count), verdict::ContentsVerdict::Fine)
    {
        return Some((facts.toc, "file"));
    }
    let made = crate::generated::find(access.generating()?, file.path(), key)?;
    (!made.entries.is_empty()).then(|| (made.toc(), "generated"))
}

/// The pages of one tab's file, as the builder reads them.
struct Source<'a, A: SessionAccess> {
    access: &'a A,
    events: &'a mut Vec<Event>,
    id: TabId,
    file: &'a TabFile,
    corpus: ReadyCorpus,
    stored: StoredBlocks,
    opened: &'a Opened,
    live_spent: Duration,
    /// The reference's page, read before the builder starts to check the reference against it, kept for the builder and for a page answer.
    anchor: Option<(u32, PageRead)>,
    key: CorpusKey,
    /// What was read, in all: checked for hidden text everywhere, some stretch not marked, some page read from the file.
    checked: bool,
    partial: bool,
    live: bool,
    buffer: Vec<u8>,
}

impl<A: SessionAccess> Source<'_, A> {
    /// The sealed page's buffer is overwritten after each page, and given back past a page's worth: what is read lives only in what is given.
    fn wipe(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.buffer);
        if self.buffer.capacity() > BIG_BUFFER {
            self.buffer = Vec::new();
        }
    }

    /// One page's text: from the index when it was checked for hidden text and nothing was dropped, else live. `None` for a page the index says has no text.
    fn fetch(&mut self, page: u32) -> Result<Option<PageRead>, AgentError> {
        let indexed = match self.corpus.page(page, &mut self.buffer) {
            Ok(stored) if stored.kind != PageKind::Text => Some(None),
            Ok(stored) if index_is_enough(stored.hidden_checked, stored.spans_dropped) => Some(Some(PageRead {
                text: stored.text.to_owned(),
                hidden: stored.hidden_spans(),
                key: Some(self.key),
                checked: true,
                partial: false,
            })),
            _ => None,
        };
        self.wipe();
        if let Some(read) = indexed {
            return Ok(read);
        }
        let started = Instant::now();
        let read = read_live(self.file.path(), page);
        self.live_spent += started.elapsed();
        self.live = true;
        if read.as_ref().map_or(true, |read| read.checked)
            && let Some(position) = self.file.place_of(page)
        {
            crate::note_draw(
                self.access,
                self.id,
                position,
                read.as_ref().err().map(|error| error.code.as_str()),
                self.events,
            );
        }
        read.map(Some)
    }

    /// Reads the reference's page ahead of the builder, so a failure to read it is the answer and a live read isn't made twice. Gives the text for the reference's check.
    fn hold(&mut self, page: u32) -> Result<&str, AgentError> {
        let read = self
            .fetch(page)?
            .ok_or_else(|| AgentError::new(Code::PAGE_UNREADABLE, NO_PAGE_TEXT, exit::FAILED))?;
        self.take_in(&read);
        Ok(&self.anchor.insert((page, read)).1.text)
    }

    fn take_in(&mut self, read: &PageRead) {
        self.checked &= read.checked;
        self.partial |= read.partial;
    }
}

impl<A: SessionAccess> Pages for Source<'_, A> {
    fn pages(&self) -> u32 {
        self.stored.made().pages
    }

    fn read(&mut self, page: u32, chars: &mut Vec<char>, hidden: &mut Vec<(u32, u32)>) -> bool {
        chars.clear();
        hidden.clear();
        if let Some((_, read)) = self.anchor.as_ref().filter(|(held, _)| *held == page) {
            chars.extend(read.text.chars());
            hidden.extend_from_slice(&read.hidden);
            return true;
        }
        match self.fetch(page) {
            Ok(Some(read)) => {
                self.take_in(&read);
                chars.extend(read.text.chars());
                *hidden = read.hidden;
                true
            }
            _ => false,
        }
    }

    fn blocks(&mut self, page: u32, chars: u32) -> Result<Vec<Block>, unit::Fault> {
        self.stored.page(page, chars).map(|found| found.blocks).map_err(|_| unit::Fault)
    }

    fn frame(&mut self, page: u32) -> Option<Frame> {
        recto_core::page_id::page_frame(self.opened.reader(), self.opened.map(), page)
    }

    fn shown(&self, page: u32) -> bool {
        self.file.place_of(page).is_some()
    }

    fn over(&mut self) -> bool {
        self.live_spent >= LIVE_BUDGET
    }
}

/// A reference to a run of characters of a page, as `get`'s references are: the page's own, with these characters.
pub(super) fn token(referrer: &mut Referrer<'_>, mark: Option<Mark>) -> Value {
    mark.and_then(|mark| {
        let page = referrer.page(mark.page).ok()?;
        Some(json!(locator::encode(&Locator::new(
            page.document(),
            page.page(),
            Target::Characters(mark.range)
        ))))
    })
    .unwrap_or(Value::Null)
}

fn kind_word(grain: AskedGrain) -> &'static str {
    match grain {
        AskedGrain::Paragraph => "paragraph",
        AskedGrain::Section => "section",
        _ => "chapter",
    }
}

fn unit_grain(grain: AskedGrain) -> Grain {
    match grain {
        AskedGrain::Paragraph => Grain::Paragraph,
        AskedGrain::Section => Grain::Section,
        _ => Grain::Chapter,
    }
}

/// `get` at a blocks grain: the unit, or why the page is answered.
pub(super) fn answer<A: SessionAccess>(
    access: &A,
    events: &mut Vec<Event>,
    asked: &Asked<'_>,
    found: &Located,
    (id, file): (TabId, &TabFile),
) -> Result<Reached, AgentError> {
    file.require_pages_shown()?;
    let key = CorpusKey::of_file(file.path()).map_err(|_| changed())?;
    let (corpus, stored) = match crate::blocks::readable(access, file.path()) {
        crate::blocks::Readable::Ready(pair) => *pair,
        crate::blocks::Readable::Widen { why, note } => return Ok(Reached::Widen { why, note, read: None }),
    };
    let needs_contents = asked.grain != AskedGrain::Paragraph;
    let contents = contents_for(access, file, key);
    if needs_contents && contents.is_none() {
        return Ok(Reached::Widen {
            why: "no_contents",
            note: Some(NO_CONTENTS_NOTE),
            read: None,
        });
    }
    let sections = contents
        .as_ref()
        .map(|(toc, _)| Sections::with_tops(toc.entries(), |entry| recto_session::entry_place(entry, file.placed())));
    let opened = Opened::open(file.path(), found.stamp).ok_or_else(changed)?;
    let mut source = Source {
        access,
        events,
        id,
        file,
        corpus,
        stored,
        opened: &opened,
        live_spent: Duration::ZERO,
        anchor: None,
        key,
        checked: true,
        partial: false,
        live: false,
        buffer: Vec::new(),
    };
    // The reference's characters are checked against the text the answer is made from.
    found.held_in(source.hold(found.file_page)?)?;
    let start = found.characters().map(|(start, _)| start);
    let one = Ask {
        grain: unit_grain(asked.grain),
        page: found.file_page,
        start,
        from_here: asked.from_here,
        max_chars: asked.max_chars,
        sections: sections.as_ref(),
    };
    let built = match unit::build(&mut source, &one) {
        Ok(Outcome::Unit(built)) => built,
        Ok(Outcome::Page(Why::NoBlockHere)) => return widen("no_block_here", source.anchor.take().map(|(_, read)| read)),
        Ok(Outcome::Page(Why::NoSection)) => return widen("no_section_here", source.anchor.take().map(|(_, read)| read)),
        Err(unit::Fault) => {
            let (why, note) = crate::blocks::throw_away_and_ask(access, file.path());
            return Ok(Reached::Widen {
                why,
                note,
                read: source.anchor.take().map(|(_, read)| read),
            });
        }
    };
    let (checked, partial, live) = (source.checked, source.partial, source.live);
    drop(source);

    let caps = ScanCaps::STANDARD;
    let mut referrer = Referrer::new(opened.reader(), opened.map(), &caps);
    let refs = &built.refs;
    let up_grain = match asked.grain {
        AskedGrain::Paragraph => json!("section"),
        AskedGrain::Section => json!("chapter"),
        _ => Value::Null,
    };
    let refs = json!({
        "self": token(&mut referrer, refs.this), "next": token(&mut referrer, refs.next), "prev": token(&mut referrer, refs.prev),
        "more": token(&mut referrer, refs.more), "up": token(&mut referrer, refs.up), "up_grain": up_grain,
    });
    drop(opened);

    let mut untrusted = json!({ "link": locator::link_of(file.path(), found.file_page + 1).ok() });
    let mut data_hidden = Value::Null;
    match asked.form {
        Form::Text => {
            let (text, spans) = unit::as_text(&built.given);
            untrusted["text"] = json!(text);
            data_hidden = json!(spans.iter().map(|span| [span.0, span.1]).collect::<Vec<_>>());
        }
        Form::Markdown => untrusted["markdown"] = json!(markdown::render(&unit::render_pieces(&built.given))),
    }
    let headings: Vec<Value> = built
        .headings
        .iter()
        .filter_map(|entry| {
            let (toc, source) = contents.as_ref()?;
            let entry = toc.entries().get(*entry)?;
            Some(json!({ "depth": entry.depth, "source": source, "untrusted": { "title": toc.title(entry) } }))
        })
        .collect();
    let spans_given: usize = built.given.iter().map(|piece| piece.hidden.len()).sum();
    let mut unit_json = json!({ "kind": kind_word(asked.grain), "pages": built.pages, "chars": built.chars });
    if asked.grain == AskedGrain::Paragraph {
        unit_json["continues"] = json!(built.continues);
    }
    let counts = &built.counts;
    // The same version of the file, and of the index it was read from, to the end.
    crate::locating::still_located(access, found)?;
    if CorpusKey::of_file(file.path()).ok() != Some(key) {
        return Err(changed());
    }
    let mut data = json!({
        "tab": file.id(),
        "grain": asked.grain.word(),
        "as": asked.form.word(),
        "ref": found.told(),
        "unit": unit_json,
        "widened": Value::Null,
        "headings_above": headings,
        "refs": refs,
        "truncated": built.truncated,
        "counts": {
            "chars_in_unit": built.chars, "chars_given": built.chars_given, "hidden_spans": spans_given, "blocks": counts.blocks,
            "chars_outside_blocks": counts.chars_outside_blocks, "pages_read": counts.pages_read, "pages_left_out": counts.pages_left_out,
            "whole_edge_pages": counts.whole_edge_pages, "pages_without_frame": counts.pages_without_frame,
        },
        "hidden_text": { "checked": checked, "partial": partial },
        "read_from": if live { "file" } else { "index" },
        "untrusted": untrusted,
    });
    if !data_hidden.is_null() {
        data["hidden_spans"] = data_hidden;
    }
    Ok(Reached::Answer(Box::new(data)))
}
