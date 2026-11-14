//! Tabs, tiers and the sidebar: every rule the host needs, with no UI toolkit in sight.
//!
//! **Cost follows activity.** A cold tab is a [`TabRecord`] and nothing else: no handle, no contents, no pages. Opening
//! reads a PDF's facts and closes it. Only the active tab and the most recent others keep their contents, up to
//! [`MAX_WARM`], and anything evicted is rebuilt from disk when it's needed again.

pub mod change;
pub mod compare;
pub mod confined;
pub mod edit;
pub mod held;
pub mod palette;
pub mod record;
pub mod sleep;
pub mod trail;

mod contents;
mod dirs;
mod dormancy;
mod dormant;
mod editing;
mod files;
mod fit;
mod follow;
mod generated;
mod journal;
mod journals;
mod lists;
mod mail;
mod pages;
mod paper;
mod parts;
mod recent;
mod registry;
mod restore;
mod saves;
mod saving;
mod sleeper;
mod tabs;
mod tiers;
mod view;
mod viewing;
mod warm;

pub use change::Closed;
// The session's components, as the accessors in `parts.rs` hand them out: named here so a caller outside the crate can take one alone, and so their
// docs are read. Naming one gives no way to build or replace it (R3.5a: no public constructor, and changes go through a `PartMut`).
pub use dormancy::Dormancy;
pub use dormant::Dormant;
pub use edit::Author;
use edit::{EditError, PageEdits};
pub use fit::{EditsFit, NoticeSaid, Notices};
pub use follow::{ContentsFollow, ContentsRowChange, ContentsView, FOLLOW_RESUME_AFTER, FollowMove};
pub use generated::Generated;
pub use journal::{JournalDesk, JournalLook};
pub use journals::Journals;
pub use lists::Lists;
pub use mail::PageViewMail;
pub use pages::Pages;
pub use paper::{Paper, WARM_PAPER, contrast_ratio};
pub use parts::PartMut;
pub use recent::Recent;
use record::{
    EditsSummary, FileNotice, FileStatus, JournalHeader, JournalOp, MAX_TAB_ID, PathField, PlaceDelta, PlaceRecord, RecordError, Selection, SessionRecord,
    TabEntry,
};
use recto_core::verdict::ContentsVerdict;
use recto_core::{DocFacts, NONE, OpenError, Toc};
use recto_platform::{FileStamp, file_stamp};
pub use registry::Tabs;
#[cfg(debug_assertions)]
pub use registry::searched;
pub use saves::Saves;
pub use sleeper::Sleeper;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
pub use viewing::Viewing;
pub use warm::Warm;

/// Tabs that may keep their contents in memory: the active one plus two recent ones.
pub const MAX_WARM: usize = 3;
/// A cap on tabs, because everything grown from outside has one.
pub const MAX_TABS: usize = 10_000;
/// Files kept in File > Open Recent and offered to ⌘K: twice what a Mac app lists by default, and a few kilobytes at most.
pub const MAX_RECENT: usize = 20;

/// Reads a document's facts. A seam so tests can stand in for the disk.
pub trait DocLoader {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError>;
}

/// Reads from disk through the lazy reader.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiskLoader;

impl DocLoader for DiskLoader {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        recto_core::inspect(path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u32);

/// How much of a tab is in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// The record only.
    Cold,
    /// The record plus contents.
    Warm,
    /// The tab on screen.
    Hot,
}

/// One tab. Everything else about it can be rebuilt from disk.
// size: asserted <= 224 at compile time, below, and by `tab_record_stays_small`: its parts' 2 bytes of padding and none of its own. Held per tab, including cold ones.
#[derive(Debug)]
pub struct TabRecord {
    /// What's known of its file came from a sleep file, and the PDF hasn't said so yet: which version that file said it was of, or zero for none said.
    from_sleep: Option<u64>,
    /// When it was last active, edited or worked on, in seconds of the session. What going dormant goes by, with no timer of its own.
    touched: u32,
    pub id: TabId,
    /// The name shown in the sidebar: the file's name without its extension.
    pub name: Box<str>,
    /// Zero-based page the reader is on.
    pub page: u32,
    pub page_count: u32,
    pub toc_open: bool,
    /// Why this tab's file can't be read at the moment, if so.
    pub problem: Option<OpenError>,
    /// Only the renderer could read this file. See [`DocFacts::renderer_only`].
    pub renderer_only: bool,
    /// Quarter turns of the view: every page drawn turned, on top of what its edits turn. Set by QA's `view_rotation` and kept in the record.
    pub view_rotation: u8,
    /// How its pages are laid out: a page to a row, or two side by side.
    pub spread: record::Spread,
    /// Its place has changed since it was last written, so one small line says so.
    place_dirty: bool,
    /// What's thought of its own contents, worked out once when they're read, so the sidebar needn't at every page.
    contents_verdict: ContentsVerdict,
    /// The reader chose generated contents over the file's own. Kept in the record.
    pub generated_contents: bool,
    /// Sections the reader opened or closed by hand, as `(entry, open)`, against what following the place opens. Nothing for a tab nobody's folded, so a cold tab pays a pointer.
    folds: Option<Box<[(u32, bool)]>>,
    /// Its file: where it is, which version, and what's said of it.
    file: TabFile,
    /// Where the reader is in it, what's selected, and Back and Forward.
    reading: TabPlace,
}

/// A tab's file: where it is, which version of it the tab is about, whether it's protected, and what the reader's told of it.
// size: asserted <= 72 at compile time, 2 of them padding. Held per tab, including cold ones.
#[derive(Debug)]
struct TabFile {
    /// Its folder's number in the session's folders.
    dir: u32,
    file_name: Box<OsStr>,
    /// Which version of the file this tab's facts and any edits are about. `None` if it couldn't be looked at.
    stamp: Option<FileStamp>,
    /// What the reader should be told about this tab's file, and whether its edits are about another version of it ([`fit`]): rare enough to be a pointer.
    news: Option<Box<fit::FileNews>>,
    /// Its file's encrypted on disk, as the loader read it, which a file anyone may open with no password can be too (permissions only). Read in one
    /// place: a save's original, kept in the history as an added source, is marked protected by it (`saving.rs`), which never holds there today, as the
    /// writer refuses an encrypted file. `facts` and the writer read the file themselves, not this. It isn't what keeps a document's pages out of a sleep
    /// file or from an agent: that's a password having been needed (`password`, [`Tabs::withheld_from_agents`]).
    encrypted: bool,
    /// Its file was opened with a password the person typed, this session or one before (it's in the record). Unlike `encrypted`, which a file anyone may open has too, this means what's known of it was decrypted for the person, so nothing of it goes to an agent: see [`Tabs::withheld_from_agents`].
    password: bool,
}

/// Where the reader is in a tab: the exact place, what's selected, and Back and Forward.
// size: asserted <= 80 at compile time. Held per tab, including cold ones.
#[derive(Debug)]
struct TabPlace {
    /// Exactly where the reader was, so a tab that goes cold or across a restart comes back there. It's what's asked for on showing the tab, and what the page view says on leaving one.
    place: Option<PlaceRecord>,
    /// What's selected, as the page view last said: kept in the record, so it comes back across a restart.
    selection: Option<Box<Selection>>,
    /// Back and Forward, once someone's jumped in it: see [`trail`]. A pointer for a tab nobody has, and at most 100 stops of 12 bytes for one somebody has, counted in a cold tab's cost (`a_cold_tab_with_a_full_trail_stays_in_budget`).
    trail: Option<Box<trail::Trail>>,
}

/// A tab's record and its parts are paid for by every tab, cold ones too, a thousand times over (rule 1). Failure caught: a field added to a tab, or a part that brings padding of its own, grows every tab unnoticed. A contents part would take the tab to 232: don't make one. Numbers that fall are lowered here.
const _: () = {
    assert!(std::mem::size_of::<TabFile>() <= 72);
    assert!(std::mem::size_of::<TabPlace>() <= 80);
    assert!(std::mem::size_of::<TabRecord>() <= 224);
};

/// A trail stop's page, and which file and file page that was, if it was any.
type StopSource = (u32, Option<(u16, u32)>);

/// What an edit needs of before it: where the reader's page came from, each trail stop's, and whether the edits fitted the file.
struct Before {
    place: Option<(u16, u32, u32)>,
    stops: Option<Vec<StopSource>>,
    fit: EditsFit,
}

/// Where a page of a tab comes from now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageSource {
    pub path: PathBuf,
    /// Zero-based page in that file.
    pub page: u32,
    /// Unsaved quarter turns on it.
    pub quarter_turns: u8,
    /// It came from another PDF added to this one, rather than the tab's own file.
    pub added: bool,
    /// What the file was when its page numbers were recorded, if this page's number depends on that: an added file, or the tab's own file once it's been edited.
    pub expected: Option<Option<FileStamp>>,
}

impl PageSource {
    /// Whether the file is still the one the page number is about, by [`recorded_version_is_current`]. A file that's gone isn't.
    pub fn is_current(&self) -> bool {
        recorded_version_is_current(&self.path, self.expected)
    }
}

/// **Whether a file is still the version page numbers were recorded from: the one rule,** for a page of a tab ([`PageSource`]), a search's pages and an index's
/// files alike. `None`: nothing was recorded, as for a tab with no edits, so the file as it is now is what's wanted. `Some(None)`: they were recorded from a
/// version nothing names (its stamp couldn't be taken, or went as the edits don't fit the file), so nothing says this file is it, and it isn't taken to be, as
/// a save has it. `Some(Some(stamp))`: the file at the path must still be that.
pub fn recorded_version_is_current(path: &Path, expected: Option<Option<FileStamp>>) -> bool {
    match expected {
        None => true,
        Some(None) => false,
        Some(Some(stamp)) => file_stamp(path).ok() == Some(stamp),
    }
}

/// A row in the sidebar. Borrows its text from the session, so building the model allocates little.
// size: asserted <= 64 by `sidebar_row_stays_small`. Held per visible row while a model is alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidebarRow<'a> {
    pub kind: RowKind,
    pub tab: TabId,
    pub text: &'a str,
    /// One-based page number, shown on hover.
    pub page: Option<u32>,
    /// Nesting within the contents.
    pub depth: u8,
    /// The active PDF, or the section being read.
    pub current: bool,
    /// Something's wrong with this tab's file, in words of Rust's own. On a note, what it means.
    pub notice: Option<&'static str>,
    /// What tapping a note does.
    pub action: Option<ContentsAction>,
    /// A contents row's entry in its list.
    pub entry: u32,
    /// How far down its page the entry is, in thousandths: where tapping it goes. 0 for a row that isn't one, or an entry with no place.
    pub top: u16,
    /// It has sections under it, so it has a chevron to fold them.
    pub expandable: bool,
    /// Those sections show.
    pub expanded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Tab,
    Contents,
    /// A line of Rust's over a tab's contents: generated contents on offer, being made, or shown.
    Note,
}

/// Everything the host draws in the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarModel<'a> {
    pub rows: Vec<SidebarRow<'a>>,
    /// How many rows the rail fills: contents rows up to and including the section being read.
    pub rail_filled: usize,
    /// Whether the active PDF's contents are showing, which decides the chevron.
    pub toc_open: bool,
    /// The place, such as "32 of 96", which the page number in the lower right shows. Empty with no PDF open.
    pub place: String,
    /// Whether the active PDF has unsaved changes, which shows its dot and the Save buttons.
    pub edited: bool,
    /// The owner's edits on the active PDF have been undone, by anyone, and can still be redone. The page matches the file, so `edited` is false, but closing would lose them, so the app has this to show.
    pub undone_owner_steps: bool,
}

/// A tab's row in the sidebar, in points.
pub const TAB_ROW_HEIGHT: f64 = 32.0;
/// A contents row in the sidebar, in points.
pub const CONTENTS_ROW_HEIGHT: f64 = 28.0;
/// The note over the contents, in points: a line of small text.
pub const NOTE_ROW_HEIGHT: f64 = 24.0;
/// The most of the sidebar's room the contents take: about 60%, by the owner's yes of 2026-11-07. The rest is for the other tabs, and it's what keeps them from being pushed off by one long outline.
pub const CONTENTS_MAX_SHARE: f64 = 0.6;

/// **The scroll thumb**, for the sidebar's lists and the page area alike, so they match. Owner, 2026-11-07: "The sidebar thumb should always be visible, and lets hide the rail", then "the thumb should be thinner", and for both scrollers "the scroll thumb always visible, the rail always hidden", whatever the system's setting. It shows only when there's something to scroll, and draws over the content's right margin, taking no width from it.
///
/// **Why 4 points:** the Mac's own overlay thumb is about 7 points at rest and 11 under the pointer. The owner asked for thinner, and 4 is the thinnest that still reads as a thumb on a 1x screen (2 device pixels either side of its centre line), with rounded ends. Under the pointer it widens to 6, as the system's does, and it's grabbed anywhere in a strip [`ScrollThumb::hit_width`] wide, so a slim look doesn't make it hard to drag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollThumb {
    /// Its width at rest, and under the pointer or while dragged, in points.
    pub width: f64,
    pub hover_width: f64,
    /// The strip along the edge that takes the pointer, in points: wider than the thumb looks.
    pub hit_width: f64,
    /// From the thumb's right side to the edge, in points.
    pub inset: f64,
    /// The shortest it gets, in points.
    pub min_length: f64,
    /// Its colour as `0xRRGGBB` and an opacity, on light and dark, at rest and under the pointer. Black or white over whatever's behind, as the system's thumb is, so it sits on the sidebar's colour and the page area's alike.
    pub light: (u32, f64),
    pub light_hover: (u32, f64),
    pub dark: (u32, f64),
    pub dark_hover: (u32, f64),
    /// The same, with the system's Increase Contrast on: about twice as strong, so the thumb stands out against the sidebar and the page as that setting asks of every control, and still clear of the text under it.
    pub light_contrast: (u32, f64),
    pub light_contrast_hover: (u32, f64),
    pub dark_contrast: (u32, f64),
    pub dark_contrast_hover: (u32, f64),
}

/// The one scroll thumb.
pub const SCROLL_THUMB: ScrollThumb = ScrollThumb {
    width: 4.0,
    hover_width: 6.0,
    hit_width: 12.0,
    inset: 3.0,
    min_length: 24.0,
    light: (0x000000, 0.22),
    light_hover: (0x000000, 0.42),
    dark: (0xFFFFFF, 0.24),
    dark_hover: (0xFFFFFF, 0.45),
    light_contrast: (0x000000, 0.55),
    light_contrast_hover: (0x000000, 0.75),
    dark_contrast: (0xFFFFFF, 0.6),
    dark_contrast_hover: (0xFFFFFF, 0.8),
};

/// How much of the sidebar's list goes where, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidebarHeights {
    /// The height of the contents' own scrolling area. Less than their rows' total means they scroll.
    pub contents: f64,
    /// The note's height over them, or nothing.
    pub note: f64,
    /// Whether the list of tabs doesn't fit, and scrolls by itself.
    pub tabs_scroll: bool,
}

/// **The sidebar's height rule.** Owner, 2026-11-07: "The max height of toc does not seem to be working", then yes to a true maximum: the contents take what they need, up to [`CONTENTS_MAX_SHARE`] of the room, and scroll inside that. The other tabs have what's left, and a long list of them scrolls by itself in it. It takes the room it's told of, so anything else docked in the sidebar comes off first.
pub fn sidebar_heights(room: f64, tabs: u32, contents_rows: u32) -> SidebarHeights {
    sidebar_heights_with_note(room, tabs, contents_rows, false)
}

/// The same, with a note over the contents, such as "Generated contents". It stays put above them while they scroll, so a generated list is labelled wherever it's scrolled to, and it comes out of the same share.
pub fn sidebar_heights_with_note(room: f64, tabs: u32, contents_rows: u32, note: bool) -> SidebarHeights {
    let room = if room.is_finite() { room.max(0.0) } else { 0.0 };
    let tab_row = if tabs > 0 { TAB_ROW_HEIGHT } else { 0.0 };
    let note = if note && room - tab_row >= NOTE_ROW_HEIGHT { NOTE_ROW_HEIGHT } else { 0.0 };
    let natural = f64::from(contents_rows) * CONTENTS_ROW_HEIGHT;
    // With a window too short for the rest, the active tab's own row comes first.
    let contents = natural.min(room * CONTENTS_MAX_SHARE - note).min(room - tab_row - note).max(0.0);
    SidebarHeights {
        contents,
        note,
        tabs_scroll: f64::from(tabs) * TAB_ROW_HEIGHT + note + contents > room,
    }
}

/// What hovering over a row shows: its full text, cleaned and bounded however long or strange it is. A title comes from the file, so it may be 100,000 characters of newlines, and this is the one place the host gets it from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowFullText {
    pub text: String,
    /// It's the file's name, as opposed to a title from inside it.
    pub is_file_name: bool,
}

/// The most of a row's full text that's shown, in characters. Longer is no longer reading a name.
pub const MAX_ROW_FULL_TEXT: usize = 400;

pub fn row_full_text(text: &str, is_file_name: bool) -> RowFullText {
    let clean = recto_core::clean_line(text);
    let mut chars = clean.char_indices().skip(MAX_ROW_FULL_TEXT);
    let text = match chars.next() {
        Some((at, _)) => format!("{}…", clean[..at].trim_end()),
        None => clean,
    };
    RowFullText { text, is_file_name }
}

/// How pages are shown, which is the same for every tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewSettings {
    /// `None` goes with the window: white with a light one, dark with a dark one.
    pub paper: Option<Paper>,
    /// The sidebar's hidden. Owner, 2026-11-08: "simply have a sidebar icon to the right of the traffic light buttons that toggles sidebar on and of. No need mor a separate mode".
    pub sidebar_hidden: bool,
    /// Owner, 2026-11-07: "Add an option in the macos top bar to turn off agent bar (hides the icon)".
    pub agent_icon_hidden: bool,
}

/// A tab's name from its path. It comes from outside: a downloaded file can carry anything in its name, and this reaches the sidebar and agents.
fn tab_name(path: &Path) -> String {
    let file_name = path.file_name().unwrap_or_default();
    let stem = Path::new(file_name).file_stem().unwrap_or(file_name).to_string_lossy();
    match recto_core::clean_line(&stem) {
        name if name.is_empty() => "Untitled".to_string(),
        name => name,
    }
}

/// A closed PDF's history, read for its being opened again. A seam, as this crate has no disk of its own.
pub trait HistorySource: Send + Sync {
    /// What's left of a PDF's history, if any. On whatever thread reads the file, with the session free.
    fn history_of(&self, stamp: Option<FileStamp>, path: &Path) -> Option<TakenHistory>;
    /// A journal of an open tab, read back as a list. Touches nothing. With the session held, so it's a bounded read of a private file.
    fn read_list(&self, name: &str) -> Option<RestoredEdits>;
    /// A tab's sleep file, for this version of its file. Asks nothing of the PDF. With the session held, so it's a bounded read of a private file.
    fn read_sleep(&self, _stamp: FileStamp, _path: &Path) -> Option<sleep::SleepFile> {
        None
    }
}

/// A PDF's history, taken up by a tab.
#[derive(Debug)]
pub struct TakenHistory {
    pub name: String,
    pub restored: RestoredEdits,
}

/// What's waiting to go in journals.
#[derive(Debug, Default)]
pub struct JournalWork {
    pub retired: Vec<String>,
    pub appends: Vec<JournalAppend>,
}

/// How that went. A journal's failing doesn't stop the others, and says nothing about them.
#[derive(Debug, Default)]
pub struct JournalOutcome {
    /// Operations in the file now: which tab, up to how many, and the name it went under, which may not be the one asked for if something's there that isn't to be touched.
    pub written: Vec<(TabId, u64, String)>,
    /// Retired journals that couldn't go, to try again.
    pub not_retired: Vec<String>,
}

/// Operations for one journal, up to this many in all.
#[derive(Clone, Debug)]
pub struct JournalAppend {
    pub tab: TabId,
    pub name: String,
    /// What a new file starts with.
    pub header: JournalHeader,
    pub through: u64,
    pub ops: Vec<JournalOp>,
    /// Its tab has closed, with history and nothing unsaved. Once these are written, it waits for the PDF's next opening, and no launch reads it.
    pub closing: bool,
    /// The whole list, at `through`, for a journal that starts again from it: in one step, in place of anything at this name. A save does this, and clearing a history. `ops` is empty then.
    pub base: Option<record::EditsRecord>,
    /// The journal this one takes over from, which goes once this has got there and not before.
    pub replaces: Option<String>,
}

/// A list brought back from a journal.
#[derive(Debug)]
pub struct RestoredEdits {
    pub edits: PageEdits,
    /// How many operations there have been in all, so what comes next is counted on from there.
    pub seq: u64,
    /// Part of the journal couldn't be read, more than a tail torn by a crash.
    pub lost_lines: bool,
    /// Steps had to go, as a file kept for them isn't what it was.
    pub rolled_off: bool,
}

/// Why a history's being cleared. SPEC decision 25. A confirmed redaction or hidden-data removal, or what's been removed could be got back from it. A file changed outside the app, as its steps no longer apply. A discard, which is all of a tab's edits. And the user's asking for it, with Clear Edit History.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearReason {
    Redaction,
    HiddenDataRemoval,
    FileChanged,
    Discard,
    UserAsked,
}

/// What a tab's sleep file would say, from here.
#[derive(Clone, Debug)]
pub struct SleepFacts {
    pub id: TabId,
    pub path: PathBuf,
    pub sleep: sleep::SleepFile,
    /// No page of the document's from a protected file.
    pub pictures_allowed: bool,
}

/// What's kept of a tab's edits while it's left alone, for a look without opening it.
pub fn edits_summary_of(edits: &PageEdits) -> EditsSummary {
    edits.summary()
}

/// All open tabs and which one is active.
pub struct Session<L = DiskLoader> {
    loader: L,
    /// The view settings, Find and edit mode.
    viewing: viewing::Viewing,
    /// Every open tab, which one's active, the next one's number, and the folders their files are in.
    tabs: registry::Tabs,
    /// The tabs that keep their contents, and the reads of cold tabs' files made ahead of waking them.
    warm: warm::Warm,
    /// Every tab's list of edits, in memory or gone to disk, those that couldn't be read back, and whether an agent's at Full.
    lists: lists::Lists,
    /// The session's clock, holds on tabs, and what's on its way to and from the sleep files.
    dormancy: dormancy::Dormancy,
    /// The pages lately drawn, the jumps the page view hasn't taken, and a place lost to an edit.
    mail: mail::PageViewMail,
    /// Each edited tab's journal, what's waiting to go out to the journals, and where a PDF's history is read from.
    journals: journals::Journals,
    /// Tabs being saved, and what saves lately moved their tabs on from.
    saves: saves::Saves,
    /// Tabs side by side, if there's a comparison: see [`compare`]. The active tab is one of them, the pane being read.
    comparison: Option<compare::Comparison>,
    /// Generated contents the sidebar knows of, for tabs someone asked about.
    generated: generated::Generated,
    /// Files opened lately, for File > Open Recent and ⌘K.
    recent: recent::Recent,
}

/// A file opened lately, for File > Open Recent and ⌘K.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentFile {
    pub path: PathBuf,
    /// Its name as a tab shows it: the file's name without `.pdf`.
    pub name: String,
    /// The tab it's open in now, if it is.
    pub open: Option<TabId>,
}

/// Saves remembered for the page view to carry its view over: one per tab saved lately is plenty, as it's asked for straight after.
pub const MAX_SAVED_OVER: usize = 8;

/// Generated contents held for the sidebar. A tab whose contents went out can always look again: they're kept on disk.
pub const MAX_GENERATED_VIEWS: usize = 8;

/// Where a tab's generated contents stand, as the sidebar shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeneratedView {
    /// Asked for, and being made.
    Making,
    /// They couldn't be, for this reason, in Rust's words.
    Failed(&'static str),
    Ready(Arc<Toc>),
    /// Made, and there were no headings to list: a scan with no text, or a document set all in one size.
    NoneFound,
    /// Looked for, for a tab that had chosen them, and they weren't kept. They're offered again.
    NotKept,
}

/// What a tap on the contents' note does. The note is Rust's: its words and what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentsAction {
    Generate,
    ShowGenerated,
    ShowOwn,
    /// Stop making them.
    Stop,
}

impl ContentsAction {
    pub fn id(self) -> &'static str {
        match self {
            Self::Generate => "contents.generate",
            Self::ShowGenerated => "contents.show_generated",
            Self::ShowOwn => "contents.show_own",
            Self::Stop => "contents.stop",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [Self::Generate, Self::ShowGenerated, Self::ShowOwn, Self::Stop]
            .into_iter()
            .find(|action| action.id() == id)
    }
}

/// The note over a tab's contents: a short line at rest, and what it means on hover. `None` for contents that are fine and shown as they are. Owner, 2026-11-06: "the left sidebar menu is a bit too complicated (visually noisy)", so it's one quiet line, and the rest waits for the pointer.
pub fn contents_note(
    verdict: ContentsVerdict,
    view: Option<&GeneratedView>,
    choose_generated: bool,
    own_entries: usize,
) -> Option<(&'static str, &'static str, Option<ContentsAction>)> {
    const MADE: &str =
        "Made by RectoPDF from the headings distillPDF found on the pages. They aren't part of the file, which isn't changed, and they can be wrong.";
    match view {
        Some(GeneratedView::Ready(_)) if choose_generated => Some((
            "Generated contents",
            if own_entries > 0 {
                "Made by RectoPDF from the headings distillPDF found on the pages. They aren't part of the file, which isn't changed, and they can be wrong. Click to show the file's own."
            } else {
                MADE
            },
            (own_entries > 0).then_some(ContentsAction::ShowOwn),
        )),
        Some(GeneratedView::Ready(_)) => Some(("Show generated contents", MADE, Some(ContentsAction::ShowGenerated))),
        Some(GeneratedView::Making) => Some((
            "Generating contents\u{2026}",
            "Finding the headings on the pages, in the background. On a long document this can take a little while. Click to stop.",
            Some(ContentsAction::Stop),
        )),
        Some(GeneratedView::Failed(words)) => Some(("Couldn't generate contents", words, Some(ContentsAction::Generate))),
        Some(GeneratedView::NoneFound) => Some((
            "No headings found",
            "RectoPDF looked for headings on the pages and found none to list. A scan with no text has none to find.",
            None,
        )),
        _ if verdict.offers() || choose_generated => Some((
            "Generate contents",
            match verdict {
                ContentsVerdict::Missing => {
                    "This file has no contents of its own. RectoPDF can make some from the headings on its pages. The file isn't changed."
                }
                _ => "The file's own contents look incomplete. RectoPDF can make some from the headings on its pages, and the file's own stay as they are.",
            },
            Some(ContentsAction::Generate),
        )),
        _ => None,
    }
}

/// What a save writes from: the document as edited, and the file it's about, copied out so the writing needn't hold the session.
#[derive(Clone, Debug)]
pub struct SaveSnapshot {
    pub path: PathBuf,
    pub stamp: Option<FileStamp>,
    pub file_pages: u32,
    pub slots: Vec<edit::PageSlot>,
    pub added: Vec<edit::AddedSource>,
    /// Some unsaved step, done or undone, is the owner's.
    pub holds_owner_edits: bool,
    /// The history needs pages of the file as it is now, which a save may take out of it.
    pub needs_own_pages: bool,
}

/// Files read ahead of waking their tabs, held at once. More than wakes can be under way together.
const MAX_READ_AHEAD: usize = 4;

/// Page results remembered across every tab, so nothing here grows with idle tabs.
pub const MAX_DRAWS_KEPT: usize = 256;

/// That a page was drawn, or couldn't be.
// size: asserted <= 32 by `draw_note_stays_small`. Held per recent draw, at most MAX_DRAWS_KEPT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawNote {
    pub tab: TabId,
    /// Zero-based page.
    pub page: u32,
    /// The failure's stable code, or `None` if it was drawn.
    pub failure: Option<&'static str>,
}

/// The session lives behind the app's one mutex and is handed between its threads: nothing in it may hold a lock, a cell or anything else that isn't `Send`. Failure caught: a component of `Session` (R3.2) brings state of its own that can't cross threads, which would only show in another crate's build.
const _: () = {
    const fn sendable<T: Send>() {}
    sendable::<Session<DiskLoader>>();
};

impl Default for Session<DiskLoader> {
    fn default() -> Self {
        Self::new(DiskLoader)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionError {
    Open(OpenError),
    NoSuchTab,
    TooManyTabs,
    UnsavedChanges,
    Edit(EditError),
    /// Only the renderer can read this file, so its pages can't be changed.
    RendererOnly,
    /// Its edits can't be read back just now, so nothing's laid on them.
    EditsUnreadable,
    /// It's being saved, and nothing changes its edits until that's done.
    Saving,
    /// A quiet run's file outside its own folders: see [`confined`].
    OutsideQuietRun,
}

impl SessionError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Open(error) => error.code(),
            Self::NoSuchTab => "no_such_tab",
            Self::TooManyTabs => "too_many_tabs",
            Self::UnsavedChanges => "unsaved_changes",
            Self::Edit(error) => error.code(),
            Self::RendererOnly => "renderer_only",
            Self::EditsUnreadable => "edits_unreadable",
            Self::Saving => "saving",
            Self::OutsideQuietRun => "outside_quiet_run",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Open(error) => error.message(),
            Self::NoSuchTab => "That tab isn't open.",
            Self::TooManyTabs => "Too many PDFs are open. Close some and try again.",
            Self::EditsUnreadable => {
                "This tab's edits can't be read back just now, so nothing more was done to them. They're kept, and it tries again each time. Discarding them is what gives up on them."
            }
            Self::UnsavedChanges => "This PDF has unsaved changes.",
            Self::Edit(error) => error.message(),
            Self::RendererOnly => "RectoPDF can draw this file but can't read its structure, so its pages can't be changed.",
            Self::Saving => "This PDF is being saved. Nothing changes until that's done, which is a moment.",
            Self::OutsideQuietRun => confined::OUTSIDE_WORDS,
        }
    }
}

#[cfg(test)]
mod tests;
