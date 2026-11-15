//! **A tab's unsaved edits, made on another version of its file than the one there now.** An edit names its page by its number in the version it was made on
//! (until the page matcher, owner 2026-11-13), so while the edits don't fit the file nothing takes that file as the edited document: not a page read or drawn
//! for an agent, a search, Find, a sleep file or a save. The fact is one, kept here beside the tab's notice and read through [`Notices::fit`]. It isn't a
//! notice, so no other notice can hide or clear it: a file gone and put back, history rolling off, or leaving the tab all leave it as it was.

use super::*;

/// How a tab's unsaved edits stand against its file, from [`Notices::fit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditsFit {
    /// No unsaved edits, or edits on the version of the file last looked at.
    Fit,
    /// The file changed on disk after they were made, and the version they were made on is known: put back, they fit again.
    FileChanged,
    /// The file changed on disk, and nothing names the version they were made on (an edit landed after a change was heard and before the file was read again), so
    /// no version put back can be known to be theirs: discarding them is the only way on.
    FileChangedVersionUnknown,
    /// Nothing says they fit this file, and it isn't known to have changed: their journal names a length it doesn't have, as a damaged or forged one can.
    NotThisFile,
    /// Which version they were made on isn't known (the file had no stamp when it was opened, and nothing's been found against them since), so no version
    /// of the file can be taken to be theirs.
    VersionUnknown,
}

impl EditsFit {
    pub fn fits(self) -> bool {
        self == Self::Fit
    }

    /// The version they were made on is known, so putting it back would make them fit.
    pub fn earlier_version_known(self) -> bool {
        self == Self::FileChanged
    }

    /// **The file on disk is known to be another than the edits are about** (the coordinator's decisions of 2026-11-15, from the QA pass before 0.1.1):
    /// it changed under them, whether or not the version they were made on is known, or their journal names a length it hasn't. **The one rule** for what
    /// follows from that: the tab's own pages are withheld ([`Self::shows_own_pages`]), no more edits are taken (`Session::edit_pages_as`), and its words
    /// say both ([`Self::said`]). Not where only the version the edits were made on isn't known (`VersionUnknown`): nothing has been found against the
    /// file, so the tab shows it and takes edits as before, while Save and an agent's reads stay refused, as for every state that doesn't fit.
    pub fn file_known_other(self) -> bool {
        match self {
            Self::FileChanged | Self::FileChangedVersionUnknown | Self::NotThisFile => true,
            Self::Fit | Self::VersionUnknown => false,
        }
    }

    /// **What a new edit refused is told,** by the one rule ([`Self::file_known_other`]), or nothing where an edit is taken: the one place for these
    /// words, which the session's error and the agent's refusal both say.
    pub fn edit_refused_words(self) -> Option<&'static str> {
        match self {
            Self::FileChanged | Self::FileChangedVersionUnknown => {
                Some("No more edits can be made here: the file changed on disk after its unsaved edits were made.")
            }
            Self::NotThisFile => Some("No more edits can be made here: this tab's unsaved edits don't fit its file."),
            Self::Fit | Self::VersionUnknown => None,
        }
    }

    /// **Whether the tab shows its own file's pages** ("nothing of the old file is shown as the new"): not while the file is known to be another
    /// ([`Self::file_known_other`]), as the list's numbers are about a version or a length the file on disk isn't, so none of its pages is drawn, sized,
    /// read for the selection or given links, and neither are the edits' highlights; the tab keeps the list's count and place, and its notice says why.
    /// Pages added from another PDF keep their own rule. Discard, Undo back to nothing unsaved, or the earlier version put back shows them again.
    pub fn shows_own_pages(self) -> bool {
        !self.file_known_other()
    }

    /// What the tab says of it, as a code and the words: [`FileNotice::Changed`]'s where the file changed, as that's what happened.
    pub fn said(self) -> Option<NoticeSaid> {
        match self {
            Self::Fit => None,
            Self::FileChanged | Self::FileChangedVersionUnknown => Some(NoticeSaid {
                code: FileNotice::Changed.code(),
                message: FileNotice::Changed.message(true),
            }),
            Self::NotThisFile => Some(NoticeSaid {
                code: "edits_dont_fit",
                message: "This tab's unsaved edits don't fit this file as it is on disk, so its pages aren't shown and the edits can't be saved onto it, here or as a copy. Discard the edits to see the file as it is now. Nothing has been written to the file.",
            }),
            Self::VersionUnknown => Some(NoticeSaid {
                code: "edits_version_unknown",
                message: "Which version of this file these unsaved edits were made on isn't known, so they can't be saved onto it, here or as a copy. They're kept, and nothing has been written to the file.",
            }),
        }
    }
}

/// **Whether the file found is another version than `own`, the one a tab's edits were made on:** another size or time, or another file in its place
/// (another inode on the volume, as a rename or a restore puts there), whatever its size and time. A size and time put back are what `cp -p` and a
/// careful rewrite keep, so going by them alone took a different file with the same size, time and page count for the edits' own (the changed-file
/// work's review, 2026-11-13); the page matcher then says whether the edits' pages are in it.
pub(crate) fn another_version(own: FileStamp, now: FileStamp) -> bool {
    !own.same_contents(now) || (own.inode != 0 && now.inode != 0 && own.inode != now.inode)
}

/// What a tab says while a follow of its edits into the new version is being read (the coordinator's words, 2026-11-15, provisional): the pages are
/// withheld meanwhile, and whether the edits follow or are held isn't known yet.
const LOOKING: &str = "This file changed on disk. Looking at whether your unsaved edits fit the new version.";

/// What a tab says of its file: a stable code for agents and tests, and plain words with nothing from outside in them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoticeSaid {
    pub code: &'static str,
    pub message: &'static str,
}

/// What's known of a tab whose edits don't fit its file, beyond its stamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Elsewhere {
    /// The version of the file last taken in: what's held of it (its contents, labels, the contents' verdict) is of this one, so a look that finds it again is
    /// nothing new, and one that finds another is. `None` until one's been looked at.
    pub(crate) seen: Option<FileStamp>,
    /// How long `seen` is, where it's been read and not only looked at: what the tab's length is once the edits are let go of.
    pub(crate) seen_pages: Option<u32>,
    /// The file's known to have changed on disk. Without, only that the edits don't fit it.
    pub(crate) file_changed: bool,
    /// A read found the file isn't the length the edits were made on. Without that or a change, which version they were made on is only unknown.
    pub(crate) other_length: bool,
}

/// What's rare about a tab's file, behind the one pointer a tab with none of it pays.
#[derive(Debug, Default)]
pub(crate) struct FileNews {
    notice: Option<FileNotice>,
    elsewhere: Option<Elsewhere>,
}

impl FileNews {
    /// What a record brings back of a tab's file: what's said of it, and whether its edits are known not to fit it, which the restore's look and read then settle.
    pub(crate) fn restored(notice: Option<FileNotice>, elsewhere: Option<Elsewhere>) -> Option<Box<Self>> {
        (notice.is_some() || elsewhere.is_some()).then(|| Box::new(Self { notice, elsewhere }))
    }
}

impl TabFile {
    /// What the reader's told of the file, if anything.
    pub(crate) fn notice(&self) -> Option<&FileNotice> {
        self.news.as_deref().and_then(|news| news.notice.as_ref())
    }

    /// Says something else of the file, or nothing. Whether its edits fit isn't touched.
    pub(crate) fn set_notice(&mut self, notice: Option<FileNotice>) {
        self.change_news(|news| news.notice = notice);
    }

    pub(crate) fn elsewhere(&self) -> Option<Elsewhere> {
        self.news.as_deref().and_then(|news| news.elsewhere)
    }

    pub(crate) fn set_elsewhere(&mut self, elsewhere: Option<Elsewhere>) {
        self.change_news(|news| news.elsewhere = elsewhere);
    }

    /// **The tab's facts and any edits are about `stamp` from here:** the version that's there, read or put back, so nothing is elsewhere.
    pub(crate) fn take_version(&mut self, stamp: Option<FileStamp>) {
        self.stamp = stamp;
        self.set_elsewhere(None);
    }

    fn change_news(&mut self, change: impl FnOnce(&mut FileNews)) {
        let mut news = self.news.take().unwrap_or_default();
        change(&mut news);
        if news.notice.is_some() || news.elsewhere.is_some() {
            self.news = Some(news);
        }
    }
}

impl<L: DocLoader> Session<L> {
    /// What's said of tabs' files, and whether their edits fit them: a look over the tabs and the lists.
    pub fn notices(&self) -> Notices<'_> {
        Notices {
            tabs: &self.tabs,
            lists: &self.lists,
        }
    }
}

/// What's said of tabs' files, borrowed from the session for one look: [`Session::notices`].
pub struct Notices<'a> {
    pub(crate) tabs: &'a registry::Tabs,
    pub(crate) lists: &'a lists::Lists,
}

impl Notices<'_> {
    /// **Whether a tab's unsaved edits fit its file: the one place it's decided,** for every read and output for an agent, search and Find, the sleep file, a
    /// save and the words. Nothing's looked at on disk: what's known, from the last look and read. A tab with no unsaved edits fits.
    pub fn fit(&self, id: TabId) -> EditsFit {
        self.tabs.tab(id).map_or(EditsFit::Fit, |tab| self.fit_of(tab))
    }

    /// [`Self::fit`], for a tab whose record is in hand: what a walk over every tab calls, as the sidebar and the record do. Looking each tab up again by
    /// its number made every publish quadratic in the tabs open (2026-11-13: about 0.5 µs a tab on every action at 1,000 tabs). A tab whose version is
    /// known, with nothing known against it, fits whether or not it has edits, so most tabs are answered from their own record with no lookup at all.
    pub(crate) fn fit_of(&self, tab: &TabRecord) -> EditsFit {
        // Not the notice: whatever's said over it, this is what's known.
        let elsewhere = tab.file.elsewhere();
        if (tab.file.stamp.is_some() && elsewhere.is_none()) || !self.lists.is_edited(tab.id) {
            return EditsFit::Fit;
        }
        let file_changed = elsewhere.is_some_and(|elsewhere| elsewhere.file_changed);
        match (tab.file.stamp, elsewhere) {
            (Some(_), None) => EditsFit::Fit,
            (Some(_), Some(_)) if file_changed => EditsFit::FileChanged,
            (None, _) if file_changed => EditsFit::FileChangedVersionUnknown,
            (None, None) => EditsFit::VersionUnknown,
            (None, Some(elsewhere)) if !elsewhere.other_length => EditsFit::VersionUnknown,
            _ => EditsFit::NotThisFile,
        }
    }

    /// **What the tab's own file must be for its page numbers to hold, if they depend on it:** nothing for a tab with no list in memory, whose positions
    /// are its file's pages whatever the file is now; for one with a list, the version its edits are about, or no version (`Some(None)`) when they
    /// don't fit the file ([`Self::fit`]) or its version isn't known, so that no version of the file can be taken to be current. A list asleep gives
    /// nothing, as for none: a caller reading pages wakes the list first (`recto-agent`'s executor does, for every command about a tab), and one that
    /// couldn't be woken is refused before any page is read.
    pub fn own_file_expected(&self, id: TabId) -> Option<Option<FileStamp>> {
        self.lists.get(id).map(|_| self.tabs.own_stamp(id).filter(|_| self.fit(id).fits()))
    }

    /// **Whether a tab's own pages are withheld** ([`EditsFit::shows_own_pages`]): what the page view is told, and what `tabs` and `where` say.
    pub fn pages_withheld(&self, id: TabId) -> bool {
        !self.fit(id).shows_own_pages()
    }

    /// What a tab says of its file, if anything: that it can't be found first, then that its edits don't fit it, which nothing else may hide, then its notice.
    pub fn said(&self, id: TabId) -> Option<NoticeSaid> {
        self.said_of(self.tabs.tab(id)?)
    }

    /// [`Self::said`], for a tab whose record is in hand, with no lookup of the tab: see [`Self::fit_of`].
    pub(crate) fn said_of(&self, tab: &TabRecord) -> Option<NoticeSaid> {
        let fit = self.fit_of(tab);
        let said = |notice: &FileNotice| NoticeSaid {
            code: notice.code(),
            // "Changed" speaks of the edits only where the file is known to be another than they're about, never because a list exists: edits made on
            // the version shown (the file changed, then the person edited) get the words a clean tab gets (review S1, 2026-11-15).
            message: notice.message(match notice {
                FileNotice::Changed => fit.file_known_other(),
                _ => self.lists.has_list(tab.id),
            }),
        };
        match tab.file.notice() {
            Some(missing @ FileNotice::Missing) => Some(said(missing)),
            // While a follow of its edits into the new version may still land, what's true is only that it's being looked at.
            _ if crate::version_change::under_way(self, tab) => Some(NoticeSaid {
                code: FileNotice::Changed.code(),
                message: LOOKING,
            }),
            notice => fit.said().or_else(|| notice.map(said)),
        }
    }
}
