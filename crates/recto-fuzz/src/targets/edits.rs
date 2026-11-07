//! Random sequences of edits by the owner and by agents, against a plain model of what each should do. The owner's go through the session, and agents' through `recto_agent::execute` at Control, which is the route they really take, with its refusals.
//!
//! What must hold after every step: the page order is the model's, undo and redo give back exactly what was there, `is_edited` means it differs from the file, the owner's edits aren't lost to anything an agent can do below Full, and the caps hold.

use crate::guard::{Budget, within};
use crate::input::Take;
use crate::targets::{NamedLoader, facts_of};
use recto_agent::{Level, parse, run};
use recto_session::edit::{Author, MAX_HISTORY_BYTES, MAX_PAGES, MAX_UNDO, PageSlot};
use recto_session::{Session, TabId};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::Duration;

const BUDGET: Budget = Budget { time: Duration::from_secs(120), memory: 256 * 1024 * 1024 };
/// What one sequence may do, where a step costs 1 and more on a long document. Enough for a small document to go well past the undo cap, or for dozens of steps at the longest a document can be.
const MAX_COST: usize = 8192;

/// A state the page order was in, small enough to keep one per step.
fn fingerprint(slots: &[PageSlot]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    slots.len().hash(&mut hasher);
    for slot in slots {
        (slot.source, slot.page, slot.quarter_turns).hash(&mut hasher);
    }
    hasher.finish()
}

fn is_the_file(slots: &[PageSlot], file_pages: u32) -> bool {
    slots.len() == file_pages as usize && slots.iter().enumerate().all(|(place, slot)| slot.source == 0 && slot.quarter_turns == 0 && slot.page as usize == place)
}

/// Whose step, and what the document was before it.
#[derive(Clone, Copy)]
struct Past {
    before: u64,
    by: Author,
}

struct Model {
    id: TabId,
    file_pages: u32,
    slots: Vec<PageSlot>,
    undo: Vec<Past>,
    redo: Vec<Past>,
    /// The next page number a model of an added file gets from: sources are numbered as they're first added.
    sources: Vec<&'static str>,
}

impl Model {
    fn new(id: TabId, file_pages: u32) -> Self {
        Self { id, file_pages, slots: (0..file_pages).map(|page| PageSlot { source: 0, page, quarter_turns: 0 }).collect(), undo: Vec::new(), redo: Vec::new(), sources: Vec::new() }
    }

    fn commit(&mut self, before: u64, by: Author) {
        self.undo.push(Past { before, by });
        self.redo.clear();
    }

    fn forget(&mut self) {
        let (id, file_pages) = (self.id, self.file_pages);
        *self = Self::new(id, file_pages);
    }
}

/// What an edit does, as plainly as it can be said.
#[derive(Clone, Copy, Debug)]
enum Op {
    Move { start: u32, len: u32, to: u32 },
    Rotate { start: u32, len: u32, turns: i32 },
    Delete { start: u32, len: u32 },
    Insert { at: u32, file: &'static str, pages: u32 },
    Undo,
    Redo,
    Discard,
    Close,
}

const FILES: &[(&str, u32)] = &[("/fuzz/p1.pdf", 1), ("/fuzz/p2.pdf", 2), ("/fuzz/p7.pdf", 7), ("/fuzz/p300.pdf", 300), ("/fuzz/p4999.pdf", 4_999), ("/fuzz/p190000.pdf", 190_000), ("/fuzz/p200000.pdf", 200_000)];

fn read_op(take: &mut Take<'_>, pages: u32) -> Op {
    let pages = pages.max(1);
    let which = take.u8() % 16;
    // Mostly in range, since a refused step tests only the refusal.
    let place = |take: &mut Take<'_>, limit: u32| if take.u8() < 240 { u32::from(take.u16()) % limit.max(1) } else { take.pick(&[0, 1, limit, limit + 1, u32::MAX - 1, u32::MAX]) };
    let start = place(take, pages);
    let len = if take.u8() < 200 { 1 + u32::from(take.u8() % 3) } else { place(take, pages).saturating_add(1) };
    match which {
        0..=2 => Op::Move { start, len, to: place(take, pages) },
        3..=5 => Op::Rotate { start, len, turns: take.pick(&[1, 2, 3, -1, -2, 4, 0, 5, i32::MIN, i32::MAX]) },
        6 | 7 => Op::Delete { start, len },
        8 | 9 => {
            let (file, pages_in_it) = FILES[usize::from(take.u8()) % if take.u8() < 240 { 4 } else { FILES.len() }];
            Op::Insert { at: place(take, pages + 1), file, pages: pages_in_it }
        }
        10..=12 => Op::Undo,
        13 | 14 => Op::Redo,
        15 if take.u8() < 128 => Op::Discard,
        _ => Op::Close,
    }
}

/// Catches: any step that leaves the order other than asked; an undo that doesn't invert; `is_edited` out of step with what's on screen; an owner's step lost below Full; a cap that doesn't hold.
pub fn check(data: &[u8]) {
    within("a sequence of edits", BUDGET, || sequence(data));
}

fn sequence(data: &[u8]) {
    let mut take = Take(data);
    let file_pages = take.pick(&[1u32, 1, 2, 3, 5, 8, 20, 100, 5_000]);
    let mut session = Session::new(NamedLoader);
    let open = |session: &mut Session<NamedLoader>| session.open_with_facts(Path::new("/fuzz/doc.pdf"), facts_of(file_pages), None).expect("a stand-in document opens");
    let id = open(&mut session);
    let mut model = Model::new(id, file_pages);
    let mut cost = 0;
    while !take.is_empty() && cost < MAX_COST {
        let by = if take.u8() & 1 == 0 { Author::Owner } else { Author::Agent };
        let op = read_op(&mut take, session.pages_in(model.id));
        // A step can be said once and run many times, so a short input can reach the caps.
        let times = if take.u8() >= 240 { 1 + usize::from(take.u8()) * 4 } else { 1 };
        for _ in 0..times {
            step(&mut session, &mut model, by, op);
            assert_state_holds(&session, &model);
            if session.tab(model.id).is_none() {
                model = Model::new(open(&mut session), file_pages);
            }
            cost += 1 + model.slots.len() / 1024;
        }
    }
    // Everything still on the lists gives back what it says it will, all the way down, and then up again.
    end_game(&mut session, &mut model);
}

/// Runs one step, by whoever's making it, and mirrors it in the model.
fn step(session: &mut Session<NamedLoader>, model: &mut Model, by: Author, op: Op) {
    match by {
        Author::Owner => owner_step(session, model, op),
        Author::Agent => agent_step(session, model, op),
    }
    // The session lets go of a list with nothing left in it, and a new one starts its numbering again.
    if session.page_edits(model.id).is_none() {
        model.undo.clear();
        model.redo.clear();
        model.sources.clear();
    }
}

fn owner_step(session: &mut Session<NamedLoader>, model: &mut Model, op: Op) {
    let id = model.id;
    let before = fingerprint(&model.slots);
    match op {
        Op::Move { start, len, to } => {
            let result = session.edit_pages(id, |edits| edits.move_pages(start, len, to));
            mirror(model, result.is_ok(), before, Author::Owner, op);
        }
        Op::Rotate { start, len, turns } => {
            let result = session.edit_pages(id, |edits| edits.rotate(start, len, turns));
            mirror(model, result.is_ok(), before, Author::Owner, op);
        }
        Op::Delete { start, len } => {
            let result = session.edit_pages(id, |edits| edits.delete(start, len));
            mirror(model, result.is_ok(), before, Author::Owner, op);
        }
        Op::Insert { at, file, pages } => {
            let result = session.insert_pdf_with_facts(id, Author::Owner, at, Path::new(file), &facts_of(pages), None);
            mirror(model, result.is_ok(), before, Author::Owner, op);
        }
        Op::Undo => {
            let undone = session.undo_edit(id);
            mirror_undo(session, model, undone);
        }
        Op::Redo => {
            let redone = session.redo_edit(id);
            mirror_redo(session, model, redone);
        }
        Op::Discard => {
            session.discard_edits(id);
            model.forget();
        }
        Op::Close => {
            // The app's own close asks first, and throws away only once told to.
            if session.close(id).is_ok() {
                model.forget();
            }
        }
    }
}

fn agent_step(session: &mut Session<NamedLoader>, model: &mut Model, op: Op) {
    let tab = format!("#{}", model.id.0);
    let before = fingerprint(&model.slots);
    let line = match op {
        Op::Move { start, len, to } => format!(r#"{{"cmd":"page_move","tab":"{tab}","pages":"{}","to":{}}}"#, span(start, len), to.saturating_add(1)),
        Op::Rotate { start, len, turns } => format!(r#"{{"cmd":"page_rotate","tab":"{tab}","pages":"{}","degrees":{}}}"#, span(start, len), turns.saturating_mul(90)),
        Op::Delete { start, len } => format!(r#"{{"cmd":"page_delete","tab":"{tab}","pages":"{}"}}"#, span(start, len)),
        Op::Insert { at, file, .. } => format!(r#"{{"cmd":"page_insert","tab":"{tab}","path":"{file}","at":{}}}"#, at.saturating_add(1)),
        Op::Undo => format!(r#"{{"cmd":"undo","tab":"{tab}"}}"#),
        Op::Redo => format!(r#"{{"cmd":"redo","tab":"{tab}"}}"#),
        Op::Discard => format!(r#"{{"cmd":"discard","tab":"{tab}"}}"#),
        Op::Close => format!(r#"{{"cmd":"close","tab":"{tab}"}}"#),
    };
    let command = parse(&line).unwrap_or_else(|_| panic!("a command written here: {line}"));
    let holds_owner_before = session.page_edits(model.id).is_some_and(|edits| edits.holds_owner_edits());
    let owner_undone_before = session.page_edits(model.id).is_some_and(|edits| edits.owner_steps_undone());
    let result = run(session, Level::Control, &command);
    match op {
        Op::Move { .. } | Op::Rotate { .. } | Op::Delete { .. } | Op::Insert { .. } => {
            // Below Full, an edit that would clear an owner's undone step is refused, and changes nothing.
            if owner_undone_before {
                assert!(result.is_err(), "{line} cleared the owner's undone steps");
            }
            mirror(model, result.is_ok(), before, Author::Agent, op);
        }
        Op::Undo => mirror_undo(session, model, result.is_ok()),
        Op::Redo => mirror_redo(session, model, result.is_ok()),
        Op::Discard => {
            if holds_owner_before {
                assert!(result.is_err(), "an agent at Control threw away the owner's edits");
            } else if result.is_ok() {
                model.forget();
            }
        }
        Op::Close => {
            if result.is_ok() {
                assert!(!owner_undone_before, "an agent closed a tab, losing the owner's undone steps");
                model.forget();
                model.id = TabId(u32::MAX);
            }
        }
    }
    // The heart of it. What an agent does can move the owner's steps around, and can't make them stop counting, unless the document is back at the file with nothing left to say so.
    let edits = session.page_edits(model.id);
    if holds_owner_before && !matches!(op, Op::Close) {
        let holds_now = edits.is_some_and(|edits| edits.holds_owner_edits());
        assert!(holds_now || !session.is_edited(model.id), "{line} lost the owner's edits");
    }
}

fn span(start: u32, len: u32) -> String {
    let first = u64::from(start) + 1;
    let last = first + u64::from(len) - 1;
    if len <= 1 { first.to_string() } else { format!("{first}-{last}") }
}

/// What the model expects of an edit that was taken. A step that does nothing isn't a step.
fn mirror(model: &mut Model, taken: bool, before: u64, by: Author, op: Op) {
    if !taken {
        return;
    }
    let slots = &mut model.slots;
    match op {
        Op::Move { start, len, to } => {
            if start == to {
                return;
            }
            let run: Vec<PageSlot> = slots.drain(start as usize..(start + len) as usize).collect();
            slots.splice(to as usize..to as usize, run);
        }
        Op::Rotate { start, len, turns } => {
            let turns = turns.rem_euclid(4) as u8;
            if turns == 0 {
                return;
            }
            for slot in &mut slots[start as usize..(start + len) as usize] {
                slot.quarter_turns = (slot.quarter_turns + turns) % 4;
            }
        }
        Op::Delete { start, len } => {
            slots.drain(start as usize..(start + len) as usize);
        }
        Op::Insert { at, file, pages } => {
            let source = match model.sources.iter().position(|known| *known == file) {
                Some(found) => found + 1,
                None => {
                    model.sources.push(file);
                    model.sources.len()
                }
            } as u16;
            slots.splice(at as usize..at as usize, (0..pages).map(|page| PageSlot { source, page, quarter_turns: 0 }));
        }
        Op::Undo | Op::Redo | Op::Discard | Op::Close => unreachable!("not edits"),
    }
    model.commit(before, by);
}

fn mirror_undo(session: &Session<NamedLoader>, model: &mut Model, undone: bool) {
    if !undone {
        // Steps may have fallen off the bottom of the list, which the model doesn't try to predict. What's left in the model is what fell off.
        model.undo.clear();
        return;
    }
    let past = model.undo.pop().expect("the session undid a step the model never saw");
    let now = session.page_edits(model.id).map_or_else(|| fingerprint(&identity(model.file_pages)), |edits| fingerprint(edits.slots()));
    assert_eq!(now, past.before, "undo didn't give back what was there before the step");
    model.redo.push(Past { before: fingerprint(&model.slots), by: past.by });
    model.slots = session.page_edits(model.id).map_or_else(|| identity(model.file_pages), |edits| edits.slots().to_vec());
}

fn mirror_redo(session: &Session<NamedLoader>, model: &mut Model, redone: bool) {
    if !redone {
        // Only agents' steps may have gone from under an undone step: an owner's must never be dropped.
        assert!(model.redo.iter().all(|past| past.by == Author::Agent), "an undone step of the owner's was dropped");
        model.redo.clear();
        return;
    }
    let past = model.redo.pop().expect("the session redid a step the model never saw");
    let now = session.page_edits(model.id).map_or_else(|| fingerprint(&identity(model.file_pages)), |edits| fingerprint(edits.slots()));
    assert_eq!(now, past.before, "redo didn't give back what was undone");
    model.undo.push(Past { before: fingerprint(&model.slots), by: past.by });
    model.slots = session.page_edits(model.id).map_or_else(|| identity(model.file_pages), |edits| edits.slots().to_vec());
}

fn identity(pages: u32) -> Vec<PageSlot> {
    (0..pages).map(|page| PageSlot { source: 0, page, quarter_turns: 0 }).collect()
}

fn assert_state_holds(session: &Session<NamedLoader>, model: &Model) {
    let Some(tab) = session.tab(model.id) else { return };
    assert_eq!(session.pages_in(model.id) as usize, model.slots.len(), "the page count isn't the model's");
    assert!(tab.page < session.pages_in(model.id).max(1), "the reader's place is off the end of the document");
    let edits = session.page_edits(model.id);
    match edits {
        Some(edits) => {
            assert_eq!(edits.slots(), model.slots, "the page order isn't the model's");
            assert!(edits.history_bytes() <= MAX_HISTORY_BYTES + MAX_PAGES * size_of::<PageSlot>(), "history holds {}", edits.history_bytes());
            assert_eq!(edits.is_edited(), !is_the_file(&model.slots, model.file_pages), "`is_edited` isn't whether it differs from the file");
            assert!(!edits.can_undo() || !model.undo.is_empty(), "the session has undo steps the model doesn't");
            assert!(!edits.can_redo() || !model.redo.is_empty(), "the session has redo steps the model doesn't");
            // Anything undone of the owner's is a reason not to discard, whoever undid it.
            if edits.owner_steps_undone() {
                assert!(edits.holds_owner_edits());
            }
            assert!(!edits.history_dropped() || edits.is_edited());
        }
        None => {
            assert!(is_the_file(&model.slots, model.file_pages), "no list, but the model has edits");
            assert!(!session.is_edited(model.id));
        }
    }
    assert!(model.slots.len() <= MAX_PAGES.max(model.file_pages as usize), "{} pages", model.slots.len());
}

fn end_game(session: &mut Session<NamedLoader>, model: &mut Model) {
    let id = model.id;
    let mut undone = 0;
    while session.undo_edit(id) {
        mirror_undo(session, model, true);
        assert_state_holds(session, model);
        undone += 1;
    }
    assert!(undone <= MAX_UNDO, "{undone} steps undone");
    // With nothing left to undo, anything still differing is what fell off the list, and it has to say so.
    if session.is_edited(id) {
        assert!(session.page_edits(id).is_some_and(|edits| edits.history_dropped()), "nothing left to undo, it still differs, and nothing says why");
    }
    while session.redo_edit(id) {
        mirror_redo(session, model, true);
        assert_state_holds(session, model);
    }
    assert!(model.redo.iter().all(|past| past.by == Author::Agent), "an undone step of the owner's was dropped");
    session.discard_edits(id);
    assert!(session.page_edits(id).is_none(), "discarding keeps nothing");
    assert!(!session.is_edited(id));
}
