//! **The codes a refusal is known by** (step R2.5): one table, each code's wire word and its next step, so the window routes on a named code, never on a
//! string, and nothing that names a code can mistype it. A refusal's code is a [`Code`]; its words for a person are the refusal's message, which nothing
//! decides by.
//!
//! [`Code`] is a word with a private field, not an enum: there are more codes than the budget for an enum's variants allows, and they're a table, not a
//! type with behaviour of each. What an enum would give is held by tests instead (`tests/codes.rs`): every code in the goldens is listed, every entry
//! says its next step, no two share a word, [`Code::ALL`] is every entry, every code any crate names is listed, no code is made from a word written in
//! place, and no code outside this module is compared as a string. The set is closed: a code is a listed one, and [`Code::lower`] sends a word the table
//! doesn't list as [`Code::INTERNAL`], never as a new word on the wire.

use crate::exit;
use std::fmt;

/// A refusal's code: the word on the wire that doesn't change, and what to do about it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Code(&'static str);

/// What a code says to do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// These words.
    Says(&'static str),
    /// The words for its kind of exit.
    ByExit,
}

/// The table: each code's const, its wire word and its next step. One line each, `NAME = "word", next;` (a comma, not an arrow, so no scan for a code
/// compared reads a row as a comparison).
macro_rules! codes {
    ($($name:ident = $wire:literal, $next:expr;)*) => {
        impl Code {
            $(pub const $name: Code = Code($wire);)*

            /// Every code, in the table's order.
            pub const ALL: &'static [Code] = &[$(Code::$name,)*];

            /// What it says to do next, as the table has it.
            #[must_use]
            pub fn next(self) -> Next {
                match self.0 {
                    $($wire => $next,)*
                    _ => Next::ByExit,
                }
            }
        }
    };
}

impl Code {
    /// Its word on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The listed code with this word, if there is one.
    #[must_use]
    pub fn listed(word: &str) -> Option<Code> {
        Self::ALL.iter().copied().find(|code| code.0 == word)
    }

    /// A code a crate below this one names (a session's refusal, a save's, the renderer's, the updater's), as the code it is here. Each such word is listed,
    /// which a test holds by reading every crate's sources; one that somehow isn't is a bug, which fails a debug build here, and in a release build goes out
    /// as [`Code::INTERNAL`]: never a word the table doesn't list.
    #[must_use]
    pub fn lower(word: &str) -> Code {
        Self::listed(word).unwrap_or_else(|| {
            debug_assert!(false, "`{word}` is named below and isn't in the table of codes");
            Self::INTERNAL
        })
    }

    /// The next step for this code, by its entry, or else by its kind of exit. Every error says one.
    #[must_use]
    pub fn next_step(self, exit_code: i32) -> &'static str {
        match self.next() {
            Next::Says(words) => words,
            Next::ByExit => match exit_code {
                exit::NOT_FOUND => "`recto tabs` lists what's open.",
                exit::AMBIGUOUS => "Use one of the candidates.",
                exit::PERMISSION => "Ask the user for more access.",
                exit::NOT_RUNNING => "Start it with `recto serve --background`.",
                exit::TIMED_OUT => "Try again, or with a longer `--timeout`.",
                exit::PROTECTED => "Ask the user: only they can unlock this.",
                exit::BUSY => "Try again in a moment.",
                exit::USAGE => "`recto help` lists every command.",
                _ => "`recto status` shows whether the session is healthy. If this keeps happening, tell the user what you tried.",
            },
        }
    }
}

/// Its word, as the wire has it.
impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// As its word in quotes, as it read when it was a string.
impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.0, f)
    }
}

impl serde::Serialize for Code {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (review of R2.5, F3): a word a crate below says that the table doesn't list goes out on the wire as a code of its own, which nothing
    /// documents or routes on. It's a bug, which a debug build stops at, and a release build sends as `internal`.
    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "isn't in the table of codes"))]
    fn a_word_the_table_doesnt_list_never_goes_out_as_a_code() {
        assert_eq!(Code::lower("unlisted_word"), Code::INTERNAL);
    }
}

/// For a test that reads a refusal as the wire does, `assert_eq!(error.code, "usage")`: only in a test build, this crate's own or another crate's through
/// the `test-words` feature, which only dev-dependencies (and `recto-app`'s `testkit`) turn on. A release build has no string comparison at all, so code
/// that ships compares named codes; in a workspace's test build, where features unify, `tests/codes.rs` holds that by reading every source outside tests.
#[cfg(any(test, feature = "test-words"))]
impl PartialEq<&str> for Code {
    fn eq(&self, word: &&str) -> bool {
        self.0 == *word
    }
}

#[cfg(any(test, feature = "test-words"))]
impl PartialEq<str> for Code {
    fn eq(&self, word: &str) -> bool {
        self.0 == word
    }
}

codes! {
    ADDED_FILE_CHANGED = "added_file_changed", Next::ByExit;
    ADDED_FILE_PROTECTED = "added_file_protected", Next::ByExit;
    ADDED_FILE_UNREADABLE = "added_file_unreadable", Next::ByExit;
    ALREADY_RUNNING = "already_running", Next::ByExit;
    AMBIGUOUS = "ambiguous", Next::Says("Use one of the candidates, written as `#id`.");
    ANOTHER_COPY = "another_copy", Next::ByExit;
    ANOTHER_HEAVY_PAGE = "another_heavy_page", Next::ByExit;
    APP_MISSING = "app_missing", Next::ByExit;
    APP_REFUSED = "app_refused", Next::ByExit;
    BAD_ANSWER = "bad_answer", Next::ByExit;
    BAD_COMMAND = "bad_command", Next::Says("`recto help` lists every command.");
    BAD_DPI = "bad_dpi", Next::ByExit;
    BAD_EVENTS = "bad_events", Next::ByExit;
    BAD_LIMIT = "bad_limit", Next::ByExit;
    BAD_OUTPUT = "bad_output", Next::Says("Choose a file name ending in .png.");
    BAD_PAGES = "bad_pages", Next::ByExit;
    BAD_PATH = "bad_path", Next::Says("Give the whole path. `recto` does this for you.");
    BAD_PATTERN = "bad_pattern", Next::ByExit;
    BAD_QUERY = "bad_query", Next::ByExit;
    BAD_RECT = "bad_rect", Next::Says("Write it as `--rect 10,20,300,200`, in points from the page's top left.");
    BAD_REQUEST = "bad_request", Next::ByExit;
    BAD_ROTATION = "bad_rotation", Next::ByExit;
    BAD_SAVE_PATH = "bad_save_path", Next::ByExit;
    BAD_SCALE = "bad_scale", Next::ByExit;
    BAD_STRATEGY = "bad_strategy", Next::ByExit;
    BAD_TEXT_MODE = "bad_text_mode", Next::Says("Use `--text both`, `paths` or `selectable`.");
    BAD_VIEWPORT = "bad_viewport", Next::ByExit;
    BREAKS_CERTIFICATION = "breaks_certification", Next::ByExit;
    BUDGET_WRONG_TABS = "budget_wrong_tabs", Next::ByExit;
    BUSY = "busy", Next::Says("Try again in a moment.");
    CANCELLED = "cancelled", Next::ByExit;
    CHANGED = "changed", Next::ByExit;
    CHANGED_SINCE_OPENED = "changed_since_opened", Next::ByExit;
    CHANGED_WHILE_COPYING = "changed_while_copying", Next::ByExit;
    CHECK_FAILED = "check_failed", Next::ByExit;
    CHECKSUM = "checksum", Next::ByExit;
    CLEAR_INCOMPLETE = "clear_incomplete", Next::ByExit;
    CONNECTION_DROPPED = "connection_dropped", Next::ByExit;
    COPY_RESTRICTED = "copy_restricted", Next::ByExit;
    DAMAGED = "damaged", Next::ByExit;
    DEV_BUILDS_OFF = "dev_builds_off", Next::ByExit;
    DEVELOPMENT_BUILD = "development_build", Next::ByExit;
    DISK_FULL = "disk_full", Next::ByExit;
    DOCUMENT_STOPPED = "document_stopped", Next::ByExit;
    DOWNLOAD_FAILED = "download_failed", Next::ByExit;
    EDITS_DONT_FIT = "edits_dont_fit", Next::ByExit;
    EDITS_DONT_MATCH = "edits_dont_match", Next::ByExit;
    EDITS_NOT_MERGED = "edits_not_merged", Next::ByExit;
    EDITS_UNREADABLE = "edits_unreadable", Next::ByExit;
    EDITS_VERSION_UNKNOWN = "edits_version_unknown", Next::ByExit;
    EXPORT_MEMORY_TIGHT = "export_memory_tight", Next::ByExit;
    EXPORT_PAGE_TOO_HEAVY = "export_page_too_heavy", Next::ByExit;
    EXPORT_WAIT_TIMED_OUT = "export_wait_timed_out", Next::ByExit;
    FIELD_NAMES_TAKEN = "field_names_taken", Next::ByExit;
    FILE_CHANGED = "file_changed", Next::ByExit;
    FILE_CRASHED_RENDERER = "file_crashed_renderer", Next::ByExit;
    FILE_MISSING = "file_missing", Next::ByExit;
    FILE_MOVED = "file_moved", Next::ByExit;
    FILE_NOT_FOUND = "file_not_found", Next::ByExit;
    FILE_TOO_LARGE_TO_OPEN = "file_too_large_to_open", Next::ByExit;
    FILE_TOO_SLOW_TO_OPEN = "file_too_slow_to_open", Next::ByExit;
    FILE_UNREADABLE = "file_unreadable", Next::ByExit;
    GATEKEEPER = "gatekeeper", Next::ByExit;
    GENERATING_FAILED = "generating_failed", Next::ByExit;
    HAND_OFF_REFUSED = "hand_off_refused", Next::ByExit;
    HARD_LINKS = "hard_links", Next::ByExit;
    HISTORY_FULL = "history_full", Next::ByExit;
    HISTORY_ROLLED_OFF = "history_rolled_off", Next::ByExit;
    INDEX_FAILED = "index_failed", Next::ByExit;
    INTERNAL = "internal", Next::ByExit;
    LAST_PAGE = "last_page", Next::Says("Keep at least one page, or delete fewer.");
    LATER = "later", Next::ByExit;
    LINK_GOES_NOWHERE = "link_goes_nowhere", Next::ByExit;
    LINK_REFUSED = "link_refused", Next::ByExit;
    LOCKED_BY_SIGNATURE = "locked_by_signature", Next::ByExit;
    MEMORY_TIGHT = "memory_tight", Next::ByExit;
    NEEDS_APP = "needs_app", Next::ByExit;
    NEEDS_PASSWORD = "needs_password", Next::Says("Ask the user to open it in the app and allow it. Passwords are never taken here.");
    NEEDS_SERVER = "needs_server", Next::Says("Start the session with `recto serve --background`, then use `recto` without `local`.");
    NEEDS_TEMP_STATE = "needs_temp_state", Next::ByExit;
    NEEDS_TERMINAL = "needs_terminal", Next::Says("Ask the user to run this command themselves, in their own terminal.");
    NO_ACTIVE_TAB = "no_active_tab", Next::ByExit;
    NO_ANSWER = "no_answer", Next::ByExit;
    NO_GENERATED_CONTENTS = "no_generated_contents", Next::ByExit;
    NO_HANDOVER_FOLDER = "no_handover_folder", Next::ByExit;
    NO_INSTALLER = "no_installer", Next::ByExit;
    NO_PAGES = "no_pages", Next::ByExit;
    NO_PAGES_LEFT = "no_pages_left", Next::ByExit;
    NO_RECORD = "no_record", Next::ByExit;
    NO_STATE = "no_state", Next::ByExit;
    NO_SUCH_ENTRY = "no_such_entry", Next::ByExit;
    NO_SUCH_HIT = "no_such_hit", Next::ByExit;
    NO_SUCH_IMAGE = "no_such_image", Next::ByExit;
    NO_SUCH_LABEL = "no_such_label", Next::ByExit;
    NO_SUCH_LINK = "no_such_link", Next::ByExit;
    NO_SUCH_PAGE = "no_such_page", Next::Says("`recto where` gives the tab's length. Pages count from 1.");
    NO_SUCH_TAB = "no_such_tab", Next::Says("`recto tabs` lists what's open. `recto open <file.pdf>` opens a file.");
    NO_SWAP = "no_swap", Next::ByExit;
    NO_TEMP = "no_temp", Next::ByExit;
    NOT_A_BUILD = "not_a_build", Next::ByExit;
    NOT_A_FILE = "not_a_file", Next::ByExit;
    NOT_A_FOLDER = "not_a_folder", Next::ByExit;
    NOT_A_JPEG = "not_a_jpeg", Next::ByExit;
    NOT_A_PDF = "not_a_pdf", Next::ByExit;
    NOT_A_SECTION = "not_a_section", Next::ByExit;
    NOT_ALLOWED = "not_allowed", Next::Says("Ask the user to give this user account access to the file.");
    NOT_AN_APP = "not_an_app", Next::ByExit;
    NOT_BUILT = "not_built", Next::Says("Tell the user this isn't in this version yet.");
    NOT_COMPARING = "not_comparing", Next::ByExit;
    NOT_CONFIRMED = "not_confirmed", Next::ByExit;
    NOT_FOUND = "not_found", Next::Says("Check the path. It's read by the session, and relative paths are taken from where you ran `recto`.");
    NOT_GENERATED = "not_generated", Next::ByExit;
    NOT_HANDED_OVER = "not_handed_over", Next::ByExit;
    NOT_HERE = "not_here", Next::ByExit;
    NOT_INDEXED = "not_indexed", Next::ByExit;
    NOT_PDF = "not_pdf", Next::Says("This file can't be opened. Try another file.");
    NOT_READY = "not_ready", Next::ByExit;
    NOT_RECTOPDF = "not_rectopdf", Next::ByExit;
    NOT_RUNNING = "not_running", Next::Says("Start it with `recto serve --background`.");
    NOT_SIDE_BY_SIDE = "not_side_by_side", Next::ByExit;
    NOT_THE_SHOWN_TAB = "not_the_shown_tab", Next::ByExit;
    NOT_WRITABLE = "not_writable", Next::ByExit;
    NOTHING_BACK = "nothing_back", Next::ByExit;
    NOTHING_FORWARD = "nothing_forward", Next::ByExit;
    NOTHING_FOUND = "nothing_found", Next::ByExit;
    NOTHING_ON_OFFER = "nothing_on_offer", Next::ByExit;
    NOTHING_TO_DO = "nothing_to_do", Next::ByExit;
    NOTHING_TO_REDO = "nothing_to_redo", Next::Says("`recto edits` shows where this tab is.");
    NOTHING_TO_UNDO = "nothing_to_undo", Next::Says("`recto edits` shows where this tab is.");
    ONLY_THE_USER_QUITS = "only_the_user_quits", Next::ByExit;
    OPEN_IN_ANOTHER_TAB = "open_in_another_tab", Next::ByExit;
    OTHER_INSTANCE = "other_instance", Next::ByExit;
    OUTPUT_EXISTS = "output_exists", Next::Says("Choose a new file name. An existing file is never replaced.");
    OUTPUT_TOO_LARGE = "output_too_large", Next::ByExit;
    OUTSIDE_QUIET_RUN = "outside_quiet_run", Next::ByExit;
    OWNER_EDITS_AT_STAKE = "owner_edits_at_stake", Next::ByExit;
    OWNER_GONE = "owner_gone", Next::ByExit;
    PAGE_CRASHED_RENDERER = "page_crashed_renderer", Next::ByExit;
    PAGE_FAILED = "page_failed", Next::ByExit;
    PAGE_NOT_DRAWABLE_NOW = "page_not_drawable_now", Next::ByExit;
    PAGE_OUT_OF_RANGE = "page_out_of_range", Next::Says("`recto where` gives the tab's length. Pages count from 1.");
    PAGE_TOO_HEAVY = "page_too_heavy", Next::ByExit;
    PAGE_TOO_LARGE = "page_too_large", Next::ByExit;
    PAGE_TOO_LARGE_TO_READ = "page_too_large_to_read", Next::Says("Tell the user this page can't be read. The rest of the document may be fine.");
    PAGE_TOO_SLOW = "page_too_slow", Next::ByExit;
    PAGE_UNREADABLE = "page_unreadable", Next::ByExit;
    PALETTE_NEEDS_APP = "palette_needs_app", Next::ByExit;
    PARSERS_DISAGREE = "parsers_disagree", Next::ByExit;
    PASSWORD_AGAIN = "password_again", Next::ByExit;
    PASSWORD_NEEDED = "password_needed", Next::Says("Ask the user to open it in the app and allow it. Passwords are never taken here.");
    PATH_REFUSED = "path_refused", Next::ByExit;
    PATTERN_TOO_LARGE = "pattern_too_large", Next::ByExit;
    PERMISSION_DENIED = "permission_denied", Next::ByExit;
    PERSONS_OWN = "persons_own", Next::ByExit;
    PREPARE_FAILED = "prepare_failed", Next::ByExit;
    PROTECTED = "protected", Next::Says("Tell the user. Only they can allow an agent to read a protected document, and this version can't ask them, so work on another file.");
    QA_FAILED = "qa_failed", Next::ByExit;
    QA_NEEDS_APP = "qa_needs_app", Next::ByExit;
    QA_REFUSED = "qa_refused", Next::ByExit;
    READ_BACK_DIFFERS = "read_back_differs", Next::ByExit;
    READ_ONLY = "read_only", Next::ByExit;
    RECORD_DAMAGED = "record_damaged", Next::ByExit;
    RECORD_NEWER = "record_newer", Next::ByExit;
    RECORD_TOO_LARGE = "record_too_large", Next::ByExit;
    RECORD_UNREADABLE = "record_unreadable", Next::ByExit;
    RELEASE_COPY = "release_copy", Next::ByExit;
    RENDER_LIBRARY_MISSING = "render_library_missing", Next::Says("Tell the user RectoPDF's install is incomplete. In a source tree, `tools/pdfium.sh` fetches the library.");
    RENDER_TOOK_TOO_LONG = "render_took_too_long", Next::ByExit;
    RENDER_UNAVAILABLE = "render_unavailable", Next::ByExit;
    RENDER_WORKER_MISSING = "render_worker_missing", Next::Says("Tell the user RectoPDF's install is incomplete: `recto-render-service` must be in the app's `Contents/Helpers/RectoRender.app`, or in a source tree in the same folder as `recto`. There, `cargo build --workspace` builds both, which `cargo build -p recto-cli` doesn't.");
    RENDERER_INSIDE_ANOTHER_SANDBOX = "renderer_inside_another_sandbox", Next::Says("Run `recto` outside the sandbox it's in, or ask the user to: the renderer can't start inside another sandbox. Nothing needs reinstalling.");
    RENDERER_LAUNCHD_REACHABLE = "renderer_launchd_reachable", Next::Says("Tell the user RectoPDF's renderer can't close itself off on this version of macOS, so it won't read PDFs here, and give them this code and their macOS version to report. Nothing needs reinstalling.");
    RENDERER_MACH_DROP_FAILED = "renderer_mach_drop_failed", Next::Says("Tell the user RectoPDF's renderer can't close itself off on this version of macOS, so it won't read PDFs here, and give them this code and their macOS version to report. Nothing needs reinstalling.");
    RENDERER_MISBEHAVED = "renderer_misbehaved", Next::ByExit;
    RENDERER_NOT_SANDBOXED = "renderer_not_sandboxed", Next::Says("Tell the user this copy of RectoPDF is signed wrong, so its renderer refuses to read any PDF. Installing a release again fixes it. In a source tree, `app/build.sh` signs the helper as it should be.");
    RENDERER_ONLY = "renderer_only", Next::Says("This file can still be read and drawn. To change it, it first needs repairing in another tool.");
    RENDERER_START_REFUSED = "renderer_start_refused", Next::Says("Tell the user macOS refused to start RectoPDF's renderer. A security tool, or a sandbox `recto` runs in, may be blocking it.");
    REPLY_TOO_LARGE = "reply_too_large", Next::ByExit;
    REWRITE_NOT_ALLOWED = "rewrite_not_allowed", Next::ByExit;
    SAME_TAB_TWICE = "same_tab_twice", Next::ByExit;
    SAME_VERSION = "same_version", Next::ByExit;
    SAVE_FAILED = "save_failed", Next::ByExit;
    SAVING = "saving", Next::ByExit;
    SERVE_FAILED = "serve_failed", Next::ByExit;
    SESSION_ENDED = "session_ended", Next::ByExit;
    SESSION_REFUSED = "session_refused", Next::ByExit;
    SIGN_REFUSED = "sign_refused", Next::Says("Tell the user. Only they can sign, in the app.");
    SIGNATURE = "signature", Next::ByExit;
    SIGNED_NEEDS_COPY = "signed_needs_copy", Next::ByExit;
    SIGNED_ORIGINAL = "signed_original", Next::ByExit;
    SKILL_CHANGED = "skill_changed", Next::ByExit;
    SOCKET_REFUSED = "socket_refused", Next::ByExit;
    SOURCE_CHANGED = "source_changed", Next::Says("Tell the user this file changed. `recto edits` shows what's unsaved. `recto discard` throws the edits away, which needs Full access if some are the user's, and opening the file again starts from what's there now.");
    STATE_REFUSED = "state_refused", Next::ByExit;
    STILL_BUSY = "still_busy", Next::ByExit;
    TARGET_EXISTS = "target_exists", Next::ByExit;
    TIMED_OUT = "timed_out", Next::ByExit;
    TOO_COMPLEX = "too_complex", Next::ByExit;
    TOO_FEW_PANES = "too_few_panes", Next::ByExit;
    TOO_LARGE = "too_large", Next::ByExit;
    TOO_LARGE_AT_THIS_SCALE = "too_large_at_this_scale", Next::Says("Try a smaller `--scale`. 2 is twice the page's size, and 1 is its own.");
    TOO_MANY_PAGES = "too_many_pages", Next::Says("Add less to this document.");
    TOO_MANY_PANES = "too_many_panes", Next::ByExit;
    TOO_MANY_SOURCES = "too_many_sources", Next::Says("Add less to this document.");
    TOO_MANY_TABS = "too_many_tabs", Next::Says("`recto close` closes a tab.");
    TOOL_MISSING = "tool_missing", Next::ByExit;
    TOOL_SLOW = "tool_slow", Next::ByExit;
    TRANSLOCATED = "translocated", Next::ByExit;
    UI_NOT_ANSWERING = "ui_not_answering", Next::ByExit;
    UNPACK_FAILED = "unpack_failed", Next::ByExit;
    UNPACK_SLOW = "unpack_slow", Next::ByExit;
    UNSAVED_CHANGES = "unsaved_changes", Next::Says("`recto discard` throws them away, then `recto close` or `recto stop` can go on. It needs Full access only if some are the user's. `recto save` puts them in the file instead, if the user wants that.");
    UNSUPPORTED_ENCRYPTION = "unsupported_encryption", Next::ByExit;
    UPDATE_UNDER_WAY = "update_under_way", Next::ByExit;
    USAGE = "usage", Next::ByExit;
    WEB_LINK_NEEDS_USER = "web_link_needs_user", Next::ByExit;
    WOULD_CLEAR_REDO = "would_clear_redo", Next::ByExit;
    WOULD_DISCARD_USER_EDITS = "would_discard_user_edits", Next::ByExit;
    WOULD_START_HEAVY_RENDERER = "would_start_heavy_renderer", Next::ByExit;
    WOULD_START_RENDERER = "would_start_renderer", Next::ByExit;
    WRITE_FAILED = "write_failed", Next::ByExit;
    WRONG_APP = "wrong_app", Next::ByExit;
    WRONG_VERSION = "wrong_version", Next::Says("Stop the old session with `recto stop` and start this version with `recto serve --background`.");
    ZIP_ACCESS_SLOW = "zip_access_slow", Next::ByExit;
    ZIP_BAD = "zip_bad", Next::ByExit;
    ZIP_GONE = "zip_gone", Next::ByExit;
    ZIP_MISSING = "zip_missing", Next::ByExit;
    ZIP_NO_ACCESS = "zip_no_access", Next::ByExit;
    ZIP_NOT_A_FILE = "zip_not_a_file", Next::ByExit;
    ZIP_NOT_YOURS = "zip_not_yours", Next::ByExit;
    ZIP_TOO_LARGE = "zip_too_large", Next::ByExit;
    ZIP_UNREADABLE = "zip_unreadable", Next::ByExit;
    ZOOM_NEEDS_APP = "zoom_needs_app", Next::ByExit;
}
