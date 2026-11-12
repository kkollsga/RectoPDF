//! **The page area's record,** as the host sees it: the reader model of [`crate::model`] flattened, and the sheets that ask something beside it.

use crate::{PaneView, ReaderModel, SelectionNotice};
use recto_reader::PaperKind;
use recto_session::Paper;
use recto_session::record;

/// What the page area shows. **The host's contract as Swift sees it**: `recto-ffi` restates it for UniFFI as it is, field for field, so its shape is frozen here; step
/// R2.3 is where it may change. It's the projection's [`ReaderModel`] flattened, with the bridge's three sheets beside it.
#[derive(Clone, Debug, PartialEq)]
pub struct ReaderView {
    /// Tabs side by side, left to right, a pane each. Empty with one tab showing, which is one pane with no label.
    pub panes: Vec<PaneView>,
    /// Synced scroll's on, which the right-click menu ticks.
    pub compare_synced: bool,
    /// No PDF is open, so it's the home state.
    pub home: bool,
    /// Something the reader needs to be told, such as a page at a lower resolution, in words.
    pub note: Option<String>,
    /// The sidebar's hidden, by its button or the View menu.
    pub sidebar_hidden: bool,
    /// The tab showing's layout, by its name: `single`, `two-up` or `two-up-cover`. Empty with none showing.
    pub layout: String,
    /// The tab showing has unsaved edits, which the red window button's dot shows.
    pub edited: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Edit mode's on, which the panel's Rotate goes by. Published, so drawing the panel never waits on the session.
    pub edit_mode: bool,
    /// Whether the panel's Rotate can turn the tab showing: there is one, more than the renderer can read it, and its edits can be read.
    pub can_rotate: bool,
    /// The page number in the lower right, such as "32 of 96". Empty with nothing open. The sidebar has no footer saying it again (the owner's default of 2026-11-08).
    pub place: String,
    /// The paper's name, which QA and the menu use.
    pub paper: String,
    /// The paper's colour, `0xRRGGBB`.
    pub paper_rgb: u32,
    /// Whether the star at the top right is hidden.
    pub agent_icon_hidden: bool,
    /// A colour to multiply over each page in the compositor, for warm paper. No tile changes for it.
    pub page_tint: Option<u32>,
    pub sidebar_width: f64,
    /// Which of the system's selection colours a highlight uses, going by the page and not the window.
    pub highlight_dark_colour: bool,
    /// A highlight lightens the page under it, for a dark page, or multiplies it. See `recto_reader::paper::HighlightLook`.
    pub highlight_lighten: bool,
    /// A line by the page about a copy, going by itself if it says so.
    pub selection_notice: Option<SelectionNotice>,
    /// A save of the tab showing is under way.
    pub saving: bool,
    /// What the Save icon beside the window's controls says, and the step a long save is at.
    pub save_tooltip: String,
    /// A save that didn't happen, for an alert over the window until it's answered.
    pub save_problem: Option<crate::saving::SaveProblem>,
    /// What Save As's sheet says about this document, as for a signed one.
    pub save_as_note: Option<String>,
    /// The tab showing, which an action from the window is about. 0 with none.
    pub shown_tab: u32,
    /// Its file's name and folder, for Save As's sheet and the close prompt, so neither waits on the session.
    pub document_name: String,
    pub document_folder: String,
    /// Whether the Save icon is beside the window's controls: with unsaved edits, or while saving.
    pub save_button: bool,
    /// A print's pages are ready: call `take_print`, and hand the file to the system's printing.
    pub print_ready: bool,
    /// A password being asked for, for the system's sheet with a secure field.
    pub password_prompt: Option<crate::passwords::PasswordPrompt>,
    /// Whether it does anything: not while saving, and not when Save over the file would break a person's signature, which Save As's unsigned copy is the way round.
    pub save_enabled: bool,
    /// Back and Forward can go somewhere in the tab being read: Go > Back and Forward, ⌘[ and ⌘], and a swipe.
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// A web or email address a click asked to open, for the sheet that asks first.
    pub link_prompt: Option<crate::linking::LinkPrompt>,
    /// The page being read, by its label if it has one: what the go-to field starts with.
    pub page_text: String,
}

impl ReaderView {
    /// What it is before the session thread's first pass: the home screen, on white paper, with nothing open.
    #[must_use]
    pub fn home() -> Self {
        Self {
            panes: Vec::new(),
            compare_synced: false,
            home: true,
            note: None,
            sidebar_hidden: false,
            layout: String::new(),
            edited: false,
            can_undo: false,
            can_redo: false,
            edit_mode: false,
            can_rotate: false,
            place: String::new(),
            paper: Paper::White.name().to_string(),
            paper_rgb: PaperKind::White.rgb(),
            page_tint: None,
            agent_icon_hidden: false,
            sidebar_width: record::SIDEBAR_WIDTH,
            highlight_dark_colour: false,
            highlight_lighten: false,
            selection_notice: None,
            saving: false,
            save_tooltip: String::new(),
            save_problem: None,
            save_as_note: None,
            shown_tab: 0,
            document_name: String::new(),
            document_folder: String::new(),
            save_button: false,
            save_enabled: false,
            print_ready: false,
            can_go_back: false,
            can_go_forward: false,
            link_prompt: None,
            page_text: String::new(),
            password_prompt: None,
        }
    }
}

/// The reader model with its sheets empty: the caller sets them.
impl From<ReaderModel> for ReaderView {
    fn from(model: ReaderModel) -> Self {
        Self {
            panes: model.panes,
            compare_synced: model.window.compare_synced,
            home: model.window.home,
            note: model.note,
            sidebar_hidden: model.window.sidebar_hidden,
            layout: model.document.layout,
            edited: model.edit.edited,
            can_undo: model.edit.can_undo,
            can_redo: model.edit.can_redo,
            edit_mode: model.edit.edit_mode,
            can_rotate: model.edit.can_rotate,
            place: model.document.place,
            paper: model.paper.name,
            paper_rgb: model.paper.rgb,
            agent_icon_hidden: model.window.agent_icon_hidden,
            page_tint: model.paper.tint,
            sidebar_width: model.window.sidebar_width,
            highlight_dark_colour: model.paper.highlight_dark_colour,
            highlight_lighten: model.paper.highlight_lighten,
            selection_notice: model.selection_notice,
            saving: model.save.saving,
            save_tooltip: model.save.tooltip,
            save_problem: None,
            save_as_note: model.save.as_note,
            shown_tab: model.document.shown_tab,
            document_name: model.document.name,
            document_folder: model.document.folder,
            save_button: model.save.button,
            print_ready: model.window.print_ready,
            password_prompt: None,
            save_enabled: model.save.enabled,
            can_go_back: model.edit.can_go_back,
            can_go_forward: model.edit.can_go_forward,
            link_prompt: None,
            page_text: model.document.page_text,
        }
    }
}
