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
        SaveError::BreaksCertification | SaveError::UnsignedCopyNeedsRewrite => "Tell the user. The edits stay unsaved, and the signed file is untouched.",
        SaveError::TargetExists => "Choose a new name, or ask the user whether to replace that file: `--replace` does, at Full.",
        SaveError::NeedsRewrite => "Tell the user this isn't in this version yet. The edits stay unsaved, and nothing is lost.",
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

/// **Saves a tab.** The one way an agent, the CLI or the app saves.
pub fn save<A: SessionAccess>(access: &A, level: Level, tab: &str, to: &SaveTo, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    if let SaveTo::As { out, replace } = to {
        crate::require_absolute(out)?;
        if *replace && level < Level::Full {
            return Err(needs_full("Replacing another file"));
        }
        if !*replace && std::fs::symlink_metadata(out).is_ok() {
            return Err(from_save(&SaveError::TargetExists));
        }
    }
    let (id, snapshot) = access.with(|s| {
        let id = select(s, tab)?;
        let snapshot = s.begin_save(id)?;
        Ok::<_, AgentError>((id, snapshot))
    })?;
    let Some(snapshot) = snapshot else {
        return match to {
            SaveTo::Same => Ok(access.with(|s| nothing_to_save(s, id))),
            SaveTo::As { out, replace } => copy_as_is(access, id, out, *replace, events),
        };
    };
    let ending = Ending { access, id };
    let pages: Vec<PageRef> = snapshot.slots.iter().map(|slot| PageRef { source: slot.source, page: slot.page, quarter_turns: slot.quarter_turns }).collect();
    let writer = recto_core::save::writer_for(snapshot.file_pages, &pages);
    if writer == Writer::Rewrite && level < Level::Full && level >= Level::Control {
        // Checked again by the writer, which is what refuses it in this build.
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
    let mut hooks = Hooks { access, id, support };
    let saved = recto_core::save::save(&request, target, backups.as_deref(), &mut hooks);
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
            Some(now) => s.saved(id, now, outcome.pages, saved_as.as_deref(), None),
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
}

impl<A: SessionAccess> SaveHooks for Hooks<'_, A> {
    fn step(&mut self, step: SaveStep) {
        if let Some(support) = self.support {
            support.step(self.id, step);
        }
    }

    fn prepared(&mut self, _written: &Path, stamp: FileStamp) -> std::io::Result<()> {
        // A save that removes something for good keeps nothing, and an appended one needs nothing kept: every page it had is still in the file.
        self.access.with(|s| s.save_prepared(self.id, stamp, None));
        match self.support {
            Some(support) => support.note_on_disk(self.id, stamp).map_err(std::io::Error::other),
            None => Ok(()),
        }
    }
}

fn nothing_to_save<L: recto_session::DocLoader>(s: &recto_session::Session<L>, id: TabId) -> Value {
    let mut data = crate::edit_state_json(s, id);
    data["did"] = json!("There's nothing unsaved, so nothing was written.");
    data["written"] = json!(false);
    data
}

/// A Save As of a document with nothing unsaved: a copy of the file as it is, byte for byte, as a Mac app's Save As makes. It's written beside the target and linked into place, so it never replaces anything unasked. The tab goes over to the copy.
fn copy_as_is<A: SessionAccess>(access: &A, id: TabId, out: &Path, replace: bool, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let path = access.with(|s| crate::tab_path(s, id))?;
    let folder = out.parent().ok_or_else(|| from_save(&SaveError::Failed))?;
    let partial = folder.join(format!(".{}.recto-copy", std::process::id()));
    let _ = std::fs::remove_file(&partial);
    let copied = (|| -> std::io::Result<()> {
        if recto_platform::local::clone_file(&path, &partial).is_err() {
            std::fs::copy(&path, &partial)?;
        }
        recto_platform::local::carry_origin(&path, &partial)?;
        std::fs::File::open(&partial)?.sync_all()?;
        if replace {
            std::fs::rename(&partial, out)?;
        } else {
            std::fs::hard_link(&partial, out)?;
            std::fs::remove_file(&partial)?;
        }
        recto_platform::local::sync_folder_of(out)
    })();
    if let Err(error) = copied {
        let _ = std::fs::remove_file(&partial);
        return Err(from_save(&match error.kind() {
            std::io::ErrorKind::AlreadyExists => SaveError::TargetExists,
            std::io::ErrorKind::PermissionDenied => SaveError::ReadOnly,
            _ => SaveError::Failed,
        }));
    }
    let now = recto_platform::file_stamp(out).map_err(|_| from_save(&SaveError::Failed))?;
    Ok(access.with(|s| {
        let pages = s.pages_in(id);
        s.saved(id, now, pages, Some(out), None);
        events.push(Event::new("saved", json!({ "tab": id.0, "writer": "copy" })));
        let mut data = crate::edit_state_json(s, id);
        data["did"] = json!(format!("Saved a copy as “{}”. This tab shows the copy now.", file_label(out)));
        data["written"] = json!(true);
        data["writer"] = json!("copy");
        data["untrusted"] = json!({ "file": file_label(out) });
        data
    }))
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
    if outcome.objects_missing > 0 {
        notes.push("Part of an added page named something its file doesn't have, so it was left out, as other readers leave it out. Check the added pages look right.");
    }
    if outcome.durability_warning {
        notes.push("The file was saved, but the disk didn't confirm it, so a power cut in the next moments could bring back the old version.");
    }
    if rolled_off > 0 {
        notes.push("Some undo steps from before this save couldn't be kept, so undo stops short of them.");
    }
    data["notes"] = json!(notes);
    // Where the backup is says something about the user's disk, so only at Full.
    data["backup_kept"] = json!(outcome.backup.is_some());
    if level >= Level::Full {
        data["backup"] = json!(outcome.backup.as_ref().map(|path| path.display().to_string()));
    }
    data["untrusted"] = json!({ "file": name });
    data
}
