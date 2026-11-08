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

use crate::{AgentError, Event, Level, SessionAccess, edits_event, exit, file_label, select};
use recto_core::save::{AddedFile, PageRef, SaveError, SaveHooks, SaveOutcome, SaveRequest, SaveStep, Target, Writer};
use recto_platform::FileStamp;
use recto_session::TabId;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// What a host gives a save beyond the session: where backups go, and how to get the session's note of a save onto the disk.
pub trait SaveSupport: Sync {
    /// The folder for the one backup of each original. `None` keeps none.
    fn backups(&self) -> Option<PathBuf>;
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

/// Lets the tab go however the save ends, a panic included.
struct Ending<'a, A: SessionAccess> {
    access: &'a A,
    id: TabId,
}

impl<A: SessionAccess> Drop for Ending<'_, A> {
    fn drop(&mut self) {
        self.access.with(|s| s.end_save(self.id));
    }
}

fn from_save(error: &SaveError) -> AgentError {
    let exit = match error {
        SaveError::Protected | SaveError::AddedFileProtected { .. } => exit::PROTECTED,
        _ => exit::FAILED,
    };
    let next = match error {
        SaveError::FileChanged => "Tell the user the file changed on disk. `recto save-as --out <new.pdf>` keeps both versions.",
        SaveError::ReadOnly | SaveError::HardLinks => "`recto save-as --out <new.pdf>` saves a separate copy.",
        SaveError::BreaksCertification | SaveError::LockedBySignature | SaveError::SignedNeedsCopy => "Tell the user. The edits stay unsaved, and the signed file is untouched. `recto save-as --out <new.pdf>` makes an unsigned copy, if the user wants one.",
        SaveError::TargetExists => "Choose a new name, or ask the user whether to replace that file: `--replace` does, at Full.",
        SaveError::NoPagesLeft => "Keep at least one of the file's own pages, or tell the user. The edits stay unsaved, and nothing is lost.",
        _ => "Tell the user what failed. The edits are still here, unsaved, and the file is as it was.",
    };
    AgentError::new(error.code(), error.message(), exit).then(next)
}

fn needs_full(what: &str) -> AgentError {
    let mut error = AgentError::new("permission_denied", format!("{what} needs Full access."), exit::PERMISSION);
    error.next = "Run `recto permission request full`, which tells the user what to allow. Nothing else raises this.".to_string();
    error.ask = Some("recto permission request full".into());
    error
}

fn bad_path(message: &str) -> AgentError {
    AgentError::new("bad_save_path", message, exit::USAGE).then("Choose a name ending in .pdf, in a folder that isn't hidden or in ~/Library.")
}

/// **Where a Save As may write** (review): a PDF's bytes are a hostile file's, so they never go where something reads them as anything else. The name ends
/// in `.pdf`; no part of the path, as given or as it leads, is hidden (a dotfile like `~/.zshenv` is run by a shell); and nothing goes in `~/Library`,
/// where the system and apps keep files they act on.
pub fn check_save_path(out: &Path) -> Result<(), AgentError> {
    if !out.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("pdf")) {
        return Err(bad_path("A saved PDF's name has to end in .pdf."));
    }
    // Where it really goes: through its folder's links, and through itself if it's there already, as a replaced link leads somewhere else (review).
    let real = if std::fs::symlink_metadata(out).is_ok() {
        out.canonicalize().ok()
    } else {
        out.parent().and_then(|folder| folder.canonicalize().ok()).map(|folder| folder.join(out.file_name().unwrap_or_default()))
    };
    if real.as_deref().is_some_and(|real| !real.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))) {
        return Err(bad_path("That name leads to a file that isn't a .pdf, which a PDF isn't saved over."));
    }
    let hidden = |path: &Path| path.components().any(|part| matches!(part, std::path::Component::Normal(name) if name.to_string_lossy().starts_with('.')));
    if hidden(out) || real.as_deref().is_some_and(hidden) {
        return Err(bad_path("That's a hidden file or folder, which a PDF isn't saved to."));
    }
    if let Some(home) = recto_platform::local::real_home() {
        let library = home.join("Library");
        let library_real = library.canonicalize().unwrap_or_else(|_| library.clone());
        if out.starts_with(&library) || real.as_deref().is_some_and(|real| real.starts_with(&library_real)) {
            return Err(bad_path("That's inside ~/Library, where the system and apps keep files of their own, so a PDF isn't saved there."));
        }
    }
    Ok(())
}

/// **Saves a tab.** The one way an agent, the CLI or the app saves.
pub fn save<A: SessionAccess>(access: &A, level: Level, tab: &str, to: &SaveTo, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let mut to = to.clone();
    if let SaveTo::As { out, replace } = &to {
        crate::require_absolute(out)?;
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
            let own = s.own_stamp(id).zip(there).is_some_and(|(mine, theirs)| mine.inode == theirs.inode && mine.device == theirs.device);
            let elsewhere = there.is_some() && s.tab_for_file(out, there).is_some_and(|other| other != id);
            Ok::<_, AgentError>((own, elsewhere))
        })?;
        if elsewhere {
            return Err(AgentError::new("open_in_another_tab", "That file is open in another tab, so it isn't replaced. A PDF can only be open once.", exit::FAILED).then("Close that tab first, or choose another name."));
        }
        if own {
            to = SaveTo::Same;
        }
    }
    let to = &to;
    let (id, snapshot) = access.with(|s| {
        let id = select(s, tab)?;
        let snapshot = s.begin_save(id, matches!(to, SaveTo::As { .. }))?;
        Ok::<_, AgentError>((id, snapshot))
    })?;
    let Some(snapshot) = snapshot else {
        return Ok(access.with(|s| nothing_to_save(s, id)));
    };
    let ending = Ending { access, id };
    let pages: Vec<PageRef> = snapshot.slots.iter().map(|slot| PageRef { source: slot.source, page: slot.page, quarter_turns: slot.quarter_turns }).collect();
    let writer = recto_core::save::writer_for(snapshot.file_pages, &pages);
    if writer == Writer::Rewrite && level < Level::Full && level >= Level::Control {
        drop(ending);
        return Err(needs_full("A save that removes pages for good"));
    }
    let added: Vec<AddedFile<'_>> = snapshot.added.iter().map(|added| AddedFile { path: &added.path, page_count: added.page_count, stamp: added.stamp }).collect();
    let request = SaveRequest { path: &snapshot.path, stamp: snapshot.stamp, file_pages: snapshot.file_pages, pages: &pages, added: &added };
    let target = match to {
        SaveTo::Same => Target::Same,
        SaveTo::As { out, replace: false } => Target::NewFile(out),
        SaveTo::As { out, replace: true } => Target::Replace(out),
    };
    let support = access.saving();
    let backups = support.and_then(SaveSupport::backups);
    let mut hooks = Hooks { access, id, support, original: &snapshot.path, needed: if snapshot.needs_own_pages { access.with(|s| s.own_pages_needed(id)) } else { Vec::new() }, kept: None };
    let saved = recto_core::save::save(&request, target, backups.as_deref(), &mut hooks);
    let kept = hooks.kept.take();
    let outcome = match saved {
        Ok(outcome) => outcome,
        Err(error) => {
            drop(ending);
            return Err(from_save(&error));
        }
    };
    let saved_as = match to {
        SaveTo::Same => None,
        SaveTo::As { out, .. } => Some(out.clone()),
    };
    let data = access.with(|s| {
        let stamp = outcome.stamp.or_else(|| recto_platform::file_stamp(saved_as.as_deref().unwrap_or(&snapshot.path)).ok());
        let rolled_off = match stamp {
            Some(now) => s.saved(id, now, outcome.pages, saved_as.as_deref(), kept),
            None => 0,
        };
        s.end_save(id);
        events.push(Event::new("saved", json!({ "tab": id.0, "writer": outcome.writer.name() })));
        events.push(edits_event(s, id));
        answer(s, id, &outcome, rolled_off, saved_as.as_deref(), level)
    });
    drop(ending);
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
}

impl<A: SessionAccess> SaveHooks for Hooks<'_, A> {
    fn step(&mut self, step: SaveStep) {
        if let Some(support) = self.support {
            support.step(self.id, step);
        }
    }

    fn prepared(&mut self, _written: &Path, stamp: FileStamp) -> std::io::Result<()> {
        // Only the pages the history can put back are kept, from the file while it's still there, and noted with the save, so a crash after the move carries them over.
        // A redaction's save will keep nothing, as a copy would hold what it removes: there's no redaction step yet.
        self.kept = if self.needed.is_empty() { None } else { self.support.and_then(|support| support.keep_pages(self.id, self.original, stamp, &self.needed)) };
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
fn answer<L: recto_session::DocLoader>(s: &recto_session::Session<L>, id: TabId, outcome: &SaveOutcome, rolled_off: usize, saved_as: Option<&Path>, level: Level) -> Value {
    let mut data = crate::edit_state_json(s, id);
    let name = s.path_of(id).map(|path| file_label(&path)).unwrap_or_default();
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
        let (gone, them) = if one { ("The deleted page is gone", "it") } else { ("The deleted pages are gone", "them") };
        format!("{gone} from the file, but it still has things that belong to the whole document, which can repeat what was on {them}: {}.", list_words(&kept_words))
    });
    if outcome.pages_removed > 0 && outcome.resources_left_whole > 0 {
        notes.push(if one { "Some of what the deleted page drew with is shared with pages that stayed, in a way that couldn't be checked, so it was left in the file." } else { "Some of what the deleted pages drew with is shared with pages that stayed, in a way that couldn't be checked, so it was left in the file." });
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
        Some(kept) => json!({ "embedded_files": kept.embedded_files, "metadata_streams": kept.metadata_streams, "info_entries": kept.info_entries, "javascript": kept.javascript, "piece_info": kept.piece_info, "document_parts": kept.document_parts, "xfa": kept.xfa, "hidden_layers": kept.hidden_layers }),
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
