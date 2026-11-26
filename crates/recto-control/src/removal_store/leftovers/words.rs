//! The words of Clear leftovers, decided here and nowhere else (rule 2): what the check says will happen, what it won't touch and why, what a refusal means,
//! and what a clear did. Pure: counts and cleaned labels in, a sentence out.
//!
//! The owner's decisions of 2026-11-26 are the wording: an unfinished save goes to the Trash after a confirmation ("empty the Trash to remove it for good"), the
//! session copies are deleted whole with the cost said ("that version may open without your tabs"), a save still being written is never touched, and a volume
//! with no Trash leaves the save where it is. Three rules for what is *not* said:
//! - **"in this folder", never "this document's".** A crashed save is found by its exact shape beside a document, and the shape can't say whose it is.
//! - **Nothing says everything was cleared, or that nothing is left.** The history gate is not asked for this scope, so a folder no one listed is simply not
//!   searched: "Cleared:" lists what was done, and the nothing-to-clear line says nothing was found.
//! - **A name is the person's.** File names come from outside, so an agent's words carry none: the caller gives [`Names`] only to the person, and an agent
//!   reads the same sentences with "this document" and "this folder".

use super::act::Partly;
use super::leases::Busy;
use recto_core::removal::{ConfirmRefusal, Refusal, Store};

/// The cleaned names the words may use. `None` for an agent: it gets the same words without them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Names {
    pub document: Option<String>,
    pub folder: Option<String>,
}

/// What a check found, as counts. Every count is of artifacts or of leftovers the clear leaves, never of names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// Unfinished saves in the document's own folder that may go to the Trash.
    pub saves_here: u32,
    /// Unfinished saves in other folders that were searched (a Save As destination, a folder an earlier version of the file was in).
    pub saves_elsewhere: u32,
    /// How many other folders those are in.
    pub other_folders: u32,
    /// Copies of the session record kept for going back: the update's and the older versions'.
    pub copies: u32,
    /// Unfinished saves whose process may still be running, in the document's own folder.
    pub waiting_here: u32,
    /// The same in other folders that were searched, and how many folders those are in.
    pub waiting_elsewhere: u32,
    pub waiting_folders: u32,
    /// Unfinished saves on a volume with no Trash.
    pub no_trash: u32,
}

impl Found {
    /// Whether there is anything this clear would do.
    pub fn is_empty(&self) -> bool {
        self.saves_here == 0 && self.saves_elsewhere == 0 && self.copies == 0
    }
}

/// The button the window's sheet would have, and the one `recto` names in its answer.
pub const BUTTON: &str = "Move to Trash and Delete";

fn n(count: u32, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn quoted(name: &Option<String>, otherwise: &str) -> String {
    name.as_deref().map_or_else(|| otherwise.to_owned(), |name| format!("“{name}”"))
}

/// What a check says. One line a thing, the first line the reason, the last the lines of what is left alone.
pub fn check(names: &Names, found: &Found) -> String {
    if found.is_empty() {
        return nothing(found);
    }
    let mut lines = vec![format!(
        "Before RectoPDF can remove anything from {}, it has to clear what it left behind:",
        quoted(&names.document, "this document")
    )];
    if found.saves_here > 0 {
        let one = found.saves_here == 1;
        lines.push(format!(
            "{} beside {} in {}. {} may hold an earlier copy of a document. {} to the Trash; empty the Trash to remove {} for good.",
            n(found.saves_here, "unfinished save", "unfinished saves"),
            quoted(&names.document, "this document"),
            quoted(&names.folder, "this folder"),
            if one { "It" } else { "They" },
            if one { "It goes" } else { "They go" },
            if one { "it" } else { "them" },
        ));
    }
    if found.saves_elsewhere > 0 {
        let one = found.saves_elsewhere == 1;
        lines.push(format!(
            "{} in {}. {} to the Trash too.",
            n(found.saves_elsewhere, "unfinished save", "unfinished saves"),
            n(found.other_folders.max(1), "other folder", "other folders"),
            if one { "It goes" } else { "They go" },
        ));
    }
    if found.copies > 0 {
        lines.push(format!(
            "{} of your session kept for going back to an earlier version: deleted. If an update later goes back, that version may open without your tabs.",
            n(found.copies, "copy", "copies"),
        ));
    }
    lines.extend(left_alone(found));
    lines.join("\n")
}

/// The lines for what a clear leaves where it is.
fn left_alone(found: &Found) -> Vec<String> {
    let mut lines = Vec::new();
    for (count, place) in [
        (found.waiting_here, "in this folder".to_owned()),
        (
            found.waiting_elsewhere,
            format!("in {}", n(found.waiting_folders.max(1), "other folder", "other folders")),
        ),
    ] {
        if count > 0 {
            lines.push(format!(
                "{} still being written {place}. Try again when {} finished.",
                if count == 1 { "1 save is".to_owned() } else { format!("{count} saves are") },
                if count == 1 { "it has" } else { "they have" },
            ));
        }
    }
    if found.no_trash > 0 {
        lines.push(format!(
            "{} on a volume with no Trash, so {} stay{}. You can delete {} in Finder.",
            n(found.no_trash, "unfinished save is", "unfinished saves are"),
            if found.no_trash == 1 { "it" } else { "they" },
            if found.no_trash == 1 { "s" } else { "" },
            if found.no_trash == 1 { "it" } else { "them" },
        ));
    }
    lines
}

/// When there is nothing to do. It says what was found, never that nothing is left.
pub fn nothing(found: &Found) -> String {
    let mut lines = vec!["Nothing found to clear.".to_owned()];
    lines.extend(left_alone(found));
    lines.join("\n")
}

/// The work of an earlier version that hasn't been brought in yet: a copy holding it is never deleted.
pub const WENT_BACK: &str = "Work from an earlier version hasn't been brought in yet. Quit and reopen RectoPDF, then try again. Nothing was cleared.";

/// Why no plan was made. `went_back`: a copy holds work of an earlier version.
pub fn refusal(why: Refusal, went_back: bool) -> String {
    match why {
        Refusal::Incomplete(Store::LegacySession) if went_back => WENT_BACK.to_owned(),
        Refusal::TooLarge => "There is more here than RectoPDF clears in one go, so nothing was cleared.".to_owned(),
        _ => "RectoPDF couldn't look at everything it left behind, so nothing was cleared.".to_owned(),
    }
}

/// A confirmation that doesn't hold. Every one ends "Nothing was cleared."
pub fn confirm_refusal(why: ConfirmRefusal) -> &'static str {
    match why {
        ConfirmRefusal::Expired => "That was more than 10 minutes ago. Look again. Nothing was cleared.",
        ConfirmRefusal::Unknown => "RectoPDF isn't holding a plan with that number. Look again. Nothing was cleared.",
        ConfirmRefusal::OtherTarget => "That plan was made for another document. Look again. Nothing was cleared.",
        ConfirmRefusal::Stale | ConfirmRefusal::PlanChanged => "Something changed since you looked. Look again. Nothing was cleared.",
        ConfirmRefusal::Full | ConfirmRefusal::Collision | ConfirmRefusal::Mismatched => "RectoPDF couldn't keep that plan. Look again. Nothing was cleared.",
    }
}

/// A lease that wasn't had.
pub fn busy(why: Busy) -> &'static str {
    match why {
        Busy::Installer => "An update is being installed. Try again in a moment. Nothing was cleared.",
        Busy::Windowless => "A `recto session` is running. Try again in a moment. Nothing was cleared.",
        Busy::Starting => "RectoPDF is still tidying up after starting. Try again in a moment. Nothing was cleared.",
        Busy::BesideNewer => "A newer version of RectoPDF has been used on this Mac. Clear leftovers from that version. Nothing was cleared.",
    }
}

/// A check or a clear while another clear runs.
pub const CLEARING: &str = "RectoPDF is clearing leftovers right now. Try again in a moment.";

/// What a clear did, as counts of what was moved or deleted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Did {
    pub trashed: u32,
    pub copies: u32,
    /// What a stopped artifact had done to it before it stopped: whole acts aren't counted in the two above, and it isn't nothing.
    pub partly: Option<Partly>,
}

impl Did {
    pub fn is_none(&self) -> bool {
        self.trashed == 0 && self.copies == 0 && self.partly.is_none()
    }

    fn list(&self) -> String {
        let mut parts = Vec::new();
        if self.trashed > 0 {
            parts.push(format!("{} moved to the Trash", n(self.trashed, "unfinished save", "unfinished saves")));
        }
        if self.copies > 0 {
            parts.push(format!("{} deleted", n(self.copies, "session copy", "session copies")));
        }
        match self.partly {
            Some(Partly::Copy) => parts.push("1 session copy partly deleted".to_owned()),
            Some(Partly::Save) => parts.push("1 unfinished save that may be in the Trash".to_owned()),
            None => {}
        }
        parts.join(", ")
    }
}

/// The whole plan went through.
pub fn done(did: &Did) -> String {
    if did.is_none() {
        return "Nothing found to clear.".to_owned();
    }
    format!("Cleared: {}.", did.list())
}

/// The walk stopped at step `at` of `of`, for `why` (a clause, never a name). It never says "nothing was changed" once an act has run, and when none has it says
/// so.
pub fn stopped(did: &Did, at: u32, of: u32, why: &str) -> String {
    if did.is_none() {
        return format!("Stopped: {why}. Nothing was cleared. Look again to see what's there.");
    }
    format!(
        "Stopped after {at} of {of}: {why}. Cleared so far: {}. Look again to see what's left.",
        did.list()
    )
}

#[cfg(test)]
mod tests;
