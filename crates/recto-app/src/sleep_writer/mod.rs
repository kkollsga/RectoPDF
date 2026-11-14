//! **The sleep writer, as a state machine on the clock** (step R1.4): when a tab goes dormant and how its sleep file goes (in place or queued), whether a sleep file
//! may carry new pictures now (the gap per tab and the cap per hour, the owner's rule on SSD wear: "edits come in bursts"), whether a tab held by an agent or a
//! save goes at all, the off-screen budget's look and who goes over it, memory pressure acted on or only counted, and what waits for what at the sleep files'
//! one-at-a-time lock (a flush, a quit or a hand-off waiting on it, and the queue's writer holding back for them). One small value, [`SleepWriter`], and one pure
//! function, [`SleepWriter::step`], of it, an [`Event`] and `now` in milliseconds of the app's [`crate::clock::Clock`]: no lock, no thread, no I/O. What it comes
//! to is an [`Out`] for the caller to carry out. The rules it goes by are [`crate::sleeper`]'s, unchanged.
//!
//! The app keeps it in [`crate::core::sleeper::Sleeper`], under a lock taken only by its `step` and the queries beside it, for a transition of a few bytes:
//! never across carrying out an `Out`, a session call, a page view, the disk, or the sleep files' own lock. It's taken *inside* that lock (a write's decision and
//! its count are made with the file's lock held, as the gap's and the hour's own locks were), never the other way round.
//!
//! **What isn't here, and why:** which tabs are due to go dormant is the session's, by its own clock (`Session::seconds_now`, a real `Instant`, which every
//! `touch` goes by): the session crate is outside this step. The machine gets how long until the next is due, and says when to ask which. What's queued for the
//! one writer, and its caps, is [`crate::sleep_queue::LeavingQueue`], already a value of its own with its own short lock and tests.

use crate::pressure::{self, Heard, Source};
use crate::sleeper::{self as rules, BudgetLook, DORMANT_AFTER_SECONDS, Hour, LIVE_BUDGET_BYTES, SLEEP_WRITE_EVERY_SECONDS, Why, Written};
use recto_session::TabId;
use recto_session::sleep::SleepFile;

#[cfg(test)]
mod tests;

/// **Whether the sleep file there is of the list, page count and version a wake now would take it for:** a wake refuses any other, so its pictures show
/// nothing. The one test both [`shows_the_place`] and [`with_older_pictures`] start from.
#[must_use]
pub fn same_list(there: &SleepFile, list_seq: u64, pages: u32, version: Option<u64>) -> bool {
    there.list_seq == list_seq && there.pages == pages && there.version.is_some() && there.version == version
}

/// **Whether the sleep file there shows the place**, which lets a write inside the gap go ahead for a place the file doesn't show
/// ([`there_for_other_place`]). It does when it is of the [`same_list`], and it holds a picture of every page in `wanted`, on that page's paper where it's
/// known. `wanted` is every page in view that has a picture on hand, so a two-page spread with both pages on hand isn't shown by a file of one. A page in view
/// with nothing on hand asks nothing: no write would add it. An empty `wanted` (nothing in view) is a place nothing is owed at, which no file shows.
#[must_use]
pub fn shows_the_place(there: &SleepFile, list_seq: u64, pages: u32, version: Option<u64>, wanted: &[(u32, Option<bool>)]) -> bool {
    same_list(there, list_seq, pages, version)
        && !wanted.is_empty()
        && wanted.iter().all(|(page, dark)| {
            there
                .pictures
                .iter()
                .any(|picture| picture.page == *page && dark.is_none_or(|dark| picture.dark == dark))
        })
}

/// **What a sleep file keeps of its older pictures** (found 2026-11-13: a wake from a file whose two pictures were of a place the tab had left a visit before,
/// so its first frame was bare, and the bytes were written and read for nothing; and in the second review, a spread with one page that never got a picture
/// lost the other's at the next write). An older picture stands for a page in view (`in_view`) that has nothing newer on hand, when the file there is of the
/// [`same_list`] the new file records, and it's of the paper of the pictures on hand, when there are any (with none on hand the paper isn't known, and a wake
/// puts a picture only on a page of its own paper). Every other older picture goes: pictures that can't be shown are bytes on the disk, and in memory on a
/// wake, for nothing. At most [`recto_session::sleep::MAX_SLEEP_PICTURES`] in all, the new ones first.
pub fn with_older_pictures(mut sleep: SleepFile, there: Option<SleepFile>, in_view: &[u32]) -> SleepFile {
    let Some(there) = there.filter(|there| same_list(there, sleep.list_seq, sleep.pages, sleep.version)) else {
        return sleep;
    };
    let paper = sleep.pictures.first().map(|picture| picture.dark);
    for picture in there.pictures {
        if sleep.pictures.len() >= recto_session::sleep::MAX_SLEEP_PICTURES {
            break;
        }
        let stands =
            in_view.contains(&picture.page) && paper.is_none_or(|dark| picture.dark == dark) && !sleep.pictures.iter().any(|newer| newer.page == picture.page);
        if stands {
            sleep.pictures.push(picture);
        }
    }
    sleep
}

/// **What of a tab's material may carry pictures, anywhere a sleep file's pictures go:** into a file written, into the queue waiting for one, and back onto the
/// page view of a tab shown inside its cool-down. A document with a page from a protected file has none (`SleepFacts::pictures_allowed`), so they're dropped
/// here, and nothing is held or shown that its sleep file couldn't carry. The one check, so "held to the same checks" is one implementation.
#[must_use]
pub fn allowed_material(facts: &mut recto_session::SleepFacts, mut material: recto_reader::SleepMaterial) -> recto_reader::SleepMaterial {
    if !facts.pictures_allowed {
        material.pictures = Vec::new();
        facts.sleep.pictures = Vec::new();
    }
    material
}

/// **Whether the sleep file there holds pictures only for another paper than the new ones** (found 2026-11-13 in a quiet run: a tab slept again inside its gap
/// after the look went from light to dark kept a file of light-paper pictures, which no page of dark paper shows, so its wake was bare). `new_dark` is the
/// paper of the pictures on hand, `None` with none. With none on hand, or none there, there's nothing a write would put right.
#[must_use]
pub fn there_for_other_paper(there: Option<&SleepFile>, new_dark: Option<bool>) -> bool {
    let (Some(there), Some(new_dark)) = (there, new_dark) else {
        return false;
    };
    !there.pictures.is_empty() && there.pictures.iter().all(|picture| picture.dark != new_dark)
}

/// **Whether the sleep file there shows nothing of the place the pictures on hand are of** (found 2026-11-13 in a quiet run: a tab left at page 2, slept and
/// woken, then left at page 4 and slept again inside its gap, kept a file whose one picture was of page 2, so its wake at page 4 was bare). True when a picture
/// on hand is of a page in view (`material.in_view`) and the file there doesn't [`shows_the_place`] for those pages, on their paper, against `now`'s list and
/// pages and the material's version: a write would turn a bare or half-bare wake into one from pictures. A file that shows them is what a wake can use, so a tab
/// slept again at the same place is put off as before. With no file there, nothing in view, or nothing on hand of it, there's nothing a write would put right.
#[must_use]
pub fn there_for_other_place(there: Option<&SleepFile>, now: &SleepFile, material: &recto_reader::SleepMaterial) -> bool {
    let Some(there) = there else {
        return false;
    };
    let wanted: Vec<(u32, Option<bool>)> = material
        .pictures
        .iter()
        .filter(|picture| material.in_view.contains(&picture.page))
        .map(|picture| (picture.page, Some(picture.paper == recto_reader::PaperKind::Dark)))
        .collect();
    !wanted.is_empty() && !shows_the_place(there, now.list_seq, now.pages, material.version, &wanted)
}

/// How soon a look at dormancy that couldn't go ahead, as a file's being read or the app's frozen, comes again.
pub const BUSY_RETRY_MS: u64 = 5_000;

/// Who wants a sleep file written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum By {
    /// A flush's, for the tab on screen: a quit's, a hand-off's or the saver's own. It doesn't wait for the gap or the hour: the next launch's first frame is it.
    Flush,
    /// Being asked (`recto session sleep`), by whoever sent it: written in place, as it's looked at straight after, and kept to the gap and the hour like any
    /// other; an agent's to its share of the hour too.
    Asked(recto_agent::Actor),
    /// The queue's one writer: a tab leaving the warm ones, or going dormant by itself, with whose command caused it (a tab an agent's command pushed out of
    /// the warm ones is the agent's, whoever opened it; one the person's own action pushed out, or that went dormant by itself, is the person's). Kept to the gap
    /// and the hour, and an agent's to its share too.
    Queue(recto_session::Author),
}

impl By {
    /// Whether an agent caused this write, so it counts against agents' share of the hour ([`crate::sleeper::AGENT_SLEEP_BYTES_PER_HOUR`]).
    #[must_use]
    pub fn agent(self) -> bool {
        matches!(self, Self::Asked(recto_agent::Actor::Agent(_)) | Self::Queue(recto_session::Author::Agent))
    }
}

/// **What came of one sleep file's write,** for whoever asked for it: what it carried, and whether that was cut short by agents' share of the hour (what an
/// agent is told, from this write's own outcome and no counter's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub write: Write,
    pub cut_for_share: bool,
}

/// What a sleep file about to be written carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    /// Nothing's written: the tab has a file, and a new one would be within its gap or past the hour's cap.
    PutOff,
    /// Its facts and the pictures on hand (or, with none to hand, those in the file there if it's of the same list).
    WithPictures,
    /// Its facts only: pictures aren't allowed for it (a protected file), or it's within its limits and has no file yet.
    FactsOnly,
}

/// What happens to the sleep writer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// How long a tab's left alone before it goes dormant (QA's `dormant_after`).
    SetAfter(u32),
    /// What the documents off screen may hold, in bytes (QA's `live_budget`).
    SetBudget(u64),
    /// The least time between a tab's sleep files, in seconds; zero takes the hour's cap with it (QA's `sleep_write_gap`).
    SetWriteGap(u32),
    /// What sleep files may come to in an hour, in bytes: [`crate::sleeper::SLEEP_BYTES_PER_HOUR`], set small by a test of the cap.
    SetHourCap(u64),
    /// What an agent's may come to of it: [`crate::sleeper::AGENT_SLEEP_BYTES_PER_HOUR`], set small by a test of the share.
    SetAgentHourCap(u64),
    /// The deadline's worked out again: the next tab is due this many seconds from now by the session's clock, or none is.
    Rearm { in_seconds: Option<u32> },
    /// The deadline's come, on the session thread. `busy`: a file's being read, or the app's frozen, so nothing goes now.
    DueNow { busy: bool },
    /// Tabs asked to go dormant, for `why`, each with whether the session lets it: not on screen, not held by an agent's command, not being saved.
    GoDormant { why: Why, tabs: Vec<(TabId, bool)>, frozen: bool },
    /// What came of it: this many went.
    WentDormant { why: Why, gone: usize },
    /// A sleep file's about to be written, the one-at-a-time lock held: `there` is whether the tab has a sleep file already, `there_for_other_paper`
    /// whether that file's pictures are all for another paper than the ones on hand ([`there_for_other_paper`]), and `there_for_other_place` whether it
    /// shows nothing of the place the ones on hand are of ([`there_for_other_place`]).
    Write {
        id: TabId,
        by: By,
        there: bool,
        pictures_allowed: bool,
        there_for_other_paper: bool,
        there_for_other_place: bool,
    },
    /// An agent-caused write the machine let through, measured before it's written: its file comes to `bytes`, `pictures` whether it carries any, and `there`
    /// whether the tab has a file already. Only a file that fits what's left of agents' share goes.
    AgentSize { bytes: u64, pictures: bool, there: bool },
    /// It was, this many bytes; zero is nothing new, so nothing was.
    Wrote { id: TabId, bytes: u64 },
    /// A write someone's waiting on (a flush's, being asked's) wants the one-at-a-time lock: the queue's writer lets it go first.
    LockWanted,
    /// It has it, or (`got: false`) gave up past its time and isn't written, which is counted, never silent.
    LockWaited { got: bool },
    /// What's off screen may be over its budget: `active` is the tab on screen, `off_screen` whether anything is.
    LookAtBudget { active: Option<TabId>, off_screen: bool },
    /// Each document off screen, oldest first, and what it holds.
    Costs(Vec<(TabId, u64)>),
    /// The system's memory pressure, from the host. `quiet`: a quiet run, the app under test.
    SystemPressure { critical: bool, quiet: bool },
    /// Memory pressure to act on, on the session thread.
    Pressure { critical: bool },
    /// Tabs just woken from their sleep files.
    Woken(usize),
}

/// What the caller does, once it has let go of the lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// The session thread's next look at dormancy, in the clock's milliseconds plus one, and zero for none.
    Due(u64),
    /// Whatever's due goes dormant: the session says which, by its own clock.
    TakeDue,
    /// These go dormant, whatever hasn't gone to the journals first: each one's sleep file written in place (`in_place`, being asked) or queued for the one
    /// writer (the session's own reasons, as the session thread never waits on a disk).
    GoDormant { tabs: Vec<TabId>, in_place: bool },
    /// What the sleep file about to be written carries.
    Write(Write),
    /// The same, cut short by agents' share of the hour: put off, or its facts alone (measured again, as an [`Event::AgentSize`]).
    ShareCut(Write),
    /// What each document off screen holds is wanted, for [`Event::Costs`].
    CountCosts,
    /// These go dormant, over the budget: queued for the session thread.
    SleepTabs(Vec<TabId>, Why),
    /// The system's pressure acted on: queued for the session thread as [`Event::Pressure`].
    Pressure(bool),
    /// Every tab that can go dormant goes, and the page views let go of what they can.
    AllGo(bool),
}

/// What's been counted, for `qa state`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub left_alone: u64,
    pub budget: u64,
    pub pressure: u64,
    pub asked: u64,
    pub sleep_bytes: u64,
    pub sleep_files: u64,
    pub writes_put_off: u64,
    /// Written inside the gap, as the file there had pictures only for another paper: once for each change of the look, at most, for each tab.
    pub writes_for_paper: u64,
    /// Written inside the gap, as the file there showed nothing of the place the tab was left at: once for each move between sleeps, at most, for each tab.
    pub writes_for_place: u64,
    /// Asked for by an agent and not written, as agents' share of the hour was used: the person's routes keep the rest.
    pub agent_share_used: u64,
    /// The tab on screen's file at a flush, not written as another was being written past the flush's time.
    pub writes_skipped_busy: u64,
    pub woken_from_file: u64,
    pub pressure_events: u64,
    /// The system's own pressure in a quiet run, counted and not acted on.
    pub system_warning: u64,
    pub system_critical: u64,
}

/// **The sleep writer, as it stands.** Its settings, the budget's last look, when each tab's file was last written within the gap, the hour under way, the writes
/// waiting on the one-at-a-time lock, and its counts. Next to no bytes per tab, and only for tabs written within the gap.
#[derive(Debug)]
pub struct SleepWriter {
    after_seconds: u32,
    budget_bytes: u64,
    write_gap_seconds: u32,
    budget_look: BudgetLook,
    written: Written,
    hour: Hour,
    /// Writes waiting for the one-at-a-time lock that someone's waiting on.
    others_waiting: u32,
    /// The write just decided is an agent's, so what it comes to counts against agents' share: decided and written under the one-at-a-time lock, so nothing
    /// comes between.
    writing_for_agent: bool,
    /// The write under way has been cut short by agents' share already, so it's counted once however many cuts it takes.
    cut_for_share: bool,
    counts: Counts,
}

impl Default for SleepWriter {
    fn default() -> Self {
        Self {
            after_seconds: DORMANT_AFTER_SECONDS,
            budget_bytes: LIVE_BUDGET_BYTES,
            write_gap_seconds: SLEEP_WRITE_EVERY_SECONDS,
            budget_look: BudgetLook::default(),
            written: Written::default(),
            // The hour starts with the clock, at its first millisecond.
            hour: Hour::starting(1),
            others_waiting: 0,
            writing_for_agent: false,
            cut_for_share: false,
            counts: Counts::default(),
        }
    }
}

impl SleepWriter {
    /// **The one transition.** Pure: the state, the event and `now` (the clock's milliseconds plus one, so zero is none); what it comes to is returned for the
    /// caller to carry out.
    #[must_use]
    pub fn step(&mut self, event: Event, now: u64) -> Option<Out> {
        match event {
            Event::SetAfter(seconds) => {
                self.after_seconds = seconds;
                None
            }
            Event::SetBudget(bytes) => {
                self.budget_bytes = bytes;
                None
            }
            Event::SetWriteGap(seconds) => {
                self.write_gap_seconds = seconds;
                None
            }
            Event::SetHourCap(bytes) => {
                self.hour.set_cap(bytes);
                None
            }
            Event::SetAgentHourCap(bytes) => {
                self.hour.set_agent_cap(bytes);
                None
            }
            Event::Rearm { in_seconds } => Some(Out::Due(rules::due_at(now, in_seconds))),
            // Looked at again shortly, never dropped: a deadline let go of here would leave tabs left alone awake till something else rearms it.
            Event::DueNow { busy: true } => Some(Out::Due(now + BUSY_RETRY_MS)),
            Event::DueNow { busy: false } => Some(Out::TakeDue),
            Event::GoDormant { why, tabs, frozen } => {
                // Whatever's asking: a tab on screen, held by an agent or being saved doesn't go, and nothing goes while the app's frozen.
                let tabs: Vec<TabId> = tabs.into_iter().filter_map(|(id, may)| may.then_some(id)).collect();
                (!tabs.is_empty() && !frozen).then_some(Out::GoDormant {
                    tabs,
                    in_place: matches!(why, Why::Asked(_)),
                })
            }
            Event::WentDormant { why, gone } => {
                let counter = match why {
                    Why::LeftAlone => &mut self.counts.left_alone,
                    Why::Budget => &mut self.counts.budget,
                    Why::Pressure => &mut self.counts.pressure,
                    Why::Asked(_) => &mut self.counts.asked,
                };
                *counter += gone as u64;
                None
            }
            Event::Write {
                id,
                by,
                there,
                pictures_allowed,
                there_for_other_paper,
                there_for_other_place,
            } => {
                let agent = by.agent();
                self.cut_for_share = false;
                let mut may = by == By::Flush || self.may_write_pictures(id, now, agent);
                // A file whose pictures no page of this paper shows, or no page at this place, is worth one write inside the gap, still within the hour's cap
                // (and an agent's share): a wake from it would be bare. Once written, the file there is of this paper and place, so the next sleep there is held
                // to the gap again.
                if !may && there && pictures_allowed && (there_for_other_paper || there_for_other_place) && self.hour.has_room(now, agent) {
                    may = true;
                    if there_for_other_paper {
                        self.counts.writes_for_paper += 1;
                    } else {
                        self.counts.writes_for_place += 1;
                    }
                }
                // Past an agent's share, what it causes is its facts at most, and with a file there, nothing: the person's routes keep the rest of the hour.
                let over_share = agent && !may && self.hour.agents_share_used();
                if over_share {
                    self.note_share_cut();
                }
                let write = if !may && there {
                    self.counts.writes_put_off += 1;
                    Write::PutOff
                } else {
                    self.writing_for_agent = agent;
                    if pictures_allowed && may { Write::WithPictures } else { Write::FactsOnly }
                };
                Some(if over_share { Out::ShareCut(write) } else { Out::Write(write) })
            }
            // Measured before it's written, so agents' files never pass their share: one that doesn't fit is its facts alone, for a tab with no file yet, or
            // nothing; and the share counts as used till the hour turns.
            Event::AgentSize { bytes, pictures, there } => {
                if self.hour.agents_fit(bytes) {
                    return None;
                }
                self.note_share_cut();
                if pictures && !there {
                    return Some(Out::ShareCut(Write::FactsOnly));
                }
                self.writing_for_agent = false;
                self.counts.writes_put_off += 1;
                Some(Out::ShareCut(Write::PutOff))
            }
            Event::Wrote { id, bytes } => {
                if bytes > 0 {
                    // Only a write within the gap holds the next one back, so older ones go: without this every tab ever written stayed, closed ones too.
                    self.written.note(id, self.gap_ms(), now);
                    self.hour.add(bytes, self.writing_for_agent);
                    self.counts.sleep_files += 1;
                    self.counts.sleep_bytes += bytes;
                }
                self.writing_for_agent = false;
                None
            }
            Event::LockWanted => {
                self.others_waiting += 1;
                None
            }
            Event::LockWaited { got } => {
                self.others_waiting = self.others_waiting.saturating_sub(1);
                self.counts.writes_skipped_busy += u64::from(!got);
                None
            }
            // At most once a second, and straight away on a change of tab, when a document comes to be off screen; and only with something off screen.
            Event::LookAtBudget { active, off_screen } => (off_screen && self.budget_look.now(active, now)).then_some(Out::CountCosts),
            Event::Costs(costs) => {
                let over = rules::over_budget(&costs, self.budget_bytes);
                (!over.is_empty()).then_some(Out::SleepTabs(over, Why::Budget))
            }
            Event::SystemPressure { critical, quiet } => match pressure::heard(quiet, Source::System) {
                Heard::Act => Some(Out::Pressure(critical)),
                Heard::Count => {
                    let counter = if critical {
                        &mut self.counts.system_critical
                    } else {
                        &mut self.counts.system_warning
                    };
                    *counter += 1;
                    None
                }
            },
            // Counted first, so a test can see it came.
            Event::Pressure { critical } => {
                self.counts.pressure_events += 1;
                Some(Out::AllGo(critical))
            }
            Event::Woken(tabs) => {
                self.counts.woken_from_file += tabs as u64;
                None
            }
        }
    }

    fn gap_ms(&self) -> u64 {
        u64::from(self.write_gap_seconds) * 1000
    }

    /// Whether a tab that's only moving about may have a new picture written now: past its gap, and with room in the hour (and for an agent's asking, in its
    /// share). Both are asked, as asking the hour starts a new one if this one's over.
    fn may_write_pictures(&mut self, id: TabId, now: u64, agent: bool) -> bool {
        if self.write_gap_seconds == 0 {
            return true;
        }
        let soon = self.written.soon(id, self.gap_ms(), now);
        let room = self.hour.has_room(now, agent);
        !soon && room
    }

    /// An agent-caused write cut short by its share: counted once for the write.
    fn note_share_cut(&mut self) {
        if !self.cut_for_share {
            self.cut_for_share = true;
            self.counts.agent_share_used += 1;
        }
    }

    /// **What an agent is told of a sleep file of its asking that its share of the hour cut short**, in Rust's words, from that write's own `outcome`: put
    /// off, or its facts written without pictures, and when the share comes back. `on_screen`: the tab on screen's, at the agent's `session save`, which the
    /// next launch's first frame would come from. `None` when the write wasn't cut short.
    #[must_use]
    pub fn share_words(&self, outcome: Outcome, on_screen: bool, now: u64) -> Option<String> {
        if !outcome.cut_for_share {
            return None;
        }
        let share = format!(
            "agents' share of sleep files this hour ({} MiB of {} MiB) is used, and the rest is kept for the person's own switching",
            crate::sleeper::AGENT_SLEEP_BYTES_PER_HOUR / (1024 * 1024),
            crate::sleeper::SLEEP_BYTES_PER_HOUR / (1024 * 1024)
        );
        let minutes = self.hour.minutes_left(now);
        Some(match (outcome.write, on_screen) {
            (Write::FactsOnly, false) => format!(
                "Its sleep file was written without pictures: {share}. The tab is asleep either way, and wakes without a picture until the person next leaves it; or ask again in {minutes} minutes."
            ),
            (_, false) => format!(
                "Its sleep file wasn't written: {share}. The tab is asleep either way; its file is written when the person next leaves it, or ask again in {minutes} minutes."
            ),
            (Write::FactsOnly, true) => format!(
                "The state is saved, and the sleep file of the tab on screen was written without pictures: {share}. If the app stopped now, the next launch would show that tab without a picture until it's drawn. Quitting, or the person leaving the tab, writes it; or ask again in {minutes} minutes."
            ),
            (_, true) => format!(
                "The state is saved, but the sleep file of the tab on screen wasn't written: {share}. If the app stopped now, the next launch would show that tab without a picture until it's drawn. Quitting, or the person leaving the tab, writes it; or ask again in {minutes} minutes."
            ),
        })
    }

    /// Whether the queue's writer still holds back, `waited_ms` into holding back, for a write someone's waiting on.
    #[must_use]
    pub fn holds_back(&self, waited_ms: u64) -> bool {
        rules::hold_back_for_others(self.others_waiting, waited_ms)
    }

    #[must_use]
    pub fn after_seconds(&self) -> u32 {
        self.after_seconds
    }

    #[must_use]
    pub fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

    #[must_use]
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// What sleep files agents caused have come to in the hour under way, for `qa state`: never past their share.
    #[must_use]
    pub fn agent_bytes_this_hour(&self) -> u64 {
        self.hour.agent_bytes()
    }

    /// Tabs whose last sleep file is within the gap, which are all it remembers.
    #[must_use]
    pub fn remembered(&self) -> usize {
        self.written.len()
    }
}
