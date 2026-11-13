//! **What the page view's types let other code reach,** listed and held (R3.6). `Reader` and its parts, `Engine` and its facades: every method visible
//! past its own module (`pub` or `pub(crate)`) with its signature, every trait implemented, and every field of `Reader` and `Engine` visible past theirs.
//! A part that can be built, replaced or reached around from outside where it belongs shows up here as a changed line, so a widening is a deliberate edit
//! of [`SURFACE`] that a reviewer sees, not a side effect of a move (the lesson of `Session`'s R3.5: a public `Default` and a `_mut` accessor let outside
//! code replace a whole part, and nothing listed it).
//!
//! A text scan of the sources, as the ratchet's is: an item under `#[cfg(test)]` isn't listed, and a signature is joined onto one line.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The files of the page view's state and its engine.
fn sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![root.join("tiles.rs"), root.join("engine.rs")];
    for dir in ["tiles", "engine"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Whether a line, trimmed, says its item is reached past its module.
fn visibility(line: &str) -> Option<&'static str> {
    if line.starts_with("pub(crate) ") {
        Some("pub(crate)")
    } else if line.starts_with("pub ") {
        Some("pub")
    } else {
        None
    }
}

/// The type an `impl` header is for, and the trait if it's a trait's: `impl<'a> Selection<'a> {` is `Selection`, `impl Default for Reader {` is `Reader: Default`.
fn impl_of(header: &str) -> String {
    let header = header.trim_end_matches('{').trim();
    let header = header.strip_prefix("impl").unwrap_or(header);
    let header = strip_generics(header.trim_start());
    let name = |text: &str| strip_generics(text.trim()).split("::").last().unwrap_or("").trim().to_string();
    match header.split_once(" for ") {
        Some((tr, ty)) => format!("{}: {}", name(ty), name(tr)),
        None => name(&header),
    }
}

/// Takes out every `<...>`, nested or not.
fn strip_generics(text: &str) -> String {
    let mut depth = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

/// What a file lets past its module, as lines of the listing.
fn listing_of(text: &str, into: &mut BTreeSet<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut at = 0;
    // The `impl` or `struct` being read: its name, and whether the struct's fields are listed.
    let mut within: Option<String> = None;
    let mut fields_of: Option<String> = None;
    let mut test_item = false;
    // What a `#[derive(..)]` just above the next item gives it: `Default` or `Clone` on a part is a way to build or replace one.
    let mut derives: Vec<String> = Vec::new();
    while at < lines.len() {
        let line = lines[at];
        let trimmed = line.trim();
        if trimmed.starts_with("#[cfg(test)]") {
            test_item = true;
            at += 1;
            continue;
        }
        if let Some(list) = trimmed.strip_prefix("#[derive(").and_then(|rest| rest.strip_suffix(")]")) {
            derives.extend(list.split(',').map(|name| name.trim().to_string()).filter(|name| !name.is_empty()));
            at += 1;
            continue;
        }
        if trimmed.starts_with("#[") || trimmed.starts_with("//") || trimmed.is_empty() {
            at += 1;
            continue;
        }
        // An item at the top of the file: a column-0 line.
        if !line.starts_with(' ') && !line.starts_with('}') {
            let skip = std::mem::take(&mut test_item);
            let derived = std::mem::take(&mut derives);
            within = None;
            fields_of = None;
            if skip {
                // The item and everything in it, to its closing brace at column 0, or to its `;`.
                if !trimmed.ends_with(';') {
                    while at < lines.len() && !lines[at].starts_with('}') {
                        at += 1;
                    }
                }
                at += 1;
                continue;
            }
            if trimmed.starts_with("impl") && trimmed.ends_with('{') {
                let of = impl_of(trimmed);
                if of.contains(": ") {
                    into.insert(format!("{of} (trait impl)"));
                }
                within = Some(of);
            } else if let Some((shown, kind, rest)) = type_item(trimmed) {
                let name = strip_generics(rest.split([' ', '{', '(', ';']).next().unwrap_or(""));
                if matches!(name.as_str(), "Reader" | "Engine") {
                    fields_of = Some(name.clone());
                }
                let derived = if derived.is_empty() {
                    String::new()
                } else {
                    format!(", derives {}", derived.join(", "))
                };
                into.insert(format!("{name} ({shown} {kind}{derived})"));
            } else if let Some(shown) = visibility(trimmed)
                && trimmed.contains(" fn ")
            {
                let mut signature = String::from(trimmed);
                while !signature.ends_with('{') && !signature.ends_with(';') && at + 1 < lines.len() {
                    at += 1;
                    signature.push(' ');
                    signature.push_str(lines[at].trim());
                }
                into.insert(format!("fn ({shown}) {}", one_line(&signature)));
            }
            at += 1;
            continue;
        }
        // A method, four spaces in, of the `impl` being read.
        if let Some(of) = &within
            && line.starts_with("    ")
            && !line.starts_with("     ")
        {
            let skip = std::mem::take(&mut test_item);
            let shown = visibility(trimmed);
            if trimmed.contains("fn ") && (shown.is_some() || of.contains(": ")) && !skip {
                let mut signature = String::from(trimmed);
                while !signature.ends_with('{') && !signature.ends_with(';') && at + 1 < lines.len() {
                    at += 1;
                    signature.push(' ');
                    signature.push_str(lines[at].trim());
                }
                into.insert(format!("{of}::{}", one_line(&signature)));
            }
            at += 1;
            continue;
        }
        // A field of `Reader` or `Engine`, visible past its module.
        if let Some(of) = &fields_of
            && line.starts_with("    ")
            && !line.starts_with("     ")
            && visibility(trimmed).is_some()
            && trimmed.contains(':')
        {
            into.insert(format!("{of}.{}", trimmed.trim_end_matches(',')));
        }
        test_item = false;
        at += 1;
    }
}

/// A struct or an enum, of any visibility: how visible, which, and the rest of its line.
fn type_item(line: &str) -> Option<(&'static str, &'static str, &str)> {
    const SHOWN: [(&str, &str); 4] = [("pub(crate) ", "pub(crate)"), ("pub(super) ", "pub(super)"), ("pub ", "pub"), ("", "private")];
    for (prefix, shown) in SHOWN {
        for (keyword, kind) in [("struct ", "struct"), ("enum ", "enum")] {
            if let Some(rest) = line.strip_prefix(prefix).and_then(|rest| rest.strip_prefix(keyword)) {
                return Some((shown, kind, rest));
            }
        }
    }
    None
}

/// A signature as one line: its body's brace gone, its whitespace one space.
fn one_line(signature: &str) -> String {
    let signature = signature.trim_end_matches('{').trim_end_matches(';').trim();
    signature
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(", )", ")")
}

fn listing() -> String {
    let mut lines = BTreeSet::new();
    for file in sources() {
        let text = std::fs::read_to_string(&file).unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        if text.contains("#![cfg(test)]") {
            continue;
        }
        listing_of(&text, &mut lines);
    }
    lines.into_iter().collect::<Vec<_>>().join("\n")
}

/// Failure: a move or an addition widens what other code can reach of the page view: a part's method made `pub(crate)`, a part given a trait other code
/// could build or replace it by (`Default`, `Clone`), a field of `Reader` or `Engine` made visible, a facade's method that takes a lock another way, or
/// one taken away that a caller needs. The list is updated by hand, in the commit that means it.
#[test]
fn what_the_page_view_lets_other_code_reach_is_listed() {
    let now = listing();
    if now != SURFACE.trim() {
        let was: BTreeSet<&str> = SURFACE.trim().lines().collect();
        let is: BTreeSet<&str> = now.lines().collect();
        let gone: Vec<&&str> = was.difference(&is).collect();
        let new: Vec<&&str> = is.difference(&was).collect();
        panic!("what the page view lets other code reach changed.\ngone: {gone:#?}\nnew: {new:#?}\nthe whole list now:\n{now}\n");
    }
}

/// Holds the scan itself: an item under `#[cfg(test)]` isn't listed, a signature across lines is joined, and a private method isn't listed.
#[test]
fn the_listing_reads_what_rust_says() {
    let mut lines = BTreeSet::new();
    listing_of(
        "pub struct Reader {\n    a: u32,\n    pub(crate) part: Part,\n}\n\nimpl Default for Part {\n    fn default() -> Self {\n        todo!()\n    }\n}\n\nimpl<'a> Facade<'a> {\n    pub fn press(\n        &self,\n        x: f64,\n    ) -> Vec<Op> {\n        todo!()\n    }\n\n    fn helper(&self) {}\n\n    #[cfg(test)]\n    pub fn only_in_tests(&self) {}\n}\n\n#[cfg(test)]\nimpl Part {\n    pub fn hidden(&self) {}\n}\n",
        &mut lines,
    );
    let got: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert_eq!(
        got,
        [
            "Facade::pub fn press(&self, x: f64) -> Vec<Op>",
            "Part: Default (trait impl)",
            "Part: Default::fn default() -> Self",
            "Reader (pub struct)",
            "Reader.pub(crate) part: Part",
        ]
    );
}

/// What the page view lets other code reach, sorted. Each change to it is deliberate: see [`what_the_page_view_lets_other_code_reach_is_listed`].
const SURFACE: &str = r#"
DocDrawing (pub(super) struct, derives Debug)
DocPlace (pub(super) struct, derives Debug)
DocView (pub(super) struct, derives Debug)
DrawKind (pub enum, derives Clone, Copy, Debug, PartialEq, Eq)
Ended (pub(super) enum, derives Clone, Copy, Debug, PartialEq, Eq)
Engine (pub struct, derives Clone)
Engine::pub fn activate(&self, tab: u32, pages: u32, page: u32, map: PageMap, warm: &[u32], saved: Option<SavedPlace>, spread: crate::layout::Spread)
Engine::pub fn applied(&self)
Engine::pub fn clear_selection(&self)
Engine::pub fn clear_selection_soon(&self) -> bool
Engine::pub fn copy_selection(&self, anyway: bool)
Engine::pub fn deactivate(&self)
Engine::pub fn dismiss_selection_notice(&self)
Engine::pub fn document_point(&self, page: u32, x: f64, y: f64) -> Option<(f64, f64)>
Engine::pub fn drain(&self) -> Vec<Op>
Engine::pub fn empty_discardable(&self) -> usize
Engine::pub fn fit(&self) -> Option<f64>
Engine::pub fn follow(&self, page: u32, within: f64) -> Option<u32>
Engine::pub fn follow_soon(&self, page: u32, within: f64) -> bool
Engine::pub fn give_sleep(&self, tab: u32, view: crate::tiles::SleepView)
Engine::pub fn has_document(&self) -> bool
Engine::pub fn has_selection(&self) -> bool
Engine::pub fn has_thread(&self) -> bool
Engine::pub fn hold_drawing(&self, on: bool)
Engine::pub fn holds(&self, tab: u32) -> bool
Engine::pub fn hot_selection(&self) -> Option<(u32, Option<recto_index::select::Range>)>
Engine::pub fn hot_tab(&self) -> Option<u32>
Engine::pub fn is_on_place(&self) -> bool
Engine::pub fn is_showable(&self) -> bool
Engine::pub fn jump_to(&self, page: u32)
Engine::pub fn jump_within(&self, page: u32, within: f64) -> Option<u64>
Engine::pub fn key_given(&self, path: &Path)
Engine::pub fn link_pages_held(&self) -> usize
Engine::pub fn magnification(&self) -> Option<f64>
Engine::pub fn memory_pressure(&self, critical: bool)
Engine::pub fn new(config: EngineConfig, events: Arc<dyn EngineEvents>) -> Self
Engine::pub fn note(&self) -> Option<String>
Engine::pub fn note_wake(&self, tab: u32, began: Instant)
Engine::pub fn own_file_page(&self, position: u32) -> Option<u32>
Engine::pub fn page_count(&self) -> u32
Engine::pub fn page_size_shown(&self, page: u32) -> Option<(f32, f32)>
Engine::pub fn page_turns(&self, page: u32) -> u8
Engine::pub fn places(&self) -> (u64, Vec<(u32, SavedPlace)>)
Engine::pub fn point_on_given_page(&self, page: u32, x: f64, y: f64) -> Option<recto_index::select::PagePoint>
Engine::pub fn point_on_page(&self, x: f64, y: f64) -> Option<(recto_index::select::PagePoint, (f32, f32))>
Engine::pub fn pointer_is_over_text(&self, x: f64, y: f64) -> bool
Engine::pub fn pointer_link(&self, x: f64, y: f64) -> Option<crate::links::LinkHit>
Engine::pub fn position_in_same_file(&self, from: u32, file_page: u32) -> Option<u32>
Engine::pub fn reading_anchor(&self) -> Option<(u32, f64)>
Engine::pub fn report(&self) -> ReaderReport
Engine::pub fn restore_selection(&self, from: (u32, u32), to: (u32, u32))
Engine::pub fn reveal(&self, page: u32, start: u32, end: u32)
Engine::pub fn same_as(&self, other: &Self) -> bool
Engine::pub fn select_all(&self)
Engine::pub fn selection_can_be_sent(&self) -> bool
Engine::pub fn selection_drag(&self, x: f64, y: f64) -> Vec<Op>
Engine::pub fn selection_notice(&self) -> Option<Notice>
Engine::pub fn selection_press(&self, x: f64, y: f64, clicks: u32, shift: bool) -> Vec<Op>
Engine::pub fn selection_release(&self) -> Vec<Op>
Engine::pub fn selection_text_now(&self) -> Option<String>
Engine::pub fn set_margins(&self, left: f64, right: f64)
Engine::pub fn set_paper(&self, paper: PaperKind)
Engine::pub fn set_share(&self, views: usize)
Engine::pub fn set_spread(&self, spread: crate::layout::Spread)
Engine::pub fn set_viewport(&self, viewport: Viewport) -> Vec<Op>
Engine::pub fn set_viewport_scrolled(&self, viewport: Viewport) -> ViewportOutcome
Engine::pub fn set_warm(&self, warm: &[u32])
Engine::pub fn shut_down(&self)
Engine::pub fn sleep_material(&self, tab: u32, now: &PageMap) -> Option<crate::tiles::SleepMaterial>
Engine::pub fn take_clicked_link(&self) -> Option<crate::links::LinkHit>
Engine::pub fn take_copied(&self) -> Option<recto_index::select::CopiedText>
Engine::pub fn take_keyless(&self) -> Vec<PathBuf>
Engine::pub fn take_moved_page(&self) -> Option<u32>
Engine::pub fn wait_settled(&self, limit: Duration) -> bool
Engine::pub fn walled_workers(&self) -> u32
Engine::pub fn window_closed(&self)
Engine::pub fn window_opened(&self)
Engine::pub fn window_up(&self)
Engine::pub fn worker_bytes(&self, tab: u32) -> u64
Engine::pub fn worker_pids(&self) -> Vec<u32>
Engine::pub fn workers_footprint(&self) -> (u64, u64)
Engine::pub fn zoom(&self, zoom: Zoom) -> Option<f64>
Engine::pub fn zoom_soon(&self, value: f64) -> bool
EngineConfig (pub struct, derives Clone, Debug, Default)
Entry (pub(super) struct, derives Debug)
HostQueue (pub(super) struct, derives Debug, Default)
Issue (pub(super) struct, derives Clone, Debug)
Job (pub struct, derives Debug)
JobKind (pub enum, derives Debug)
JobPlan (pub struct, derives Debug)
JobPlan::pub fn ready(self) -> Option<Job>
Keyring (pub struct, derives Default)
Keyring: Debug (trait impl)
Keyring: Debug::fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
Keyring::pub fn for_path(&self, path: &Path) -> Option<Arc<recto_core::Password>>
Keyring::pub fn get(&self, identity: (u64, u64)) -> Option<Arc<recto_core::Password>>
Keyring::pub fn is_empty(&self) -> bool
Keyring::pub fn keep_only(&self, open: &[(u64, u64)])
Keyring::pub fn len(&self) -> usize
Keyring::pub fn set(&self, identity: (u64, u64), password: Arc<recto_core::Password>)
LowResNote (pub(super) struct, derives Debug)
Next (pub(super) enum, derives Clone, Copy, Debug, PartialEq, Eq)
NoLook (private enum)
Op (pub enum, derives Clone, Debug, PartialEq)
OpenDoc (private struct)
Outcome (pub enum, derives Debug)
Owed (pub(super) struct, derives Debug)
PageMap (pub struct, derives Clone, Debug, Default, PartialEq, Eq)
PageNow (pub(super) struct, derives Clone, Copy, Debug, Default, PartialEq, Eq)
Pending (private struct, derives Default)
Place (pub(super) struct, derives Debug)
Placement (pub(super) struct, derives Clone, Copy, Debug, PartialEq)
PlanKind (pub(super) enum, derives Debug)
Pointer (pub(super) struct, derives Debug, Default)
RawPicture (pub struct, derives Clone, Debug, PartialEq)
Reader (pub struct)
Reader: Default (trait impl)
Reader: Default::fn default() -> Self
Reader::pub fn activate(&mut self, tab: u32, pages: u32, page: u32, map: PageMap, warm: &[u32], saved: Option<SavedPlace>, spread: Spread)
Reader::pub fn applied(&mut self)
Reader::pub fn clear_selection(&mut self)
Reader::pub fn counts(&self) -> &ReaderCounts
Reader::pub fn deactivate(&mut self)
Reader::pub fn dismiss_selection_notice(&mut self)
Reader::pub fn document_point(&self, page: u32, x: f64, y: f64) -> Option<(f64, f64)>
Reader::pub fn drain(&mut self, budget: usize) -> Vec<Op>
Reader::pub fn empty_discardable(&mut self) -> usize
Reader::pub fn entry_count(&self) -> usize
Reader::pub fn expected_stamp(&self, tab: u32, file: &std::path::Path) -> Option<FileStamp>
Reader::pub fn file_changed(&mut self, tab: u32)
Reader::pub fn files_of(&self, tab: u32) -> Vec<PathBuf>
Reader::pub fn finish(&mut self, job: Job, outcome: Outcome)
Reader::pub fn fit(&self) -> Option<f64>
Reader::pub fn follow(&mut self, page: u32, within: f64) -> Option<u32>
Reader::pub fn give_sleep(&mut self, tab: u32, view: SleepView)
Reader::pub fn has_document(&self) -> bool
Reader::pub fn has_ops(&self) -> bool
Reader::pub fn has_selection(&self) -> bool
Reader::pub fn hold_drawing(&mut self, on: bool)
Reader::pub fn holds(&self, tab: u32) -> bool
Reader::pub fn hot_tab(&self) -> Option<u32>
Reader::pub fn is_on_place(&self) -> bool
Reader::pub fn is_own_file(&self, tab: u32, file: &std::path::Path) -> bool
Reader::pub fn is_settled(&self) -> bool
Reader::pub fn is_showable(&self) -> bool
Reader::pub fn is_window_open(&self) -> bool
Reader::pub fn jump_to(&mut self, page: u32, within: f64) -> bool
Reader::pub fn key_given(&mut self, path: &std::path::Path)
Reader::pub fn last_wake(&self) -> Option<WakeReport>
Reader::pub fn link_pages_held(&self) -> usize
Reader::pub fn magnification(&self) -> Option<f64>
Reader::pub fn memory_pressure(&mut self, critical: bool)
Reader::pub fn new() -> Self
Reader::pub fn next_plan(&mut self) -> Option<JobPlan>
Reader::pub fn note(&self) -> Option<&str>
Reader::pub fn note_started(&mut self, job: &Job, cancel: Option<CancelHandle>)
Reader::pub fn note_version(&mut self, file: &std::path::Path, version: u64)
Reader::pub fn note_wake(&mut self, tab: u32, began: Instant)
Reader::pub fn own_file_page(&self, position: u32) -> Option<u32>
Reader::pub fn page_count(&self) -> u32
Reader::pub fn page_size_shown(&self, page: u32) -> Option<(f32, f32)>
Reader::pub fn page_turns(&self, page: u32) -> u8
Reader::pub fn places(&self) -> Vec<(u32, SavedPlace)>
Reader::pub fn point_on_given_page(&self, page: u32, x: f64, y: f64) -> Option<PagePoint>
Reader::pub fn point_on_page(&self, x: f64, y: f64) -> Option<PagePoint>
Reader::pub fn pointer_is_over_text(&mut self, x: f64, y: f64) -> bool
Reader::pub fn pointer_link(&mut self, x: f64, y: f64) -> Option<crate::links::LinkHit>
Reader::pub fn position_in_same_file(&self, from: u32, file_page: u32) -> Option<u32>
Reader::pub fn reading_anchor(&self) -> Option<(u32, f64)>
Reader::pub fn report(&self) -> ReaderReport
Reader::pub fn restore_selection(&mut self, range: recto_index::select::Range)
Reader::pub fn reveal_range(&mut self, range: recto_index::select::Range)
Reader::pub fn running_is_wanted(&self) -> bool
Reader::pub fn select_all(&mut self)
Reader::pub fn selection_can_be_sent(&self) -> bool
Reader::pub fn selection_drag(&mut self, x: f64, y: f64)
Reader::pub fn selection_notice(&self) -> Option<Notice>
Reader::pub fn selection_press(&mut self, x: f64, y: f64, clicks: u32, shift: bool)
Reader::pub fn selection_range(&self) -> Option<recto_index::select::Range>
Reader::pub fn selection_release(&mut self)
Reader::pub fn selection_text_now(&self) -> Option<String>
Reader::pub fn set_cancel(&mut self, cancel: CancelHandle)
Reader::pub fn set_margins(&mut self, left: f64, right: f64)
Reader::pub fn set_paper(&mut self, paper: PaperKind)
Reader::pub fn set_share(&mut self, views: usize)
Reader::pub fn set_spread(&mut self, spread: Spread)
Reader::pub fn set_viewport(&mut self, viewport: Viewport) -> Vec<Op>
Reader::pub fn set_warm(&mut self, warm: &[u32])
Reader::pub fn sleep_material(&mut self, tab: u32, now: &PageMap) -> Option<SleepMaterial>
Reader::pub fn spread(&self) -> Option<Spread>
Reader::pub fn start_copy(&mut self, anyway: bool)
Reader::pub fn take_clicked_link(&mut self) -> Option<crate::links::LinkHit>
Reader::pub fn take_copied(&mut self) -> Option<recto_index::select::CopiedText>
Reader::pub fn take_copy_signal(&mut self) -> bool
Reader::pub fn take_keyless(&mut self) -> Vec<PathBuf>
Reader::pub fn take_moved_page(&mut self) -> Option<u32>
Reader::pub fn take_reopen_files(&mut self) -> Vec<PathBuf>
Reader::pub fn take_user_scrolled(&mut self) -> bool
Reader::pub fn thread_restarted(&mut self, give_up: bool)
Reader::pub fn wanted_files(&self) -> Arc<Vec<PathBuf>>
Reader::pub fn window_closed(&mut self)
Reader::pub fn window_opened(&mut self)
Reader::pub fn window_up(&mut self)
Reader::pub fn zoom(&mut self, zoom: Zoom) -> Option<f64>
Reader::pub(crate) fn has_moved_page(&self) -> bool
ReaderCounts (pub struct, derives Clone, Debug, Default)
ReaderReport (pub struct)
Running (pub(super) struct, derives Debug)
SavedPlace (pub struct, derives Clone, Copy, Debug, PartialEq)
Scratch (pub(super) struct, derives Debug, Default)
SelectingNews (private struct)
Shared (private struct)
SleepDrawCounts (pub struct, derives Clone, Copy, Debug, Default, PartialEq, Eq)
SleepDraws (pub struct, derives Debug, Default)
SleepDraws::pub(crate) fn is_for(reader: &Reader, tab: u32) -> bool
SleepDraws::pub(crate) fn plan(reader: &mut Reader) -> Option<JobPlan>
SleepDraws::pub(crate) fn under_way(reader: &Reader) -> bool
SleepMaterial (pub struct, derives Clone, Debug, Default, PartialEq)
SleepShot (pub struct, derives Debug)
SleepView (pub struct, derives Debug, Default)
TabNow (pub(super) struct, derives Clone, Copy, Debug, Default, PartialEq, Eq)
TileCache (pub(super) struct, derives Debug)
TileKey (pub(super) struct, derives Clone, Copy, Debug, PartialEq, Eq, Hash)
Unopened (private enum)
Viewport (pub struct, derives Clone, Copy, Debug, Default, PartialEq)
ViewportOutcome (pub struct)
WakeReport (pub struct, derives Clone, Copy, Debug, Default)
WakeStock (pub enum, derives Clone, Copy, Debug, Default, PartialEq, Eq)
WakeStock::pub fn covered_the_view(self) -> bool
WakeStock::pub fn word(self) -> &'static str
Waking (pub(super) struct, derives Debug, Default)
Work (pub(super) struct, derives Debug)
Zoom (pub enum, derives Clone, Copy, Debug, PartialEq)
fn (pub) pub fn place_stamp() -> u64
fn (pub) pub fn typed_zoom(text: &str, decimal: char) -> Option<Zoom>
fn (pub) pub fn zoom_after_scroll(current: f64, points: f64) -> f64
fn (pub) pub fn zoom_percent(magnification: f64, fit: f64) -> f64
"#;
