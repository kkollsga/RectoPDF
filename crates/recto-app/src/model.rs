//! **The window's view models, projected from the session and a few facts of the app.** Nothing in here takes a lock, calls a page view, schedules work or tells a listener: it reads
//! a session (or what was read from one) and returns what the window shows, so the rules that decide it are tested without the app (review T2, restructuring step R1.1).
//!
//! `reconcile` takes the inputs, calls these, then does the side effects. [`read`] takes everything the session is asked for in one pass, which the caller does under one lock,
//! so the view models of a pass are of one moment in the session and not of six.
//!
//! The plain records it returns are owned here; `recto-ffi` keeps the UniFFI records the host sees (their shape is the Swift contract) and converts one for one, with no logic.
//! The reader model is composed of parts by concern ([`PaperModel`], [`EditModel`], [`SaveModel`], [`DocumentModel`], [`WindowModel`]), each small.

use recto_platform::FileStamp;
use recto_reader::PaperKind;
use recto_reader::tiles::{PageMap, SavedPlace};
use recto_session::compare::Comparison;
use recto_session::record::{self, notices};
use recto_session::{DocLoader, Paper, RowKind as SessionRowKind, Session, SidebarModel, TabId, WARM_PAPER};
use std::path::PathBuf;
use std::sync::Arc;

/// What a row of the sidebar is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Tab,
    Contents,
    /// A quiet line over the contents, about generated contents.
    Note,
}

/// A row of the sidebar. At rest it's a name; its page number shows on hover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarRow {
    pub kind: RowKind,
    pub tab: u32,
    /// From the file, so it's only ever drawn as text.
    pub text: String,
    pub page: Option<u32>,
    pub depth: u8,
    pub current: bool,
    pub rail_filled: bool,
    pub rail_first: bool,
    pub rail_last: bool,
    /// Something's wrong with this tab's file, in Rust's words. On a note, what it means.
    pub notice: Option<String>,
    /// What tapping a note does, by Rust's own name for it.
    pub action: Option<String>,
    pub expandable: bool,
    pub expanded: bool,
    /// A tab side by side with others.
    pub in_comparison: bool,
}

/// Everything the sidebar draws but for the contents' rows, which the host pulls as they come on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarView {
    pub tabs_version: u64,
    pub tab_count: u32,
    pub active_index: Option<u32>,
    pub contents_count: u32,
    pub contents_version: u64,
    pub contents_changed_from: u32,
    pub contents_removed: u32,
    pub contents_inserted: u32,
    pub contents_place_shift: i32,
    pub contents_current: Option<u32>,
    pub toc_open: bool,
    pub edited: bool,
    pub undone_owner_steps: bool,
    pub contents_note: Option<SidebarRow>,
}

/// A comparison's pane, as its label says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneView {
    pub tab: u32,
    /// Its file's name, cleaned.
    pub title: String,
    /// It's the pane being read.
    pub reading: bool,
    /// Its tab's layout, by name.
    pub layout: String,
    /// What's wrong with its file, in Rust's words.
    pub note: Option<String>,
}

/// What to say by the page about a copy, a print or an action, and the button if there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionNotice {
    pub text: String,
    pub action_title: Option<String>,
    pub action_id: Option<String>,
    pub fades: bool,
}

/// A file opened lately, for File > Open Recent and for ⌘K to offer. The host's record is this one, declared to UniFFI by `recto-ffi` as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentFile {
    /// Which file this is, however the list moves: what its Open Recent item's id carries, as `recent.<key>`.
    pub key: String,
    /// As a tab shows it: the file's name without `.pdf`, cleaned.
    pub name: String,
    /// The folder it's in, as a path, for telling two of the same name apart.
    pub folder: String,
    pub path: String,
    /// It's open in a tab now, so choosing it goes there.
    pub open: bool,
}

/// What the window's Save is, decided by the caller from the signature rule: whether it's off, what Save As's sheet says, and the icon's tooltip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveFacts {
    pub off: bool,
    pub note: Option<String>,
    pub tooltip: String,
}

/// **The key a file in Open Recent goes by:** from its path, so it's the same file however the list has moved since the menu was made, and an item can't open another file than the one it showed. Sixteen hex digits, never a path, so nothing of where someone keeps their files is in a menu item's id.
pub fn recent_key(path: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// A file's stamp as it says which version of the file this is: not the disk it's on, whose number can change while the file doesn't.
pub fn version_of(stamp: FileStamp) -> FileStamp {
    FileStamp { device: 0, ..stamp }
}

/// The paper as the window is told it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaperModel {
    /// The paper's name, which QA and the menu use.
    pub name: String,
    pub rgb: u32,
    /// A colour to multiply over each page in the compositor, for warm paper.
    pub tint: Option<u32>,
    pub highlight_dark_colour: bool,
    pub highlight_lighten: bool,
}

/// What can be done to the tab being read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditModel {
    pub edited: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub edit_mode: bool,
    pub can_rotate: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

/// The Save icon and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveModel {
    pub saving: bool,
    pub tooltip: String,
    pub as_note: Option<String>,
    /// The icon is beside the window's controls: with unsaved edits, or while saving.
    pub button: bool,
    /// It does something: not while saving, and not when Save over the file would break a signature.
    pub enabled: bool,
}

/// The tab showing and where in it the reader is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentModel {
    /// The tab an action from the window is about. 0 with none.
    pub shown_tab: u32,
    pub name: String,
    pub folder: String,
    /// The page number in the lower right, such as "32 of 96".
    pub place: String,
    /// The page being read, by its label if it has one.
    pub page_text: String,
    /// The tab showing's layout, by name. Empty with none.
    pub layout: String,
}

/// The window around the page.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowModel {
    pub home: bool,
    pub sidebar_hidden: bool,
    pub agent_icon_hidden: bool,
    pub sidebar_width: f64,
    pub compare_synced: bool,
    pub print_ready: bool,
}

/// What the page area shows, by concern. The sheets that ask something (a link, a password, a failed save) are the bridge's and aren't in it.
#[derive(Clone, Debug, PartialEq)]
pub struct ReaderModel {
    /// Tabs side by side, a pane each. Empty with one tab showing.
    pub panes: Vec<PaneView>,
    /// Something the reader needs to be told, in words.
    pub note: Option<String>,
    pub selection_notice: Option<SelectionNotice>,
    pub window: WindowModel,
    pub paper: PaperModel,
    pub edit: EditModel,
    pub save: SaveModel,
    pub document: DocumentModel,
}

/// A contents row as it's kept, with no more in it than it needs. Rows are fetched by the host for what's on screen, so a long outline crosses only as far as it's shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentsEntry {
    pub tab: u32,
    pub text: Box<str>,
    pub page: Option<u32>,
    pub depth: u8,
    /// Its entry in the list, and whether it folds and is open.
    pub entry: u32,
    pub expandable: bool,
    pub expanded: bool,
}

/// What the last pass published, which this pass keeps when nothing in it has changed: a page crossed mustn't mean thousands of strings copied, for a long outline or for many tabs.
#[derive(Clone)]
pub struct Held {
    pub tabs: Arc<Vec<SidebarRow>>,
    pub contents: Arc<Vec<ContentsEntry>>,
    pub sidebar: Arc<SidebarView>,
    /// What the Open Recent list was last built from.
    pub recent_key: Option<(u64, u64)>,
}

/// A sidebar with nothing in it: what the window starts with.
pub fn empty_sidebar() -> SidebarView {
    SidebarView {
        tabs_version: 0,
        tab_count: 0,
        active_index: None,
        contents_version: 0,
        contents_count: 0,
        contents_changed_from: 0,
        contents_removed: 0,
        contents_inserted: 0,
        contents_place_shift: 0,
        contents_current: None,
        toc_open: false,
        edited: false,
        undone_owner_steps: false,
        contents_note: None,
    }
}

/// The sidebar as one pass projects it. `tabs` and `contents` are the held ones, the same `Arc`s, when they haven't changed.
pub struct SidebarParts {
    pub view: SidebarView,
    pub tabs: Arc<Vec<SidebarRow>>,
    pub contents: Arc<Vec<ContentsEntry>>,
    pub rail_filled: u32,
}

/// How many tab rows and contents rows the model has, and where the current one of each is. A note belongs to neither.
struct RowCounts {
    tabs: u32,
    contents: u32,
    active_index: Option<u32>,
    current: Option<u32>,
}

fn count_rows(model: &SidebarModel<'_>) -> RowCounts {
    let mut counts = RowCounts {
        tabs: 0,
        contents: 0,
        active_index: None,
        current: None,
    };
    for row in &model.rows {
        match row.kind {
            SessionRowKind::Note => {}
            SessionRowKind::Contents => {
                if row.current {
                    counts.current = Some(counts.contents);
                }
                counts.contents += 1;
            }
            SessionRowKind::Tab => {
                if row.current {
                    counts.active_index = Some(counts.tabs);
                }
                counts.tabs += 1;
            }
        }
    }
    counts
}

/// The tab rows, and the tabs' version: the held rows and version if nothing a tab row shows has changed, or else new rows and the next version.
fn tab_rows(model: &SidebarModel<'_>, compared: &impl Fn(TabId) -> bool, count: u32, held: &Held) -> (Arc<Vec<SidebarRow>>, u64) {
    let rows = || model.rows.iter().filter(|row| row.kind == SessionRowKind::Tab);
    let same = held.tabs.len() == count as usize
        && rows().zip(held.tabs.iter()).all(|(row, tab)| {
            row.tab.0 == tab.tab
                && row.current == tab.current
                && row.text == tab.text.as_str()
                && row.notice == tab.notice.as_deref()
                && compared(row.tab) == tab.in_comparison
        });
    if same {
        return (Arc::clone(&held.tabs), held.sidebar.tabs_version);
    }
    let list = rows()
        .map(|row| SidebarRow {
            kind: RowKind::Tab,
            tab: row.tab.0,
            text: row.text.to_string(),
            page: row.page,
            depth: row.depth,
            current: row.current,
            rail_filled: false,
            rail_first: false,
            rail_last: false,
            notice: row.notice.map(str::to_string),
            action: None,
            expandable: false,
            expanded: false,
            in_comparison: compared(row.tab),
        })
        .collect();
    (Arc::new(list), held.sidebar.tabs_version + 1)
}

/// The contents rows with how the list differs from the held one: its version, which rows went and came, and how far the place was carried.
struct ContentsChange {
    list: Arc<Vec<ContentsEntry>>,
    version: u64,
    change: (u32, u32, u32),
    place_shift: i32,
}

fn contents_change(model: &SidebarModel<'_>, counts: &RowCounts, held: &Held) -> ContentsChange {
    let rows = || model.rows.iter().filter(|row| row.kind == SessionRowKind::Contents);
    let same = held.contents.len() == counts.contents as usize
        && rows().zip(held.contents.iter()).all(|(row, entry)| {
            row.tab.0 == entry.tab
                && row.page == entry.page
                && row.depth == entry.depth
                && row.text == entry.text.as_ref()
                && row.entry == entry.entry
                && row.expanded == entry.expanded
                && row.expandable == entry.expandable
        });
    let was = &held.sidebar;
    if same {
        return ContentsChange {
            list: Arc::clone(&held.contents),
            version: was.contents_version,
            change: (was.contents_changed_from, was.contents_removed, was.contents_inserted),
            place_shift: was.contents_place_shift,
        };
    }
    let list: Arc<Vec<ContentsEntry>> = Arc::new(
        rows()
            .map(|row| ContentsEntry {
                tab: row.tab.0,
                text: row.text.into(),
                page: row.page,
                depth: row.depth,
                entry: row.entry,
                expandable: row.expandable,
                expanded: row.expanded,
            })
            .collect(),
    );
    let change = rows_changed(&held.contents, &list);
    let place_shift = place_shift(&held.contents, &list, change, was.contents_current, counts.current);
    ContentsChange {
        list,
        version: was.contents_version + 1,
        change,
        place_shift,
    }
}

/// The note over the contents stays put while they scroll, so it's drawn apart from them.
fn contents_note(model: &SidebarModel<'_>) -> Option<SidebarRow> {
    model.rows.iter().find(|row| row.kind == SessionRowKind::Note).map(|row| SidebarRow {
        kind: RowKind::Note,
        tab: row.tab.0,
        text: row.text.to_string(),
        page: None,
        depth: 0,
        current: false,
        rail_filled: false,
        rail_first: false,
        rail_last: false,
        notice: row.notice.map(str::to_string),
        action: row.action.map(|action| action.id().to_string()),
        expandable: false,
        expanded: false,
        in_comparison: false,
    })
}

/// **The sidebar,** from the session's model and what was published last: rows are copied only when they've changed.
pub fn sidebar_parts(model: &SidebarModel<'_>, compared: impl Fn(TabId) -> bool, held: &Held) -> SidebarParts {
    let counts = count_rows(model);
    let (tabs, tabs_version) = tab_rows(model, &compared, counts.tabs, held);
    let contents = contents_change(model, &counts, held);
    let view = SidebarView {
        contents_note: contents_note(model),
        tabs_version,
        tab_count: counts.tabs,
        active_index: counts.active_index,
        contents_count: counts.contents,
        contents_version: contents.version,
        contents_changed_from: contents.change.0,
        contents_removed: contents.change.1,
        contents_inserted: contents.change.2,
        contents_place_shift: contents.place_shift,
        contents_current: counts.current,
        toc_open: model.toc_open,
        edited: model.edited,
        undone_owner_steps: model.undone_owner_steps,
    };
    SidebarParts {
        view,
        tabs,
        contents: contents.list,
        rail_filled: u32::try_from(model.rail_filled).unwrap_or(u32::MAX),
    }
}

/// What a read of the tab being read says, for the window's buttons.
pub struct ReadingFacts {
    pub edited: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub edit_mode: bool,
    pub can_rotate: bool,
    pub saving: bool,
    /// The tab, its file and the file's stamp, if there is a tab being read and its file is known.
    pub file: Option<(TabId, PathBuf, recto_platform::FileStamp)>,
    pub needs_rewrite: bool,
    /// Opened with its password: nothing saves into a protected file yet, so Save and Save As are off.
    pub protected: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// The page by its label, or its number: what the go-to field starts with.
    pub page_text: String,
}

impl ReadingFacts {
    /// The tab an action from the window is about, 0 with none.
    pub fn shown_tab(&self) -> u32 {
        self.file.as_ref().map_or(0, |(id, _, _)| id.0)
    }

    /// The file's name and its folder, for Save As's sheet and the close prompt.
    fn document(&self) -> (String, String) {
        self.file.as_ref().map_or((String::new(), String::new()), |(_, path, _)| {
            (
                path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
                path.parent().map(|folder| folder.display().to_string()).unwrap_or_default(),
            )
        })
    }

    /// Nothing is being read: every button is off.
    fn none(edit_mode: bool) -> Self {
        Self {
            edited: false,
            can_undo: false,
            can_redo: false,
            edit_mode,
            can_rotate: false,
            saving: false,
            file: None,
            needs_rewrite: false,
            protected: false,
            can_go_back: false,
            can_go_forward: false,
            page_text: String::new(),
        }
    }
}

/// Everything one pass reads of the session.
pub struct Facts {
    /// What each pane shows, left to right. Their jumps are filled in by the caller, which takes them from the session.
    pub panes: Vec<PaneSnapshot>,
    pub comparison: Option<Comparison>,
    pub pane_pages: Vec<u32>,
    pub compare_note: Option<String>,
    pub active: Option<TabId>,
    pub sidebar: SidebarParts,
    /// The page number in the lower right, such as "32 of 96".
    pub place: String,
    pub chosen_paper: Option<Paper>,
    pub sidebar_hidden: bool,
    pub agent_icon_hidden: bool,
    pub reading: ReadingFacts,
    pub compare_synced: bool,
    /// What Open Recent was built from, and the list if that has changed since the last pass.
    pub recent_key: (u64, u64),
    pub recent: Option<Vec<RecentFile>>,
}

impl Facts {
    /// The pane being read: the active tab's, or else the first.
    pub fn reading_pane(&self) -> Option<&PaneSnapshot> {
        self.panes.iter().find(|shown| Some(shown.id) == self.active).or(self.panes.first())
    }
}

fn reading_facts<L: DocLoader>(session: &Session<L>, shown: Option<&PaneSnapshot>) -> ReadingFacts {
    let edit_mode = session.viewing().edit_mode();
    let Some(shown) = shown else { return ReadingFacts::none(edit_mode) };
    let page = session.tabs().tab(shown.id).map_or(0, |tab| tab.page);
    let (can_undo, can_redo) = session.lists().undo_and_redo(shown.id);
    let (can_go_back, can_go_forward) = session.tabs().can_go_back_and_forward(shown.id);
    ReadingFacts {
        edited: session.lists().is_edited(shown.id),
        can_undo,
        can_redo,
        edit_mode,
        can_rotate: session.can_edit_pages(shown.id),
        saving: session.saves().is_saving(shown.id),
        file: session
            .tabs()
            .path_of(shown.id)
            .zip(session.tabs().own_stamp(shown.id))
            .map(|(path, stamp)| (shown.id, path, stamp)),
        needs_rewrite: session.lists().needs_rewrite(shown.id),
        protected: session.tabs().withheld_from_agents(shown.id),
        can_go_back,
        can_go_forward,
        page_text: session.pages().label(shown.id, page).unwrap_or_else(|| (page + 1).to_string()),
    }
}

/// Open Recent's rows, in the words the window draws them in.
fn recent_files<L: DocLoader>(session: &Session<L>) -> Vec<RecentFile> {
    session
        .recent_files()
        .into_iter()
        .map(|file| RecentFile {
            key: recent_key(&file.path),
            name: recto_core::clean_line(&file.name),
            folder: file.path.parent().map(|folder| folder.display().to_string()).unwrap_or_default(),
            path: file.path.to_string_lossy().into_owned(),
            open: file.open.is_some(),
        })
        .collect()
}

/// **Reads everything a pass needs of the session,** in one go. Called under the session's one lock, so what it returns is of one moment. Takes nothing out of the session: the caller takes each pane's jump.
pub fn read<L: DocLoader>(session: &Session<L>, held: &Held) -> Facts {
    let active = session.tabs().active();
    let panes = session.panes();
    let snapshots: Vec<PaneSnapshot> = panes
        .iter()
        .enumerate()
        .filter_map(|(pane, id)| pane_snapshot(session, *id, pane == 0, &panes))
        .collect();
    let comparison = session.comparison().cloned();
    let compare_synced = comparison.as_ref().is_some_and(|comparison| comparison.synced);
    let pane_pages = panes.iter().map(|id| session.pages().count(*id)).collect();
    let model = session.sidebar();
    let compared = |id: TabId| session.comparison().is_some_and(|comparison| comparison.contains(id));
    let sidebar = sidebar_parts(&model, compared, held);
    let recent_key = (session.recent().version(), sidebar.view.tabs_version);
    let recent = (held.recent_key != Some(recent_key)).then(|| recent_files(session));
    let view = session.viewing().view();
    let mut facts = Facts {
        panes: snapshots,
        comparison,
        pane_pages,
        compare_note: session.compare_note(),
        active,
        sidebar,
        place: model.place.clone(),
        chosen_paper: view.paper,
        sidebar_hidden: view.sidebar_hidden,
        agent_icon_hidden: view.agent_icon_hidden,
        reading: ReadingFacts::none(false),
        compare_synced,
        recent_key,
        recent,
    };
    facts.reading = reading_facts(session, facts.reading_pane());
    facts
}

/// The paper as it's drawn: what's shown, what the page views are told, and a tint over it if any.
pub struct PaperLook {
    pub shown: Paper,
    pub kind: PaperKind,
    pub tint: Option<u32>,
}

/// **The paper,** with nothing chosen going with the window. Warm paper is white paper with a tint over it in the compositor, so going to it or from it draws nothing and holds nothing more.
pub fn paper_look(chosen: Option<Paper>, window_is_dark: bool) -> PaperLook {
    let shown = Paper::resolved(chosen, window_is_dark);
    let (kind, tint) = match shown {
        Paper::White => (PaperKind::White, None),
        Paper::Warm => (PaperKind::White, Some(WARM_PAPER)),
        Paper::Dark => (PaperKind::Dark, None),
    };
    PaperLook { shown, kind, tint }
}

/// What can be said by the page about the document, in the order it wins. The first of these that has something to say is the one shown.
pub struct NoteSources {
    /// A file that wouldn't open.
    pub open_failure: Option<String>,
    /// The app's own state couldn't be saved.
    pub save_failed: bool,
    /// What's wrong with the tab being read.
    pub problem: Option<String>,
    /// A word from the session's start, as a damaged record.
    pub session: Option<String>,
    /// Synced scroll over documents of different lengths.
    pub compare: Option<String>,
    /// The page view's own, such as a page at a lower resolution.
    pub engine: Option<String>,
}

impl NoteSources {
    /// **The one note shown:** a file that wouldn't open, then a save of the app's state that failed, then a problem with the tab, then the session's word, then the comparison's, and last the page view's.
    pub fn note(self) -> Option<String> {
        self.open_failure
            .or_else(|| self.save_failed.then(|| notices::SAVE_FAILED.to_string()))
            .or(self.problem)
            .or(self.session)
            .or(self.compare)
            .or(self.engine)
    }
}

/// What the page view says of a copy in its selection.
pub struct CopyNotice {
    pub text: String,
    pub offers_copy_anyway: bool,
    pub fades: bool,
}

/// The lines by the page, each from its own part of the app. The first that has something to say is the one shown.
pub struct SelectionSources {
    pub copy: Option<CopyNotice>,
    pub printing: Option<SelectionNotice>,
    pub tool: Option<SelectionNotice>,
    /// A line from an action, which goes by itself.
    pub action: Option<String>,
    /// The updater's line: its words, and whether it goes by itself.
    pub update: Option<(String, bool)>,
}

impl SelectionSources {
    /// **The line by the page:** a copy's (with "Copy anyway" if it offers it), then a print's, then a tool's, then an action's, and last the updater's.
    pub fn notice(self) -> Option<SelectionNotice> {
        self.copy
            .map(|notice| SelectionNotice {
                text: notice.text,
                action_title: notice.offers_copy_anyway.then(|| "Copy anyway".to_string()),
                action_id: notice.offers_copy_anyway.then(|| "edit.copy_anyway".to_string()),
                fades: notice.fades,
            })
            .or(self.printing)
            .or(self.tool)
            .or_else(|| {
                self.action.map(|text| SelectionNotice {
                    text,
                    action_title: None,
                    action_id: None,
                    fades: true,
                })
            })
            .or_else(|| {
                self.update.map(|(text, fades)| SelectionNotice {
                    text,
                    action_title: None,
                    action_id: None,
                    fades,
                })
            })
    }
}

/// **The labels of a comparison's panes,** a pane each, left to right; none outside a comparison, where the one pane has no label. A pane's note is its tab's problem, or else its page view's, as a pane that isn't read has no other place to say so.
pub fn pane_views(snapshots: &[PaneSnapshot], active: Option<TabId>, comparing: bool, engine_notes: &[Option<String>]) -> Vec<PaneView> {
    if !comparing {
        return Vec::new();
    }
    snapshots
        .iter()
        .enumerate()
        .map(|(pane, shown)| PaneView {
            tab: shown.id.0,
            title: shown.title.clone(),
            reading: Some(shown.id) == active,
            layout: shown.spread.name().to_string(),
            note: shown.problem.clone().or_else(|| engine_notes.get(pane).cloned().flatten()),
        })
        .collect()
}

/// What the app holds beside the session that the window shows.
pub struct Surfaces {
    pub paper: PaperLook,
    pub note: Option<String>,
    pub selection_notice: Option<SelectionNotice>,
    pub pane_views: Vec<PaneView>,
    /// A tab is being brought to the window, so the home state doesn't show in between.
    pub holding: bool,
    /// The first pane has no tab in it.
    pub first_pane_empty: bool,
    pub sidebar_width: f64,
    pub save: SaveFacts,
    pub print_ready: bool,
}

/// **What the page area shows.** The home state is no tab showing and none on its way; the Save icon is there with edits to save or while saving, and does something unless saving or a signature is in the way.
pub fn reader_view(facts: &Facts, app: Surfaces) -> ReaderModel {
    let reading = &facts.reading;
    let shown_tab = reading.shown_tab();
    let (name, folder) = reading.document();
    let highlight = app.paper.kind.highlight();
    ReaderModel {
        panes: app.pane_views,
        note: app.note,
        selection_notice: app.selection_notice,
        window: WindowModel {
            home: app.first_pane_empty && !app.holding,
            sidebar_hidden: facts.sidebar_hidden,
            agent_icon_hidden: facts.agent_icon_hidden,
            sidebar_width: app.sidebar_width,
            compare_synced: facts.compare_synced,
            print_ready: app.print_ready,
        },
        paper: PaperModel {
            name: app.paper.shown.name().to_string(),
            rgb: app.paper.kind.rgb(),
            tint: app.paper.tint,
            highlight_dark_colour: highlight.dark_colour,
            highlight_lighten: highlight.lighten,
        },
        edit: EditModel {
            edited: reading.edited,
            can_undo: reading.can_undo,
            can_redo: reading.can_redo,
            edit_mode: reading.edit_mode,
            can_rotate: reading.can_rotate,
            can_go_back: reading.can_go_back,
            can_go_forward: reading.can_go_forward,
        },
        save: SaveModel {
            saving: reading.saving,
            tooltip: app.save.tooltip,
            as_note: app.save.note,
            button: shown_tab != 0 && (reading.edited || reading.saving),
            enabled: !app.save.off && !reading.saving,
        },
        document: DocumentModel {
            shown_tab,
            name,
            folder,
            place: facts.place.clone(),
            page_text: reading.page_text.clone(),
            layout: facts.reading_pane().map_or(String::new(), |shown| shown.spread.name().to_string()),
        },
    }
}

/// By how many rows a change moved the place, for the host to take back so it stays put on screen: the place's row if it was listed before, or else the row it came from if that's listed still, or else the nearest chapter of it that was.
fn place_shift(
    before: &[ContentsEntry],
    after: &[ContentsEntry],
    (from, removed, inserted): (u32, u32, u32),
    place_before: Option<u32>,
    place_after: Option<u32>,
) -> i32 {
    let (from, removed, inserted) = (from as usize, removed as usize, inserted as usize);
    let rows = |count: usize| i32::try_from(count).unwrap_or(i32::MAX);
    let Some(place) = place_after.map(|row| row as usize).filter(|&row| row < after.len()) else {
        return 0;
    };
    if place < from {
        return 0;
    }
    if place >= from + inserted {
        return rows(inserted) - rows(removed);
    }
    let (gone, came) = (&before[from..from + removed], &after[from..from + inserted]);
    let find = |list: &[ContentsEntry], wanted: &ContentsEntry| list.iter().position(|entry| entry == wanted);
    if let Some(was) = find(gone, &after[place]) {
        return rows(place - from) - rows(was);
    }
    match place_before.map(|row| row as usize) {
        Some(old) if old >= from + removed => return rows(inserted) - rows(removed),
        Some(old) if old >= from => {
            if let Some(now) = find(came, &before[old]) {
                return rows(now) - rows(old - from);
            }
        }
        _ => {}
    }
    let mut depth = after[place].depth;
    for (index, entry) in came[..place - from].iter().enumerate().rev() {
        if entry.depth < depth {
            depth = entry.depth;
            if let Some(was) = find(gone, entry) {
                return rows(index) - rows(was);
            }
        }
    }
    0
}

/// Where two lists of contents rows differ: from which row, how many went, and how many came. Their matching starts and ends are compared, and it's only asked when the lists differ, which is when a chapter closes or opens.
fn rows_changed(before: &[ContentsEntry], after: &[ContentsEntry]) -> (u32, u32, u32) {
    if std::ptr::eq(before, after) {
        return (0, 0, 0);
    }
    let start = before.iter().zip(after).take_while(|(one, two)| one == two).count();
    let end = before[start..]
        .iter()
        .rev()
        .zip(after[start..].iter().rev())
        .take_while(|(one, two)| one == two)
        .count();
    let count = |list: &[ContentsEntry]| u32::try_from(list.len() - start - end).unwrap_or(u32::MAX);
    (u32::try_from(start).unwrap_or(u32::MAX), count(before), count(after))
}

/// What a pane shows, from the session.
pub struct PaneSnapshot {
    pub id: TabId,
    pub pages: u32,
    pub page: u32,
    pub map: PageMap,
    /// Tabs this pane's page view keeps warm besides: pane 0 has the warm tabs off screen, and the others none.
    pub warm: Vec<u32>,
    pub problem: Option<String>,
    pub saved: Option<SavedPlace>,
    pub spread: record::Spread,
    /// Its file's name, cleaned, for its pane's label.
    pub title: String,
    /// A jump the page view hasn't taken: where to land, as far down the page as it says.
    pub jump: Option<recto_session::trail::Stop>,
    /// Its contents and page labels, shared, so a link's hint can say a page by its label without asking the session.
    pub toc: Option<Arc<recto_core::Toc>>,
}

pub fn pane_snapshot<L: DocLoader>(session: &recto_session::Session<L>, id: TabId, first: bool, panes: &[TabId]) -> Option<PaneSnapshot> {
    let tab = session.tabs().tab(id)?;
    let own = session.tabs().path_of(id)?;
    let same_file = session.tabs().own_stamp(id).map(version_of);
    // A save wrote these pages as they were shown: the page view goes on with its layout, place and selection.
    let follows = session.saved_from(id).map(version_of);
    let mut map = match session.lists().get(id) {
        None => PageMap {
            files: vec![own],
            slots: None,
            own: same_file,
            turns: None,
            expected: Vec::new(),
            follows,
        },
        Some(edits) => {
            let mut files = vec![own];
            files.extend(edits.added_sources().iter().map(|added| added.path.to_path_buf()));
            // A page's quarter turns, one byte a page of an edited tab, and none at all while nothing's turned.
            let turns = edits
                .slots()
                .iter()
                .any(|slot| slot.quarter_turns % 4 != 0)
                .then(|| edits.slots().iter().map(|slot| slot.quarter_turns).collect::<Arc<[u8]>>());
            // Pages only turned are the file's own pages in its own order, which is what was drawn already: the same map, so nothing's dropped.
            let own_order = edits.added_sources().is_empty()
                && edits.len() == edits.file_pages()
                && edits.slots().iter().enumerate().all(|(at, slot)| slot.source == 0 && slot.page as usize == at);
            let slots = (!own_order).then(|| edits.slots().iter().map(|slot| (slot.source, slot.page)).collect());
            // What each added PDF was when its pages were added: a page from one that's changed since isn't drawn.
            let mut expected = vec![None];
            expected.extend(edits.added_sources().iter().map(|added| added.stamp));
            PageMap {
                files,
                slots,
                own: same_file,
                turns,
                expected,
                follows,
            }
        }
    };
    // The tab's view turned, as the state record carries it: every page drawn turned on top of what its edits turn, and nothing about the file. One byte a page of a hot or warm tab, and none while it's not turned.
    let view_turns = tab.view_rotation % 4;
    if view_turns != 0 {
        let pages = session.pages().count(id) as usize;
        let turns: Arc<[u8]> = (0..pages)
            .map(|at| (map.turns.as_ref().and_then(|turns| turns.get(at)).copied().unwrap_or(0) + view_turns) % 4)
            .collect();
        map.turns = Some(turns);
    }
    // Pane 0's page view keeps the warm tabs that aren't on screen, as many as the panes leave of the live documents. A tab in another pane is that pane's.
    let warm: Vec<u32> = if first { warm_beside_panes(session.warm().ids(), panes) } else { Vec::new() };
    let problem = session
        .notices()
        .said(id)
        .map(|said| said.message.to_string())
        .or_else(|| tab.problem.map(|problem| problem.message().to_string()));
    let saved = session.tabs().place_to_show(id).map(|place| SavedPlace {
        page: place.page,
        anchor: (place.anchor_page, place.within),
        x: place.x,
        y: place.y,
        document_height: place.document_height,
        magnification: place.zoom,
        page_size: (place.page_width > 0.0 && place.page_height > 0.0).then_some(recto_reader::layout::PageBox {
            width: place.page_width,
            height: place.page_height,
        }),
    });
    // Edits that can't be read just now can't say what's on the pages, so it's the file that's shown.
    let pages = if session.lists().is_asleep(id) {
        tab.page_count
    } else {
        session.pages().count(id)
    };
    Some(PaneSnapshot {
        id,
        pages,
        page: tab.page,
        map,
        warm,
        problem,
        saved,
        spread: tab.spread,
        title: tab.name.to_string(),
        jump: None,
        toc: session.warm().toc(id).cloned(),
    })
}

/// The warm tabs off screen that pane 0's page view keeps: most recent first, and only as many as leave the panes and these at [`recto_session::MAX_WARM`] live documents (SPEC §2).
pub fn warm_beside_panes(warm: impl Iterator<Item = TabId>, panes: &[TabId]) -> Vec<u32> {
    warm.filter(|other| !panes.contains(other))
        .take(recto_session::MAX_WARM.saturating_sub(panes.len()))
        .map(|other| other.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_core::{DocFacts, NONE, OpenError, Toc, TocLine};
    use std::path::Path;

    /// A disk of documents by name: `locked` needs a password, `missing` isn't there, `outlined` has two chapters with a section each, `short` has 2 pages, anything else 10.
    struct Disk;

    impl DocLoader for Disk {
        fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
            let name = path.to_string_lossy();
            if name.contains("missing") {
                return Err(OpenError::NotFound);
            }
            if name.contains("locked") {
                return Err(OpenError::PasswordNeeded);
            }
            let line = |title, page, parent, depth| TocLine { title, page, parent, depth };
            let toc = if name.contains("outlined") {
                Toc::from_lines(
                    &[line("One", 0, NONE, 0), line("One a", 1, 0, 1), line("Two", 5, NONE, 0), line("Two a", 6, 2, 1)],
                    false,
                )
            } else {
                Toc::default()
            };
            Ok(DocFacts {
                page_count: if name.contains("short") { 2 } else { 10 },
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc,
                renderer_only: false,
            })
        }
    }

    fn session() -> Session<Disk> {
        Session::new(Disk)
    }

    fn nothing_held() -> Held {
        Held {
            tabs: Arc::new(Vec::new()),
            contents: Arc::new(Vec::new()),
            sidebar: Arc::new(empty_sidebar()),
            recent_key: None,
        }
    }

    /// What a pass publishes becomes what the next pass holds.
    fn held_after(facts: &Facts) -> Held {
        Held {
            tabs: Arc::clone(&facts.sidebar.tabs),
            contents: Arc::clone(&facts.sidebar.contents),
            sidebar: Arc::new(facts.sidebar.view.clone()),
            recent_key: Some(facts.recent_key),
        }
    }

    fn surfaces() -> Surfaces {
        Surfaces {
            paper: paper_look(None, false),
            note: None,
            selection_notice: None,
            pane_views: Vec::new(),
            holding: false,
            first_pane_empty: false,
            sidebar_width: 240.0,
            save: SaveFacts {
                off: false,
                note: None,
                tooltip: String::new(),
            },
            print_ready: false,
        }
    }

    fn sources() -> NoteSources {
        NoteSources {
            open_failure: None,
            save_failed: false,
            problem: None,
            session: None,
            compare: None,
            engine: None,
        }
    }

    fn line(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    fn stamp(n: u64) -> recto_platform::FileStamp {
        recto_platform::FileStamp {
            size: n,
            mtime_ns: n as i64,
            device: 1,
            inode: n,
        }
    }

    /// An edited tab, with its file's stamp known so the window can name it.
    fn edited_tab(s: &mut Session<Disk>, path: &str) -> TabId {
        let id = s.open(Path::new(path)).unwrap();
        s.apply_file_status(id, recto_session::record::FileStatus::Same(stamp(1)));
        s.edit_pages(id, |edits| edits.rotate(0, 1, 1)).unwrap();
        id
    }

    fn texts(rows: &[SidebarRow]) -> Vec<&str> {
        rows.iter().map(|row| row.text.as_str()).collect()
    }

    /// Failure: a lower source wins over a higher one, so a file that wouldn't open is hidden behind a page view's resolution note, or a failed state save is hidden behind a comparison's note. Each source in turn wins over every one below it.
    #[test]
    fn the_note_goes_by_its_sources_in_order() {
        let all = || NoteSources {
            open_failure: line("open"),
            save_failed: true,
            problem: line("problem"),
            session: line("session"),
            compare: line("compare"),
            engine: line("engine"),
        };
        assert_eq!(all().note(), line("open"));
        assert_eq!(NoteSources { open_failure: None, ..all() }.note().as_deref(), Some(notices::SAVE_FAILED));
        let without_save = |source: NoteSources| NoteSources { save_failed: false, ..source };
        assert_eq!(without_save(NoteSources { open_failure: None, ..all() }).note(), line("problem"));
        let lower = |source: NoteSources| NoteSources {
            problem: None,
            ..without_save(NoteSources { open_failure: None, ..source })
        };
        assert_eq!(lower(all()).note(), line("session"));
        assert_eq!(NoteSources { session: None, ..lower(all()) }.note(), line("compare"));
        assert_eq!(
            NoteSources {
                session: None,
                compare: None,
                ..lower(all())
            }
            .note(),
            line("engine")
        );
    }

    /// Failure: a note appears with nothing to say (an empty line over the page), or a failed save of the app's state is shown when it didn't fail.
    #[test]
    fn with_nothing_to_say_there_is_no_note() {
        assert_eq!(sources().note(), None);
        assert_eq!(
            NoteSources {
                save_failed: false,
                engine: line("lower resolution"),
                ..sources()
            }
            .note(),
            line("lower resolution")
        );
    }

    /// Failure: a state save that failed says nothing, or the sentence is a different one from the session's own words, so a person is told something an agent isn't.
    #[test]
    fn a_failed_state_save_is_said_in_the_sessions_words() {
        assert_eq!(
            NoteSources {
                save_failed: true,
                ..sources()
            }
            .note()
            .as_deref(),
            Some(notices::SAVE_FAILED)
        );
    }

    /// Failure: a copy on a protected document offers no way to go ahead anyway, or a notice that doesn't offer it grows a button. The button is the copy notice's own, and its id is the one `perform` knows.
    #[test]
    fn copy_anyway_is_offered_only_when_the_copy_notice_offers_it() {
        let copy = |offers| SelectionSources {
            copy: Some(CopyNotice {
                text: "This PDF asks not to be copied.".into(),
                offers_copy_anyway: offers,
                fades: !offers,
            }),
            printing: None,
            tool: None,
            action: None,
            update: None,
        };
        let offered = copy(true).notice().unwrap();
        assert_eq!(offered.action_title.as_deref(), Some("Copy anyway"));
        assert_eq!(offered.action_id.as_deref(), Some("edit.copy_anyway"));
        assert!(!offered.fades, "a notice with a button stays until answered");
        let plain = copy(false).notice().unwrap();
        assert!(plain.action_title.is_none() && plain.action_id.is_none());
        assert!(plain.fades);
    }

    /// Failure: a print's Stop, or a tool's own Copy anyway, is hidden by a lower line (an action's or the updater's), or a copy's line is hidden by a print's. The page view's copy line wins, then printing, the tool, an action, and last the updater.
    #[test]
    fn the_line_by_the_page_goes_by_its_sources_in_order() {
        let notice = |text: &str| SelectionNotice {
            text: text.into(),
            action_title: None,
            action_id: None,
            fades: false,
        };
        let all = || SelectionSources {
            copy: Some(CopyNotice {
                text: "copy".into(),
                offers_copy_anyway: false,
                fades: true,
            }),
            printing: Some(notice("print")),
            tool: Some(notice("tool")),
            action: line("action"),
            update: Some(("update".into(), false)),
        };
        let text = |sources: SelectionSources| sources.notice().map(|notice| notice.text);
        assert_eq!(text(all()), line("copy"));
        assert_eq!(text(SelectionSources { copy: None, ..all() }), line("print"));
        assert_eq!(
            text(SelectionSources {
                copy: None,
                printing: None,
                ..all()
            }),
            line("tool")
        );
        assert_eq!(
            text(SelectionSources {
                copy: None,
                printing: None,
                tool: None,
                ..all()
            }),
            line("action")
        );
        assert_eq!(
            text(SelectionSources {
                copy: None,
                printing: None,
                tool: None,
                action: None,
                ..all()
            }),
            line("update")
        );
        assert_eq!(
            text(SelectionSources {
                copy: None,
                printing: None,
                tool: None,
                action: None,
                update: None
            }),
            None
        );
    }

    /// Failure: an action's line stays over the page for good, or the updater's line (which says whether it fades) is made to fade when it mustn't, such as a restart that's asked for.
    #[test]
    fn an_action_fades_and_the_updater_says_for_itself() {
        let just = |sources: SelectionSources| sources.notice().unwrap();
        let none = || SelectionSources {
            copy: None,
            printing: None,
            tool: None,
            action: None,
            update: None,
        };
        assert!(
            just(SelectionSources {
                action: line("Couldn't rotate."),
                ..none()
            })
            .fades
        );
        assert!(
            !just(SelectionSources {
                update: Some(("Update ready".into(), false)),
                ..none()
            })
            .fades
        );
        assert!(
            just(SelectionSources {
                update: Some(("Checked".into(), true)),
                ..none()
            })
            .fades
        );
    }

    /// Failure: the Save icon is missing with edits to save, or stays after they're saved, or goes while a save is under way (so its progress tooltip has nowhere to be).
    #[test]
    fn the_save_icon_shows_with_edits_or_while_saving() {
        let mut s = session();
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.apply_file_status(id, recto_session::record::FileStatus::Same(stamp(1)));
        let icon = |s: &Session<Disk>| reader_view(&read(s, &nothing_held()), surfaces()).save.button;
        assert!(!icon(&s), "nothing to save");
        s.edit_pages(id, |edits| edits.rotate(0, 1, 1)).unwrap();
        assert!(icon(&s), "an edit to save");
        s.begin_save(id, false).unwrap();
        assert!(icon(&s), "while saving");
        // A save that has taken the edits leaves nothing edited, and the icon stays for its progress.
        let mut facts = read(&s, &nothing_held());
        facts.reading.edited = false;
        facts.reading.saving = true;
        assert!(reader_view(&facts, surfaces()).save.button, "saving, with nothing left edited");
    }

    /// Failure: the Save icon shows with no document (a stray 'edited' flag with no tab to save), so a click has nothing it is about.
    #[test]
    fn the_save_icon_never_shows_with_nothing_open() {
        let facts = read(&session(), &nothing_held());
        assert_eq!(facts.reading.shown_tab(), 0);
        let mut reading = ReadingFacts::none(false);
        reading.edited = true;
        reading.saving = true;
        let facts = Facts { reading, ..facts };
        assert!(!reader_view(&facts, surfaces()).save.button);
    }

    /// Failure: Save stays on while a save is already running (a second one starts over the first), or it stays on when a signature would be broken by it, or it is off when nothing is in the way.
    #[test]
    fn save_is_off_while_saving_or_when_the_signature_rule_says_so() {
        let mut s = session();
        let id = edited_tab(&mut s, "/d/a.pdf");
        let facts = read(&s, &nothing_held());
        assert!(reader_view(&facts, surfaces()).save.enabled);
        let signed = Surfaces {
            save: SaveFacts {
                off: true,
                note: line("The new copy won't be signed."),
                tooltip: "off".into(),
            },
            ..surfaces()
        };
        let view = reader_view(&facts, signed);
        assert!(!view.save.enabled);
        assert_eq!(
            view.save.as_note.as_deref(),
            Some("The new copy won't be signed."),
            "Save As says what its copy is"
        );
        assert_eq!(view.save.tooltip, "off");
        s.begin_save(id, false).unwrap();
        let view = reader_view(&read(&s, &nothing_held()), surfaces());
        assert!(view.save.saving && !view.save.enabled);
    }

    /// Failure: the home state shows while a tab is being brought to the window (a flash of the empty window between two tabs), or doesn't show with nothing open.
    #[test]
    fn home_is_no_tab_showing_and_none_on_its_way() {
        let facts = read(&session(), &nothing_held());
        let home = |first_pane_empty, holding| {
            reader_view(
                &facts,
                Surfaces {
                    first_pane_empty,
                    holding,
                    ..surfaces()
                },
            )
            .window
            .home
        };
        assert!(home(true, false));
        assert!(!home(true, true), "a tab is being brought");
        assert!(!home(false, false), "a tab is showing");
    }

    /// Failure: an empty window draws leftovers: a page number, a layout, a document name, or buttons that are on with nothing to act on.
    #[test]
    fn an_empty_window_projects_nothing_to_act_on() {
        let facts = read(&session(), &nothing_held());
        let view = reader_view(&facts, surfaces());
        assert!(facts.panes.is_empty() && facts.active.is_none() && facts.comparison.is_none());
        assert_eq!(
            (
                view.document.place.as_str(),
                view.document.layout.as_str(),
                view.document.name.as_str(),
                view.document.page_text.as_str()
            ),
            ("", "", "", "")
        );
        assert!(
            !view.edit.edited && !view.edit.can_undo && !view.edit.can_redo && !view.edit.can_rotate && !view.edit.can_go_back && !view.edit.can_go_forward
        );
        assert_eq!(view.document.shown_tab, 0);
        assert!(view.panes.is_empty());
        assert_eq!(facts.sidebar.view.tab_count, 0);
        assert_eq!(facts.sidebar.view.contents_count, 0);
    }

    /// Failure: a tab whose document needs a password shows its pages anyway or loses its tab: it has to be a tab with a problem the window can say, with no contents rows and no page text of a document that wasn't read.
    #[test]
    fn a_protected_document_is_a_tab_with_a_problem() {
        let mut s = session();
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.needs_password_again(id);
        assert!(s.tabs().needs_password(id));
        let facts = read(&s, &nothing_held());
        let shown = facts.reading_pane().expect("the tab is a pane");
        assert!(shown.problem.is_some(), "the window can say what is wrong");
        assert_eq!(facts.sidebar.view.tab_count, 1);
        assert_eq!(facts.sidebar.view.contents_count, 0, "nothing was read, so there are no contents");
    }

    /// Failure: a tab whose file has gone is shown as if nothing were wrong: its row has no notice and the reader gets no problem to say.
    #[test]
    fn a_tab_whose_file_went_has_a_notice_in_its_row_and_a_problem() {
        let mut s = session();
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        assert!(read(&s, &nothing_held()).reading_pane().unwrap().problem.is_none());
        s.apply_file_status(id, recto_session::record::FileStatus::Missing);
        let facts = read(&s, &nothing_held());
        assert!(facts.sidebar.tabs[0].notice.is_some(), "the row says it on hover");
        assert!(facts.reading_pane().unwrap().problem.is_some());
    }

    /// Failure: contents rows of a tab in the background are projected beside the active tab's, so the sidebar lists two documents' chapters under one tab and a cold tab costs what a warm one does.
    #[test]
    fn only_the_active_tab_has_contents_rows() {
        let mut s = session();
        let first = s.open(Path::new("/d/outlined-one.pdf")).unwrap();
        let second = s.open(Path::new("/d/outlined-two.pdf")).unwrap();
        let facts = read(&s, &nothing_held());
        assert_eq!(facts.sidebar.view.tab_count, 2);
        assert_eq!(facts.sidebar.view.contents_count, 3, "one document's chapters, not six");
        assert!(facts.sidebar.contents.iter().all(|entry| entry.tab == second.0));
        s.tap(first).unwrap();
        assert!(
            read(&s, &nothing_held()).sidebar.contents.iter().all(|entry| entry.tab == first.0),
            "they follow the active tab"
        );
    }

    /// Failure: the paper doesn't follow the window when nothing's chosen, or warm paper changes the paper the page views are told (a redraw of every tile) instead of laying a tint over white.
    #[test]
    fn warm_paper_is_white_with_a_tint_and_nothing_chosen_follows_the_window() {
        let warm = paper_look(Some(Paper::Warm), true);
        assert_eq!((warm.kind, warm.tint), (PaperKind::White, Some(WARM_PAPER)));
        assert_eq!(paper_look(None, true).kind, PaperKind::Dark);
        assert_eq!(paper_look(None, false).kind, PaperKind::White);
        assert_eq!(paper_look(Some(Paper::White), true).kind, PaperKind::White, "a choice beats the window");
        assert!(paper_look(Some(Paper::Dark), false).tint.is_none());
    }

    /// Failure: the reader view's paper name or colour disagree with the paper being drawn, or a highlight on a dark page multiplies (so it vanishes) instead of lightening.
    #[test]
    fn the_reader_view_carries_the_paper_as_drawn() {
        let facts = read(&session(), &nothing_held());
        let dark = reader_view(
            &facts,
            Surfaces {
                paper: paper_look(Some(Paper::Dark), false),
                ..surfaces()
            },
        );
        assert_eq!(dark.paper.name, "dark");
        assert_eq!(dark.paper.rgb, PaperKind::Dark.rgb());
        assert!(dark.paper.highlight_lighten);
        let warm = reader_view(
            &facts,
            Surfaces {
                paper: paper_look(Some(Paper::Warm), false),
                ..surfaces()
            },
        );
        assert_eq!(
            (warm.paper.name.as_str(), warm.paper.rgb, warm.paper.tint),
            ("warm", 0xFF_FF_FF, Some(WARM_PAPER))
        );
        assert!(!warm.paper.highlight_lighten);
    }

    /// Failure: a comparison's panes aren't labelled with their own file's names, or two panes both read as the one being read, or the label of the pane being read is on the other.
    #[test]
    fn a_comparisons_panes_are_labelled_and_one_is_being_read() {
        let mut s = session();
        let one = s.open(Path::new("/d/alpha.pdf")).unwrap();
        let two = s.open(Path::new("/d/beta.pdf")).unwrap();
        s.compare(&[one, two], Some(two)).unwrap();
        let facts = read(&s, &nothing_held());
        let labels = pane_views(&facts.panes, facts.active, facts.comparison.is_some(), &[None, None]);
        assert_eq!(labels.iter().map(|label| label.title.as_str()).collect::<Vec<_>>(), ["alpha", "beta"]);
        assert_eq!(labels.iter().filter(|label| label.reading).count(), 1);
        assert_eq!(labels.iter().find(|label| label.reading).map(|label| label.tab), Some(two.0));
        assert!(labels.iter().all(|label| !label.layout.is_empty()));
    }

    /// Failure: one tab showing gets a label bar over its page, which the owner's layout has none of, or a comparison that has ended keeps its labels.
    #[test]
    fn one_pane_has_no_label() {
        let mut s = session();
        s.open(Path::new("/d/alpha.pdf")).unwrap();
        let facts = read(&s, &nothing_held());
        assert!(pane_views(&facts.panes, facts.active, facts.comparison.is_some(), &[None]).is_empty());
        assert!(
            reader_view(
                &facts,
                Surfaces {
                    pane_views: Vec::new(),
                    ..surfaces()
                }
            )
            .panes
            .is_empty()
        );
    }

    /// Failure: a pane whose file has a problem shows the page view's note instead (or neither), so the problem of a pane that isn't being read is said nowhere.
    #[test]
    fn a_panes_own_problem_beats_its_page_views_note() {
        let mut s = session();
        let one = s.open(Path::new("/d/alpha.pdf")).unwrap();
        let two = s.open(Path::new("/d/beta.pdf")).unwrap();
        s.compare(&[one, two], Some(one)).unwrap();
        s.apply_file_status(two, recto_session::record::FileStatus::Missing);
        let facts = read(&s, &nothing_held());
        let notes = [line("lower resolution"), line("lower resolution")];
        let labels = pane_views(&facts.panes, facts.active, true, &notes);
        assert_eq!(labels[0].note, line("lower resolution"), "no problem: the page view's note");
        assert!(
            labels[1].note.is_some() && labels[1].note != line("lower resolution"),
            "a problem: its own words"
        );
    }

    /// Failure: synced scroll over documents of different lengths says nothing, so the second pane seems stuck; or it says it while scroll is separate.
    #[test]
    fn synced_scroll_over_different_lengths_leaves_a_note() {
        let mut s = session();
        let long = s.open(Path::new("/d/long.pdf")).unwrap();
        let short = s.open(Path::new("/d/short.pdf")).unwrap();
        s.compare(&[long, short], None).unwrap();
        assert_eq!(read(&s, &nothing_held()).compare_note, None, "separate scroll says nothing");
        s.set_compare_synced(true);
        let facts = read(&s, &nothing_held());
        assert!(facts.compare_synced);
        assert!(facts.compare_note.as_deref().is_some_and(|note| note.contains("Different lengths")));
        assert!(reader_view(&facts, surfaces()).window.compare_synced);
    }

    /// Failure: with two tabs side by side the page number and edit buttons go by the first pane and not the one being read.
    #[test]
    fn the_buttons_go_by_the_pane_being_read() {
        let mut s = session();
        let one = s.open(Path::new("/d/alpha.pdf")).unwrap();
        let two = s.open(Path::new("/d/beta.pdf")).unwrap();
        s.apply_file_status(two, recto_session::record::FileStatus::Same(stamp(2)));
        s.compare(&[one, two], Some(two)).unwrap();
        s.edit_pages(two, |edits| edits.rotate(0, 1, 1)).unwrap();
        let facts = read(&s, &nothing_held());
        assert_eq!(facts.reading_pane().map(|shown| shown.id), Some(two));
        let view = reader_view(&facts, surfaces());
        assert_eq!(view.document.shown_tab, two.0);
        assert!(view.edit.edited && view.save.button);
        assert_eq!(view.document.name, "beta.pdf");
    }

    /// Failure: the go-to field starts with the wrong page, or with an empty string where the document has no labels (the page number is the fallback).
    #[test]
    fn the_page_text_is_the_number_when_there_is_no_label() {
        let mut s = session();
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.go_to(id, 3).unwrap();
        assert_eq!(read(&s, &nothing_held()).reading.page_text, "4");
    }

    /// Failure: contents rows of the tab being read aren't projected, or they're projected for a tab whose contents are folded away.
    #[test]
    fn the_contents_follow_the_open_tab_and_its_fold() {
        let mut s = session();
        let id = s.open(Path::new("/d/outlined.pdf")).unwrap();
        let facts = read(&s, &nothing_held());
        assert_eq!(texts(&facts.sidebar.tabs), ["outlined"]);
        assert_eq!(
            facts.sidebar.contents.iter().map(|entry| &*entry.text).collect::<Vec<_>>(),
            ["One", "One a", "Two"],
            "the chapter being read is open, the other isn't"
        );
        assert_eq!(facts.sidebar.view.contents_current, Some(0));
        assert_eq!(facts.sidebar.view.active_index, Some(0));
        s.tap(id).unwrap();
        let folded = read(&s, &nothing_held());
        assert!(!folded.sidebar.view.toc_open);
        assert_eq!(folded.sidebar.view.contents_count, 0);
    }

    /// Failure: a pass with nothing changed copies every row again (thousands of strings for a long outline, on each page crossed) or bumps the versions so the host reloads for nothing.
    #[test]
    fn a_pass_with_nothing_changed_keeps_the_held_rows_and_versions() {
        let mut s = session();
        s.open(Path::new("/d/outlined.pdf")).unwrap();
        let first = read(&s, &nothing_held());
        let again = read(&s, &held_after(&first));
        assert!(Arc::ptr_eq(&first.sidebar.tabs, &again.sidebar.tabs));
        assert!(Arc::ptr_eq(&first.sidebar.contents, &again.sidebar.contents));
        assert_eq!(first.sidebar.view, again.sidebar.view);
        assert!(again.recent.is_none(), "Open Recent isn't built again");
    }

    /// Failure: a tab opened or renamed doesn't bump the tabs' version (the host keeps drawing the old rows), or a page turned does (a thousand tab rows copied for a scroll).
    #[test]
    fn the_tabs_version_moves_with_the_tabs_and_not_with_a_page() {
        let mut s = session();
        let one = s.open(Path::new("/d/a.pdf")).unwrap();
        let first = read(&s, &nothing_held());
        s.go_to(one, 4).unwrap();
        let turned = read(&s, &held_after(&first));
        assert_eq!(turned.sidebar.view.tabs_version, first.sidebar.view.tabs_version);
        s.open(Path::new("/d/b.pdf")).unwrap();
        let opened = read(&s, &held_after(&turned));
        assert_eq!(opened.sidebar.view.tabs_version, first.sidebar.view.tabs_version + 1);
        assert_eq!(opened.sidebar.view.tab_count, 2);
        assert_eq!(opened.sidebar.view.active_index, Some(1));
    }

    /// Failure: a chapter closing as the reader leaves it is not said (the host reloads the whole list and the scroll jumps), or the contents' version stays the same for a changed list.
    #[test]
    fn leaving_a_chapter_says_which_rows_went_and_came() {
        let mut s = session();
        let id = s.open(Path::new("/d/outlined.pdf")).unwrap();
        let first = read(&s, &nothing_held());
        s.go_to(id, 6).unwrap();
        let moved = read(&s, &held_after(&first));
        assert_eq!(moved.sidebar.view.contents_version, first.sidebar.view.contents_version + 1);
        let view = &moved.sidebar.view;
        let (from, removed, inserted) = (view.contents_changed_from, view.contents_removed, view.contents_inserted);
        assert_eq!(
            (from, removed, inserted),
            rows_changed(&first.sidebar.contents, &moved.sidebar.contents),
            "it says what differs"
        );
        assert!(removed > 0 && inserted > 0, "rows went and rows came");
        assert_eq!(
            moved.sidebar.contents.iter().map(|entry| &*entry.text).collect::<Vec<_>>(),
            ["One", "Two", "Two a"]
        );
        assert_eq!(view.contents_current, Some(2));
    }

    /// Failure: the tabs side by side read as one active tab and the rest plain, or a tab outside the comparison is drawn as part of it.
    #[test]
    fn tab_rows_mark_the_tabs_in_a_comparison() {
        let mut s = session();
        let one = s.open(Path::new("/d/a.pdf")).unwrap();
        let two = s.open(Path::new("/d/b.pdf")).unwrap();
        s.open(Path::new("/d/c.pdf")).unwrap();
        s.compare(&[one, two], None).unwrap();
        let facts = read(&s, &nothing_held());
        assert_eq!(facts.sidebar.tabs.iter().map(|row| row.in_comparison).collect::<Vec<_>>(), [true, true, false]);
        let before = Arc::clone(&facts.sidebar.tabs);
        s.tap(one).unwrap();
        let after = read(&s, &held_after(&facts));
        assert!(
            after.sidebar.tabs.iter().all(|row| !row.in_comparison),
            "a plain click ends the comparison, and the rows say so"
        );
        assert!(!Arc::ptr_eq(&before, &after.sidebar.tabs));
    }

    /// Failure: Open Recent is rebuilt at every pass (a list of paths copied at each page turn), or isn't rebuilt when a file is opened.
    #[test]
    fn open_recent_is_built_only_when_its_key_moves() {
        let mut s = session();
        s.open(Path::new("/d/a.pdf")).unwrap();
        let first = read(&s, &nothing_held());
        let built = first.recent.as_ref().expect("built the first time");
        assert_eq!(built.len(), 1);
        assert!(built[0].open);
        assert!(read(&s, &held_after(&first)).recent.is_none());
        s.open(Path::new("/d/b.pdf")).unwrap();
        assert_eq!(read(&s, &held_after(&first)).recent.map(|list| list.len()), Some(2));
    }

    /// Failure: the chosen view settings (hidden sidebar, hidden agent icon) don't reach the reader view, so the host draws what the menu says is off.
    #[test]
    fn the_view_settings_reach_the_reader_view() {
        let mut s = session();
        s.viewing_mut().set_view(recto_session::ViewSettings {
            paper: Some(Paper::Warm),
            sidebar_hidden: true,
            agent_icon_hidden: true,
        });
        let facts = read(&s, &nothing_held());
        assert_eq!(facts.chosen_paper, Some(Paper::Warm));
        let view = reader_view(&facts, surfaces());
        assert!(view.window.sidebar_hidden && view.window.agent_icon_hidden);
    }

    /// Failure: rows opening as the reader enters a chapter jump the list under a smooth drift, because the host wasn't told which went and which came. It must say exactly, a chapter closing and the next opening together included, and a change of nothing says nothing.
    #[test]
    fn opening_and_closing_rows_says_what_changed() {
        let row = |text: &str| ContentsEntry {
            tab: 1,
            text: text.into(),
            page: Some(1),
            depth: 0,
            entry: 0,
            expandable: false,
            expanded: false,
        };
        let list = |names: &[&str]| names.iter().map(|name| row(name)).collect::<Vec<_>>();
        let before = list(&["1", "2", "3"]);
        assert_eq!(rows_changed(&before, &list(&["1", "2", "2.1", "2.2", "3"])), (2, 0, 2));
        assert_eq!(rows_changed(&list(&["1", "2", "2.1", "2.2", "3"]), &before), (2, 2, 0));
        assert_eq!(
            rows_changed(&list(&["1", "2", "2.1", "3", "4"]), &list(&["1", "2", "3", "3.1", "4"])),
            (2, 2, 2),
            "2.1 and 3 go, and 3 and 3.1 come"
        );
        assert_eq!(rows_changed(&before, &before), (0, 0, 0));
        assert_eq!(rows_changed(&[], &before), (0, 0, 3));
        assert_eq!(rows_changed(&before, &[]), (0, 3, 0));
    }

    /// Failure: a chapter closes as the reader leaves it, the place drops up the list by the rows that went, and the contents make a second move straight after the first to bring it back. How far the place was carried has to come out right wherever it starts and ends.
    #[test]
    fn a_row_change_says_how_far_it_carried_the_place() {
        let row = |text: &str| ContentsEntry {
            tab: 1,
            text: text.into(),
            page: Some(1),
            depth: u8::try_from(text.matches('.').count()).unwrap(),
            entry: 0,
            expandable: false,
            expanded: false,
        };
        let list = |names: &[&str]| names.iter().map(|name| row(name)).collect::<Vec<_>>();
        let shift = |one: &[&str], place_before: u32, two: &[&str], place_after: u32| {
            let (before, after) = (list(one), list(two));
            place_shift(&before, &after, rows_changed(&before, &after), Some(place_before), Some(place_after))
        };
        let in_two = ["1", "2", "2.1", "2.2", "3", "4"];
        let in_three = ["1", "2", "3", "3.1", "4"];
        assert_eq!(shift(&in_two, 3, &in_three, 2), -2, "forward: 3 was listed, two rows further down");
        assert_eq!(shift(&in_two, 3, &in_three, 3), -2, "forward, to a section that wasn't listed: its chapter was");
        assert_eq!(
            shift(&in_three, 2, &in_two, 3),
            2,
            "back: 2.2 wasn't listed, but 3, the row it came from, still is"
        );
        assert_eq!(shift(&in_two, 5, &["1", "2", "2.1", "2.2", "3", "4", "4.1"], 5), 0, "everything happened below");
        assert_eq!(shift(&in_two, 5, &in_three, 4), -1, "everything happened above");
    }

    /// Failure (review 1): pane 0's page view keeps every warm tab off screen beside a comparison's panes, so three panes and two recent tabs are five live documents where SPEC §2 allows three.
    #[test]
    fn panes_and_warm_tabs_stay_within_the_live_documents() {
        let tabs = |ids: &[u32]| ids.iter().copied().map(TabId).collect::<Vec<_>>();
        assert_eq!(
            warm_beside_panes(tabs(&[5, 4, 3]).into_iter(), &tabs(&[5])),
            vec![4, 3],
            "one tab showing keeps two recent"
        );
        assert_eq!(
            warm_beside_panes(tabs(&[5, 4, 3]).into_iter(), &tabs(&[1, 2])),
            vec![5],
            "two panes leave room for one"
        );
        assert!(
            warm_beside_panes(tabs(&[5, 4, 3]).into_iter(), &tabs(&[1, 2, 3])).is_empty(),
            "three panes are all of them"
        );
        for panes in [tabs(&[1]), tabs(&[1, 2]), tabs(&[1, 2, 3])] {
            assert!(panes.len() + warm_beside_panes(tabs(&[9, 8, 7, 6]).into_iter(), &panes).len() <= recto_session::MAX_WARM);
        }
    }
}
