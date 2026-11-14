//! **Save and Save As, for agents and the app alike.** `recto save` and `recto save-as` run this, and so do the app's ⌘S and Save As sheet, through
//! the same command: the writing is `recto_core::save`'s, and around it this does what the session needs, in this order (`docs/ENGINEERING.md`,
//! "The steps of a save"):
//!
//! 1. `Session::begin_save` copies out what's to be written and holds the tab, so nothing changes its edits while the file is written. An
//!    edit made then came back reading as saved with its rotation not in the file (review).
//! 2. The file is written beside its target and read back. Before it's moved into place, `Session::save_prepared` notes what it will be, and the
//!    host writes that to the journal and reads it back: a crash after the move then finds the journal saying so, and carries the history over.
//! 3. It's moved into place, and `Session::saved` marks the saved point. Whatever happened, `Session::end_save` lets the tab go.
//!
//! **Levels** (SPEC decision 20 and section 5; the coordinator's decision, 2026-11-08, until the owner says otherwise). At Control an agent may
//! save what's appended, as every byte that was in the file stays in it, the history keeps the saved point to undo past, and the first save keeps
//! a backup of the original: nothing is lost. It may Save As to a new file, which changes nothing that exists. Replacing another file, and a save
//! that rewrites the file, which takes what was removed out of it for good, need Full. Read-only and Off can't save.

use crate::Code;
use crate::TabRef;
use crate::{AgentError, Event, Level, SessionAccess, exit, file_label, select};
use recto_core::save::{AddedFile, PageRef, SaveError, SaveHooks, SaveOutcome, SaveRequest, SaveStep, Target, Writer};
use recto_platform::FileStamp;
use recto_session::TabId;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// What a host gives a save beyond the session: where backups go, and how to get the session's note of a save onto the disk.
pub trait SaveSupport: Sync {
    /// The folder for the one backup of each original. `None` keeps none.
    fn backups(&self) -> Option<PathBuf>;
    /// **A save is about to start writing:** what the session has waiting for its journals goes now. Edits wait up to 5 s for the journal (SPEC
    /// decision 25), and a save of a big file takes seconds, so a crash during it would otherwise lose the very edits being saved. A host with no
    /// journals has nothing to write.
    fn before_writing(&self, _tab: TabId) {}
    /// Writes what the session has waiting for its journals now, and reads back that the save's note is there, before the file's moved into place. A host with no journals has nothing to write.
    fn note_on_disk(&self, _tab: TabId, _target: FileStamp) -> Result<(), String> {
        Ok(())
    }
    /// The save's done and confirmed. The record can follow it now, so a Save As isn't left with the record naming the old file.
    fn after_save(&self, _tab: TabId) {}
    /// **Keeps these pages of `original`, ascending, for undoing past a save that replaces it with `target`**: only the pages the history can put back (owner, 2026-11-07). `None` keeps nothing, and those steps roll off. A host with no history folder keeps none.
    fn keep_pages(&self, _tab: TabId, _original: &Path, _target: FileStamp, _pages: &[u32]) -> Option<recto_session::edit::AddedSource> {
        None
    }
    /// How a save is going, for progress on a big file.
    fn step(&self, _tab: TabId, _step: SaveStep) {}
}

/// Where a save goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveTo {
    /// Over the file itself.
    Same,
    /// Another file: a new one, or one the person chose to replace.
    As { out: PathBuf, replace: bool },
}

/// Lets the tab go however the save ends, a panic included, **once**: unless the save's own end has run (`ended`), in the same hold of the session as
/// what it saved. Ending again, under a hold of its own, would end a save of the same tab begun in between, and let edits through while its file is
/// written (review, 2026-11-13).
struct Ending<'a, A: SessionAccess> {
    access: &'a A,
    id: TabId,
    ended: std::cell::Cell<bool>,
}

impl<'a, A: SessionAccess> Ending<'a, A> {
    fn new(access: &'a A, id: TabId) -> Self {
        Self {
            access,
            id,
            ended: std::cell::Cell::new(false),
        }
    }
}

impl<A: SessionAccess> Drop for Ending<'_, A> {
    fn drop(&mut self) {
        if !self.ended.get() {
            self.access.with(|s| s.end_save(self.id));
        }
    }
}

fn from_save(error: &SaveError) -> AgentError {
    let exit = match error {
        SaveError::Protected { .. } | SaveError::AddedFileProtected { .. } => exit::PROTECTED,
        _ => exit::FAILED,
    };
    let next = match error {
        SaveError::FileChanged => crate::unfit::CHANGED_UNDER_EDITS_NEXT,
        SaveError::ChangedSinceOpened => "Tell the user the file changed on disk. `recto save-as --out <new.pdf>` keeps both versions.",
        SaveError::ChangedWhileCopying => {
            "Tell the user the file changed on disk while it was being copied. Nothing was written. Run the same `recto save-as` again to copy it as it is now."
        }
        SaveError::Unreadable => {
            "Tell the user the file couldn't be read during the save. Nothing was written, and any unsaved edits are still here. Try again once the file can be read."
        }
        SaveError::ReadOnly | SaveError::HardLinks => "`recto save-as --out <new.pdf>` saves a separate copy.",
        SaveError::BreaksCertification | SaveError::LockedBySignature | SaveError::SignedNeedsCopy => {
            "Tell the user. The edits stay unsaved, and the signed file is untouched. `recto save-as --out <new.pdf>` makes an unsigned copy, if the user wants one."
        }
        SaveError::TargetExists => "Choose a new name, or ask the user whether to replace that file: `--replace` does, at Full.",
        SaveError::NoPagesLeft => "Keep at least one of the file's own pages, or tell the user. The edits stay unsaved, and nothing is lost.",
        _ => "Tell the user what failed. The edits are still here, unsaved, and the file is as it was.",
    };
    AgentError::new(Code::lower(error.code()), error.message(), exit).then(next)
}

fn needs_full(what: &str) -> AgentError {
    let mut error = AgentError::new(Code::PERMISSION_DENIED, format!("{what} needs Full access."), exit::PERMISSION);
    error.next = "Run `recto permission request full`, which tells the user what to allow. Nothing else raises this.".to_string();
    error.ask = Some("recto permission request full".into());
    error
}

fn bad_path(message: &str) -> AgentError {
    AgentError::new(Code::BAD_SAVE_PATH, message, exit::USAGE).then("Choose a name ending in .pdf, in a folder that isn't hidden or in ~/Library.")
}

/// What's wrong with a place to write a file, by the rules every write path keeps (review of Save): the name has the right ending; no part of the path, as given or as it leads, is hidden (a dotfile like `~/.zshenv` is run by a shell); and nothing goes in `~/Library`, where the system and apps keep files they act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathProblem {
    Ending,
    LeadsToAnotherKind,
    Hidden,
    Library,
}

/// Which of the rules a path breaks, if any. `endings` are without their dot, and any of them will do.
pub(crate) fn path_problem(out: &Path, endings: &[&str]) -> Option<PathProblem> {
    let ends_right = |path: &Path| {
        path.extension()
            .is_some_and(|extension| endings.iter().any(|ending| extension.eq_ignore_ascii_case(ending)))
    };
    if !ends_right(out) {
        return Some(PathProblem::Ending);
    }
    // Where it really goes: through its folder's links, and through itself if it's there already, as a replaced link leads somewhere else (review).
    let real = if std::fs::symlink_metadata(out).is_ok() {
        out.canonicalize().ok()
    } else {
        out.parent()
            .and_then(|folder| folder.canonicalize().ok())
            .map(|folder| folder.join(out.file_name().unwrap_or_default()))
    };
    if real.as_deref().is_some_and(|real| !ends_right(real)) {
        return Some(PathProblem::LeadsToAnotherKind);
    }
    let hidden = |path: &Path| {
        path.components()
            .any(|part| matches!(part, std::path::Component::Normal(name) if name.to_string_lossy().starts_with('.')))
    };
    if hidden(out) || real.as_deref().is_some_and(hidden) {
        return Some(PathProblem::Hidden);
    }
    if let Some(home) = recto_platform::local::real_home() {
        let library = home.join("Library");
        let library_real = library.canonicalize().unwrap_or_else(|_| library.clone());
        if out.starts_with(&library) || real.as_deref().is_some_and(|real| real.starts_with(&library_real)) {
            return Some(PathProblem::Library);
        }
    }
    None
}

/// **Where a Save As may write** (review): a PDF's bytes are a hostile file's, so they never go where something reads them as anything else. [`path_problem`]'s rules, with `.pdf`.
pub fn check_save_path(out: &Path) -> Result<(), AgentError> {
    match path_problem(out, &["pdf"]) {
        None => Ok(()),
        Some(PathProblem::Ending) => Err(bad_path("A saved PDF's name has to end in .pdf.")),
        Some(PathProblem::LeadsToAnotherKind) => Err(bad_path("That name leads to a file that isn't a .pdf, which a PDF isn't saved over.")),
        Some(PathProblem::Hidden) => Err(bad_path("That's a hidden file or folder, which a PDF isn't saved to.")),
        Some(PathProblem::Library) => Err(bad_path(
            "That's inside ~/Library, where the system and apps keep files of their own, so a PDF isn't saved there.",
        )),
    }
}

/// **Where an export may write**: a picture, an SVG or a snapshot carries a hostile file's content too, so it keeps the same rules as a Save As, with its own endings (review of Save: every write path needs them). `kind` names it, as "picture" or "SVG"; `endings` are what it may end in, the first being what's suggested.
pub fn check_export_path(out: &Path, endings: &[&str], kind: &str) -> Result<(), AgentError> {
    let suggested = endings.first().copied().unwrap_or("");
    let bad = |message: String| {
        AgentError::new(Code::BAD_OUTPUT, message, exit::FAILED)
            .then(format!("Choose a name ending in .{suggested}, in a folder that isn't hidden or in ~/Library."))
    };
    let listed = endings.iter().map(|ending| format!(".{ending}")).collect::<Vec<_>>().join(" or ");
    match path_problem(out, endings) {
        None => Ok(()),
        Some(PathProblem::Ending) => Err(bad(format!("The output file must end in {listed}."))),
        Some(PathProblem::LeadsToAnotherKind) => Err(bad(format!(
            "That name leads to a file that doesn't end in {listed}, which a {kind} isn't written to."
        ))),
        Some(PathProblem::Hidden) => Err(bad(format!("That's a hidden file or folder, which a {kind} isn't written to."))),
        Some(PathProblem::Library) => Err(bad(format!(
            "That's inside ~/Library, where the system and apps keep files of their own, so a {kind} isn't written there."
        ))),
    }
}

/// The tab to save, unless its edits were made on another version of the file than the one there: those are saved nowhere, here or as a copy, as they'd
/// land on its pages. What's known decides ([`recto_session::Notices::fit`]); a change since the last look is the save's own to find.
fn tab_to_save<L: recto_session::DocLoader>(s: &recto_session::Session<L>, tab: &TabRef) -> Result<recto_session::TabId, AgentError> {
    let id = select(s, tab)?;
    match crate::unfit::refusal(s.notices().fit(id), crate::unfit::Asked::Save) {
        Some(refused) => Err(refused),
        None => Ok(id),
    }
}

/// **What a refused save is said to be.** The writer can't tell edits from a copy of the document as it is, nor whose file changed under it: a copy with no edits
/// isn't said to be about edits, and the tab's own file changing under a save of edits, before it or while it ran, is that file changing after they were made.
/// A target being replaced that changed is neither.
fn as_said(error: SaveError, request: &SaveRequest<'_>, to: &SaveTo) -> SaveError {
    let whole_file = request.added.is_empty()
        && usize::try_from(request.file_pages).is_ok_and(|pages| pages == request.pages.len())
        && request
            .pages
            .iter()
            .zip(0..)
            .all(|(page, at)| page.source == 0 && page.page == at && page.quarter_turns == 0);
    let copy = matches!(to, SaveTo::As { .. });
    match error {
        // A copy of the document as it is, refused because its file changed as it was copied: Save As is what was just refused. (A changed target being
        // replaced can't be told from that, and nor can a file whose version was never known, so those stay as they were.)
        SaveError::FileChanged if whole_file && copy && request.stamp.is_some() => SaveError::ChangedWhileCopying,
        SaveError::ChangedSinceOpened if whole_file && matches!(to, SaveTo::As { replace: false, .. }) => SaveError::ChangedWhileCopying,
        SaveError::FileChanged if whole_file => SaveError::ChangedSinceOpened,
        SaveError::ChangedSinceOpened if !whole_file && !matches!(to, SaveTo::As { replace: true, .. }) => SaveError::FileChanged,
        error => error,
    }
}

/// **Saves a tab.** The one way an agent, the CLI or the app saves.
pub fn save<A: SessionAccess>(access: &A, level: Level, tab: &TabRef, to: &SaveTo, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let mut to = to.clone();
    // Save As onto the tab's own file, which is a Save: a signed one's refusal then says to choose another name, not to use Save As.
    let mut onto_itself = false;
    if let SaveTo::As { out, replace } = &to {
        crate::quiet_files::check_new_file(access, out)?; // The session's own wall, beside the table's (F4).
        check_save_path(out)?;
        if *replace && level < Level::Full {
            return Err(needs_full("Replacing another file"));
        }
        if !*replace && std::fs::symlink_metadata(out).is_ok() {
            return Err(from_save(&SaveError::TargetExists));
        }
        // Over the tab's own file is a Save, with its backup and its checks. Over a file open in another tab would leave two tabs on one file (SPEC decision 26).
        let there = recto_platform::file_stamp(out).ok();
        let (own, elsewhere) = access.with(|s| {
            let id = select(s, tab)?;
            let mine = s.tabs().own_stamp(id);
            let own = mine
                .zip(there)
                .is_some_and(|(mine, theirs)| mine.inode == theirs.inode && mine.device == theirs.device);
            let elsewhere = there.is_some() && s.tab_for_file(out, there).is_some_and(|other| other != id);
            Ok::<_, AgentError>((own, elsewhere))
        })?;
        if elsewhere {
            return Err(AgentError::new(
                Code::OPEN_IN_ANOTHER_TAB,
                "That file is open in another tab, so it isn't replaced. A PDF can only be open once.",
                exit::FAILED,
            )
            .then("Close that tab first, or choose another name."));
        }
        if own {
            to = SaveTo::Same;
            onto_itself = true;
        }
    }
    let to = &to;
    // The agreement gate (SPEC section 3), before the tab's held, as a look can take seconds (review round 1); asked again below, from what this kept.
    let something = matches!(to, SaveTo::As { .. }) || access.with(|s| select(s, tab).map(|id| s.lists().is_edited(id)))?;
    if something && let Some(refusal) = crate::agreement::refusal_for(access, tab)? {
        return Err(refusal);
    }
    let (id, snapshot) = access.with(|s| {
        let id = tab_to_save(s, tab)?;
        let snapshot = s.begin_save(id, matches!(to, SaveTo::As { .. }))?;
        Ok::<_, AgentError>((id, snapshot))
    })?;
    let Some(snapshot) = snapshot else {
        return Ok(access.with(|s| nothing_to_save(s, id)));
    };
    let ending = Ending::new(access, id);
    if let Some(support) = access.saving() {
        support.before_writing(id);
    }
    // The agreement gate again, now the tab's held: anything added since is looked at, the rest is what was kept.
    if let Some(refusal) = crate::agreement::save_refusal(&snapshot) {
        drop(ending);
        return Err(refusal);
    }
    let pages: Vec<PageRef> = snapshot
        .slots
        .iter()
        .map(|slot| PageRef {
            source: slot.source,
            page: slot.page,
            quarter_turns: slot.quarter_turns,
        })
        .collect();
    let writer = recto_core::save::writer_for(snapshot.file_pages, &pages);
    if writer == Writer::Rewrite && level < Level::Full && level >= Level::Control {
        drop(ending);
        return Err(needs_full("A save that removes pages for good"));
    }
    let added: Vec<AddedFile<'_>> = snapshot
        .added
        .iter()
        .map(|added| AddedFile {
            path: &added.path,
            page_count: added.page_count,
            stamp: added.stamp,
        })
        .collect();
    let request = SaveRequest {
        path: &snapshot.path,
        stamp: snapshot.stamp,
        file_pages: snapshot.file_pages,
        pages: &pages,
        added: &added,
    };
    let target = match to {
        SaveTo::Same => Target::Same,
        SaveTo::As { out, replace: false } => Target::NewFile(out),
        SaveTo::As { out, replace: true } => Target::Replace(out),
    };
    let support = access.saving();
    let backups = support.and_then(SaveSupport::backups);
    // A Save As leaves the old file where it is, so the history needs no copy of its pages.
    let needed = if snapshot.needs_own_pages && matches!(to, SaveTo::Same) {
        access.with(|s| s.lists().own_pages_needed(id))
    } else {
        Vec::new()
    };
    let mut hooks = Hooks {
        access,
        id,
        support,
        original: &snapshot.path,
        needed,
        kept: None,
        level,
    };
    let saved = recto_core::save::save(&request, target, backups.as_deref(), &mut hooks);
    let kept = hooks.kept.take();
    let outcome = match saved {
        Ok(outcome) => outcome,
        Err(SaveError::RewriteNotAllowed) => {
            drop(ending);
            return Err(needs_full("A save that writes the file anew"));
        }
        Err(SaveError::SignedNeedsCopy) if onto_itself => {
            drop(ending);
            return Err(AgentError::new(Code::SIGNED_ORIGINAL, "This document is signed, so it isn't saved over, even by Save As onto its own name. Choose another name for the new copy, which won't be signed. Nothing was written, and the original keeps its signature.", exit::FAILED).then("Choose a new name: `recto save-as --out <new.pdf>`."));
        }
        Err(error) => {
            drop(ending);
            return Err(from_save(&as_said(error, &request, to)));
        }
    };
    let saved_as = match to {
        SaveTo::Same => None,
        SaveTo::As { out, .. } => Some(out.clone()),
    };
    let data = access.with(|s| {
        let stamp = outcome
            .stamp
            .or_else(|| recto_platform::file_stamp(saved_as.as_deref().unwrap_or(&snapshot.path)).ok());
        let rolled_off = match stamp {
            Some(now) => s.saved(id, now, outcome.pages, saved_as.as_deref(), kept),
            None => 0,
        };
        s.end_save(id);
        ending.ended.set(true);
        events.extend(crate::event::saved(s, id, outcome.writer.name()));
        answer(s, id, &outcome, rolled_off, saved_as.as_deref(), level)
    });
    if let Some(support) = support {
        support.after_save(id);
    }
    Ok(data)
}

struct Hooks<'a, A: SessionAccess> {
    access: &'a A,
    id: TabId,
    support: Option<&'a dyn SaveSupport>,
    /// The file as it is, before the save replaces it.
    original: &'a Path,
    /// The file's pages the history can put back, which a copy keeps.
    needed: Vec<u32>,
    /// That copy, once it's made.
    kept: Option<recto_session::edit::AddedSource>,
    level: Level,
}

impl<A: SessionAccess> SaveHooks for Hooks<'_, A> {
    fn may_rewrite(&self) -> bool {
        // Writing a file anew takes what was removed out of it for good: Full only (SPEC section 5). Below Control nothing saves at all.
        self.level >= Level::Full
    }

    fn step(&mut self, step: SaveStep) {
        if let Some(support) = self.support {
            support.step(self.id, step);
        }
    }

    fn prepared(&mut self, _written: &Path, stamp: FileStamp) -> std::io::Result<()> {
        // Only the pages the history can put back are kept, from the file while it's still there, and noted with the save, so a crash after the move carries them over.
        // A redaction's save will keep nothing, as a copy would hold what it removes: there's no redaction step yet.
        self.kept = if self.needed.is_empty() {
            None
        } else {
            self.support.and_then(|support| support.keep_pages(self.id, self.original, stamp, &self.needed))
        };
        self.access.with(|s| s.save_prepared(self.id, stamp, self.kept.as_ref()));
        match self.support {
            Some(support) => support.note_on_disk(self.id, stamp).map_err(std::io::Error::other),
            None => Ok(()),
        }
    }
}

/// The decided words for a signed file's copy (SPEC decision 27).
pub const UNSIGNED_COPY: &str = "The new copy won't be signed. The original keeps its signature.";

/// "a, b and c".
fn list_words(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn nothing_to_save<L: recto_session::DocLoader>(s: &recto_session::Session<L>, id: TabId) -> Value {
    let mut data = crate::edit_state_json(s, id);
    data["did"] = json!("There's nothing unsaved, so nothing was written.");
    data["written"] = json!(false);
    data
}

/// What a save answers: what it did in plain words, and the facts behind them.
fn answer<L: recto_session::DocLoader>(
    s: &recto_session::Session<L>,
    id: TabId,
    outcome: &SaveOutcome,
    rolled_off: usize,
    saved_as: Option<&Path>,
    level: Level,
) -> Value {
    let mut data = crate::edit_state_json(s, id);
    let name = s.tabs().path_of(id).map(|path| file_label(&path)).unwrap_or_default();
    let how = match outcome.writer {
        Writer::Append => "The changes were added to the end of the file, so everything that was in it is still there.",
        Writer::Rewrite => "The file was written anew, so what was removed is gone from it.",
    };
    data["did"] = json!(match saved_as {
        Some(_) => format!("Saved as “{name}”. {how} This tab shows the new file now."),
        None => format!("Saved “{name}”. {how}"),
    });
    data["written"] = json!(true);
    data["writer"] = json!(outcome.writer.name());
    data["bytes_before"] = json!(outcome.previous_len);
    data["bytes"] = json!(outcome.new_len);
    let mut notes = Vec::new();
    if outcome.usage_rights_removed {
        notes.push("Its usage rights, which let Adobe Reader fill in and sign this form, were removed, as any change to its pages ends them. They weren't a person's signature.");
    }
    if outcome.signatures_removed > 0 {
        notes.push(UNSIGNED_COPY);
    }
    if outcome.objects_missing > 0 {
        notes.push(match outcome.writer {
            Writer::Append => "Part of an added page named something its file doesn't have, so it was left out, as other readers leave it out. Check the added pages look right.",
            Writer::Rewrite => "Part of the file named something it doesn't have, so it was left out of the saved file, as most readers leave it out. Some readers can repair a damaged file and would have shown it: check the pages look right, and keep the backup if you're unsure.",
        });
    }
    let kept_words = outcome.document_data_kept.map(|kept| kept.kinds()).unwrap_or_default();
    let one = outcome.pages_removed == 1;
    let kept_note = (outcome.pages_removed > 0 && !kept_words.is_empty()).then(|| {
        let (gone, them) = if one {
            ("The deleted page is gone", "it")
        } else {
            ("The deleted pages are gone", "them")
        };
        format!(
            "{gone} from the file, but it still has things that belong to the whole document, which can repeat what was on {them}: {}.",
            list_words(&kept_words)
        )
    });
    if outcome.pages_removed > 0 && outcome.resources_left_whole > 0 {
        notes.push(if one {
            "Some of what the deleted page drew with is shared with pages that stayed, in a way that couldn't be checked, so it was left in the file."
        } else {
            "Some of what the deleted pages drew with is shared with pages that stayed, in a way that couldn't be checked, so it was left in the file."
        });
    }
    if outcome.form_fields_left_out {
        notes.push("This document's own form couldn't be read, so the form fields on the added pages weren't joined to it: they show, and can't be filled in.");
    }
    if outcome.attributes_missed {
        notes.push("Some of the file's Finder information, such as tags or who may open it, couldn't be kept on the saved file.");
    }
    if outcome.durability_warning {
        notes.push("The file was saved, but the disk didn't confirm it, so a power cut in the next moments could bring back the old version.");
    }
    if rolled_off > 0 {
        notes.push("Some undo steps from before this save couldn't be kept, so undo stops short of them.");
    }
    let mut notes: Vec<String> = notes.into_iter().map(str::to_string).collect();
    notes.extend(kept_note);
    data["notes"] = json!(notes);
    data["pages_removed"] = json!(outcome.pages_removed);
    data["signatures_removed"] = json!(outcome.signatures_removed);
    data["objects_missing"] = json!(outcome.objects_missing);
    data["document_data_kept"] = match outcome.document_data_kept {
        Some(kept) => {
            json!({ "embedded_files": kept.embedded_files, "metadata_streams": kept.metadata_streams, "info_entries": kept.info_entries, "javascript": kept.javascript, "piece_info": kept.piece_info, "document_parts": kept.document_parts, "xfa": kept.xfa, "hidden_layers": kept.hidden_layers })
        }
        None => Value::Null,
    };
    // Where the backup is says something about the user's disk, so only at Full.
    data["backup_kept"] = json!(outcome.backup.is_some());
    if level >= Level::Full {
        data["backup"] = json!(outcome.backup.as_ref().map(|path| path.display().to_string()));
    }
    data["untrusted"] = json!({ "file": name });
    data
}

#[cfg(test)]
mod ending_tests {
    use super::*;
    use recto_core::{DocFacts, OpenError, Toc};
    use recto_session::{DocLoader, Session};

    #[derive(Clone)]
    struct Fake;

    impl DocLoader for Fake {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            Ok(DocFacts {
                page_count: 3,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::default(),
                renderer_only: false,
            })
        }
    }

    /// Failure (review of 2026-11-13): a save ended twice, once with what it saved and again as its guard drops, each under its own hold of the session, so
    /// a second save of the same tab begun in between was ended by the first's guard, and edits were let through while its file was being written. A save
    /// that has ended, a second begun, then the first's guard dropped: the second is still saving.
    #[test]
    fn a_saves_guard_never_ends_the_next_save() {
        let session = std::sync::Mutex::new(Session::new(Fake));
        let id = session.with(|s| s.open(Path::new("/d/a.pdf"))).expect("open");
        session.with(|s| s.begin_save(id, true)).expect("the first save").expect("something to save");
        let ending = Ending::new(&session, id);
        session.with(|s| {
            s.end_save(id);
            ending.ended.set(true);
        });
        session.with(|s| s.begin_save(id, true)).expect("the second save").expect("something to save");
        drop(ending);
        assert!(session.with(|s| s.saves().is_saving(id)), "the second save is still under way");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (F4c, review round 1): "This file changed on disk after these edits were made ... your edits are kept" was said of a Save As with no edits (no
    /// stamp, or the file moved on since), of a source that couldn't be read or opened, and of a `--replace` target that changed, and the app stopped offering
    /// Save As for those. The edits' sentence is said only of edits whose own file changed; the rest keep the earlier words, with Save As offered.
    #[test]
    fn the_edits_sentence_is_said_only_of_edits_whose_file_changed() {
        let whole = [0, 1].map(|page| PageRef {
            source: 0,
            page,
            quarter_turns: 0,
        });
        let turned = [0, 1].map(|page| PageRef {
            source: 0,
            page,
            quarter_turns: u8::from(page == 1),
        });
        fn request(pages: &[PageRef]) -> SaveRequest<'_> {
            SaveRequest {
                path: Path::new("/d/doc.pdf"),
                stamp: Some(recto_platform::FileStamp {
                    size: 1,
                    mtime_ns: 1,
                    device: 1,
                    inode: 1,
                }),
                file_pages: 2,
                pages,
                added: &[],
            }
        }
        let (whole, turned) = (&whole[..], &turned[..]);
        let copy = SaveTo::As {
            out: "/d/copy.pdf".into(),
            replace: false,
        };
        let replace = SaveTo::As {
            out: "/d/other.pdf".into(),
            replace: true,
        };
        let cases = [
            (SaveError::FileChanged, whole, &SaveTo::Same, "changed_since_opened"),
            (SaveError::ChangedSinceOpened, whole, &SaveTo::Same, "changed_since_opened"),
            (SaveError::FileChanged, turned, &SaveTo::Same, "file_changed"),
            (SaveError::ChangedSinceOpened, turned, &SaveTo::Same, "file_changed"),
            (SaveError::ChangedSinceOpened, turned, &replace, "changed_since_opened"),
            // A Save As of the document as it is, refused because its source changed: no "Save As keeps both" for the Save As that was just refused.
            (SaveError::FileChanged, whole, &copy, "changed_while_copying"),
            (SaveError::ChangedSinceOpened, whole, &copy, "changed_while_copying"),
            (SaveError::ChangedSinceOpened, whole, &replace, "changed_since_opened"),
            // A file that couldn't be read is never said to have changed under the edits.
            (SaveError::Unreadable, turned, &SaveTo::Same, "file_unreadable"),
            (SaveError::Unreadable, whole, &copy, "file_unreadable"),
        ];
        for (error, pages, to, said) in cases {
            assert_eq!(as_said(error.clone(), &request(pages), to).code(), said, "{error:?}, {pages:?}, {to:?}");
        }
        assert!(!SaveError::ChangedSinceOpened.message().contains("edits"));
        assert!(
            !SaveError::ChangedWhileCopying.message().contains("Save As"),
            "the Save As refused isn't the way on"
        );
    }
}
