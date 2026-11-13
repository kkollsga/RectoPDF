//! **Sleep files written off the session's time: one queue, one writer, and a cap on both** (AGENTS rule 1: "No thread, task, buffer or handle per idle item, caps on everything from outside"). Tabs pushed out of the warm ones, and tabs going dormant by themselves (left alone, the budget, memory pressure), queue theirs here. Each tab leaving used to get a thread of its own, which held its pictures while it waited on the one-at-a-time lock: on a slow disk 39 of them at once in `a_thousand_tabs_come_back_cold` (slow mode, 2026-11-11), so threads and memory grew with the tabs leaving. And going dormant wrote on the session thread, which with a slow disk sat behind the files queued while it served nothing else (review 1).
//!
//! **One writer, only while there's work.** The first tab queued starts it; it writes what's queued, oldest first, and ends when there's nothing left, which it finds out under the same lock a tab's queued under, so a tab queued as it ends starts the next one and is never left behind. At rest there's no thread. It's counted from its thread's start to that end, so two at once would show ([`WriterRunning`]), and one that panics leaves the way open for the next.
//!
//! **Full, memory stays flat**, decided here and tested without the app:
//! - What's held is what's queued and the one being written, which is counted till its write's done, so [`MAX_LEAVING_BYTES`] is all of it.
//! - A tab queued again keeps only its newest: the older is what the newer says, less up to date. One written straight from the page view, as a flush or being asked has it, is newer than what's queued for it, which goes, or isn't written if it's already been taken for writing.
//! - Past [`MAX_LEAVING_BYTES`] of pictures, the oldest queued lose theirs first. What's left of them is small, their facts, and writing that keeps the pictures already in their files when they're still of the same pages (`write_with_material`).
//! - Past [`MAX_LEAVING_TABS`], the oldest queued goes, and is counted. It costs only the quick way back: that tab wakes from its PDF, or from an older file only if that's still about the same document, and shows bare paper till it's drawn. Its state is in the record, which never waits for any of this.

use crate::lock;
use recto_reader::SleepMaterial;
use recto_session::{SleepFacts, TabId};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Mutex;

/// Tabs whose sleep files may wait to be written at once. Each is a tab off screen, so a handful covers going through tabs at any pace a person does; past it, tabs are leaving faster than the disk takes their files, and the oldest go.
pub const MAX_LEAVING_TABS: usize = 8;
/// Pictures that may wait to be written, in all, the one being written with them, whatever comes of the tabs leaving: four tabs' worth at their most (`DocView::left_pictures` keeps 4 MiB a tab), and about twenty letter pages at a Retina window's fit, 730 KiB each.
pub const MAX_LEAVING_BYTES: u64 = 16 * 1024 * 1024;

/// One tab's sleep file, waiting.
struct Leaving {
    facts: SleepFacts,
    material: SleepMaterial,
    /// What its pictures hold.
    bytes: u64,
}

/// The one the writer's taken, till its write's done.
struct InFlight {
    id: TabId,
    bytes: u64,
    /// Something newer's been written for it since it was taken: it isn't written.
    stale: bool,
}

fn bytes_of(facts: &SleepFacts, material: &SleepMaterial) -> u64 {
    let material: usize = material.pictures.iter().map(|picture| picture.bgra.len()).sum();
    let facts: usize = facts.sleep.pictures.iter().map(|picture| picture.bgra.len()).sum();
    (material + facts) as u64
}

/// **The queue,** behind one lock in the sleeper.
#[derive(Default)]
pub struct LeavingQueue {
    queue: VecDeque<Leaving>,
    in_flight: Option<InFlight>,
    /// What's queued and in flight hold.
    bytes: u64,
    /// Whether a writer's been started and not ended, which is whether there's anything queued or being written.
    writer: bool,
    /// Writer threads between their start and their end.
    running: u32,
    peak_bytes: u64,
    peak_tabs: usize,
    peak_running: u32,
    writers_started: u64,
    /// Tabs queued again, whose older one went for the newer.
    replaced: u64,
    /// Tabs that lost their pictures to the byte cap.
    pictures_dropped: u64,
    /// Tabs whose sleep file wasn't written, past the cap on tabs.
    dropped: u64,
    /// Taken for writing, and not written, as something newer was.
    stale_skipped: u64,
    writer_panics: u64,
}

impl LeavingQueue {
    /// Queues a tab's sleep file, by the rules above. Says whether a writer has to be started: only when there isn't one.
    #[must_use]
    pub fn push(&mut self, facts: SleepFacts, material: SleepMaterial) -> bool {
        if let Some(at) = self.queue.iter().position(|leaving| leaving.facts.id == facts.id) {
            self.remove(at);
            self.replaced += 1;
        }
        let bytes = bytes_of(&facts, &material);
        self.queue.push_back(Leaving { facts, material, bytes });
        self.bytes += bytes;
        while self.queue.len() > MAX_LEAVING_TABS {
            self.remove(0);
            self.dropped += 1;
        }
        // The oldest first, and the one just queued too if it's more than what's left by itself. The one in flight was under the cap when it was queued.
        let mut at = 0;
        while self.bytes > MAX_LEAVING_BYTES && at < self.queue.len() {
            let leaving = &mut self.queue[at];
            if leaving.bytes > 0 {
                leaving.material.pictures = Vec::new();
                leaving.facts.sleep.pictures = Vec::new();
                self.bytes -= leaving.bytes;
                leaving.bytes = 0;
                self.pictures_dropped += 1;
            }
            at += 1;
        }
        self.peak_bytes = self.peak_bytes.max(self.bytes);
        self.peak_tabs = self.peak_tabs.max(self.queue.len());
        if self.writer {
            return false;
        }
        self.writer = true;
        self.writers_started += 1;
        true
    }

    /// The writer's next, oldest first, which stays counted till [`Self::done`]. `None` is the writer's end, said here under the same lock as [`Self::push`], so whatever's queued after starts another.
    pub fn take_next(&mut self) -> Option<(SleepFacts, SleepMaterial)> {
        self.done();
        let Some(leaving) = self.queue.pop_front() else {
            self.writer = false;
            self.running = self.running.saturating_sub(1);
            return None;
        };
        self.in_flight = Some(InFlight {
            id: leaving.facts.id,
            bytes: leaving.bytes,
            stale: false,
        });
        Some((leaving.facts, leaving.material))
    }

    /// Whether the one in flight is still to be written: asked with the one-at-a-time lock held, so a newer write either came before, and this isn't written, or comes after it.
    pub fn still_wanted(&mut self) -> bool {
        let stale = self.in_flight.as_ref().is_some_and(|flight| flight.stale);
        self.stale_skipped += u64::from(stale);
        !stale
    }

    /// The one in flight is written, or skipped: what it held goes.
    pub fn done(&mut self) {
        if let Some(flight) = self.in_flight.take() {
            self.bytes -= flight.bytes;
        }
    }

    /// The writer couldn't be started, or what it needs has gone. What's queued stays, and the next tab queued starts one.
    pub fn writer_gone(&mut self) {
        self.writer = false;
    }

    /// A tab's sleep file's being written from what's newer than what's queued for it: that goes, and if it's been taken for writing already, it isn't written.
    pub fn forget(&mut self, id: TabId) {
        if let Some(at) = self.queue.iter().position(|leaving| leaving.facts.id == id) {
            self.remove(at);
        }
        if let Some(flight) = self.in_flight.as_mut().filter(|flight| flight.id == id) {
            flight.stale = true;
        }
    }

    fn remove(&mut self, at: usize) {
        if let Some(leaving) = self.queue.remove(at) {
            self.bytes -= leaving.bytes;
        }
    }

    pub fn report(&self) -> Value {
        json!({
            "queued": self.queue.len(),
            "queued_bytes": self.bytes,
            "in_flight": self.in_flight.is_some(),
            "writer": self.writer,
            "writers_running": self.running,
            "peak_tabs": self.peak_tabs,
            "peak_bytes": self.peak_bytes,
            "peak_writers": self.peak_running,
            "writers_started": self.writers_started,
            "cap_tabs": MAX_LEAVING_TABS,
            "cap_bytes": MAX_LEAVING_BYTES,
            "replaced": self.replaced,
            "pictures_dropped": self.pictures_dropped,
            "dropped": self.dropped,
            "stale_skipped": self.stale_skipped,
            "writer_panics": self.writer_panics,
        })
    }
}

/// **A writer thread, from its start:** counted as running, so two at once would show in `peak_writers`. Its end is [`LeavingQueue::next`]'s `None`; if it panics instead, this lets the next tab queued start another, as a writer marked there for good would stop every sleep file after.
pub struct WriterRunning<'a> {
    queue: &'a Mutex<LeavingQueue>,
}

impl<'a> WriterRunning<'a> {
    pub fn enter(queue: &'a Mutex<LeavingQueue>) -> Self {
        let mut held = lock(queue);
        held.running += 1;
        held.peak_running = held.peak_running.max(held.running);
        drop(held);
        Self { queue }
    }
}

impl Drop for WriterRunning<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let mut queue = lock(self.queue);
        queue.done();
        queue.writer = false;
        queue.running = queue.running.saturating_sub(1);
        queue.writer_panics += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_platform::FileStamp;
    use recto_reader::RawPicture;
    use recto_reader::paper::PaperKind;
    use recto_session::sleep::SleepFile;

    fn facts(id: u32) -> SleepFacts {
        SleepFacts {
            id: TabId(id),
            path: format!("/t{id}.pdf").into(),
            sleep: SleepFile {
                file: FileStamp {
                    size: 1,
                    mtime_ns: 1,
                    device: 1,
                    inode: u64::from(id),
                },
                file_pages: 3,
                pages: 3,
                list_seq: 0,
                renderer_only: false,
                version: Some(1),
                toc: Default::default(),
                usual: None,
                sizes: Vec::new(),
                pictures: Vec::new(),
            },
            pictures_allowed: true,
        }
    }

    /// A tab's material with one picture of this many bytes, its first byte saying which of a tab's it is.
    fn material(bytes: usize, mark: u8) -> SleepMaterial {
        let mut bgra = vec![0; bytes];
        if let Some(first) = bgra.first_mut() {
            *first = mark;
        }
        SleepMaterial {
            version: Some(1),
            usual: None,
            sizes: Vec::new(),
            pictures: vec![RawPicture {
                page: 0,
                scale: 1.0,
                width: 1,
                height: 1,
                paper: PaperKind::White,
                bgra,
            }],
            in_view: vec![0],
        }
    }

    const MIB: usize = 1024 * 1024;

    /// Failure (slow mode, 2026-11-11): each tab leaving the warm ones got a writer thread of its own, holding its pictures, so with a slow disk threads and memory grew with the tabs leaving (39 threads at once). However many leave while the writer's busy, one writer is started, the tabs queued and their pictures stay under their caps, and those past the cap on tabs are counted.
    #[test]
    fn however_many_tabs_leave_there_is_one_writer_and_the_queue_is_capped() {
        let mut queue = LeavingQueue::default();
        let mut started = 0;
        for id in 0..1000 {
            started += usize::from(queue.push(facts(id), material(3 * MIB, 0)));
            assert!(queue.queue.len() <= MAX_LEAVING_TABS);
            assert!(queue.bytes <= MAX_LEAVING_BYTES, "{} bytes queued", queue.bytes);
        }
        assert_eq!(started, 1, "one writer, however many tabs leave while it writes");
        let report = queue.report();
        assert_eq!(report["dropped"], 1000 - MAX_LEAVING_TABS as u64, "{report}");
        // The newest are the ones kept, as the tab most likely to come back is the one just left.
        let kept: Vec<u32> = queue.queue.iter().map(|leaving| leaving.facts.id.0).collect();
        assert_eq!(kept, ((1000 - MAX_LEAVING_TABS as u32)..1000).collect::<Vec<_>>());
    }

    /// Failure: a tab that leaves, comes back and leaves again before its file's written is written twice, the older last, or holds two sets of pictures. Only its newest is kept, in the newest place.
    #[test]
    fn a_tab_queued_twice_keeps_only_its_newest() {
        let mut queue = LeavingQueue::default();
        assert!(queue.push(facts(1), material(MIB, 1)));
        assert!(!queue.push(facts(2), material(MIB, 0)));
        assert!(!queue.push(facts(1), material(MIB, 2)));
        assert_eq!(queue.queue.len(), 2);
        assert_eq!(queue.bytes, 2 * MIB as u64);
        assert_eq!(queue.report()["replaced"], 1);
        let (first, _) = queue.take_next().unwrap();
        assert_eq!(first.id, TabId(2), "the one queued again went to the back");
        let (second, newest) = queue.take_next().unwrap();
        assert_eq!(second.id, TabId(1));
        assert_eq!(newest.pictures[0].bgra[0], 2, "its newest pictures");
        assert!(queue.take_next().is_none());
    }

    /// Failure: the cap on bytes is held by dropping whole tabs, or the newest, so tabs that could have had their facts written have none. Over the bytes, the oldest lose their pictures first and keep their place, and the newest keeps its own.
    #[test]
    fn over_the_byte_cap_the_oldest_lose_their_pictures_first() {
        let mut queue = LeavingQueue::default();
        let each = MAX_LEAVING_BYTES as usize / 3 + 1;
        let _ = queue.push(facts(1), material(each, 0));
        let _ = queue.push(facts(2), material(each, 0));
        let _ = queue.push(facts(3), material(each, 0));
        assert_eq!(queue.queue.len(), 3, "no tab goes for its bytes");
        assert!(queue.bytes <= MAX_LEAVING_BYTES);
        assert!(queue.queue[0].material.pictures.is_empty(), "the oldest lost its pictures");
        assert!(!queue.queue[2].material.pictures.is_empty(), "the newest kept its own");
        assert_eq!(queue.report()["pictures_dropped"], 1);
        // One more than the whole cap by itself keeps its facts only.
        let _ = queue.push(facts(4), material(MAX_LEAVING_BYTES as usize + 1, 0));
        assert!(queue.bytes <= MAX_LEAVING_BYTES);
        assert_eq!(queue.queue.len(), 4);
    }

    /// Failure: the writer stays when there's nothing to write, a thread held at rest; or a tab queued as it ends finds it still there and is never written. Its end is said under the queue's lock, and the next tab starts another.
    #[test]
    fn the_writer_ends_with_the_queue_and_the_next_tab_starts_another() {
        let mut queue = LeavingQueue::default();
        assert!(queue.push(facts(1), material(10, 0)));
        assert!(queue.take_next().is_some());
        assert!(!queue.push(facts(2), material(10, 0)), "still writing");
        assert!(queue.take_next().is_some());
        assert!(queue.take_next().is_none(), "nothing left, so it ends");
        assert_eq!(queue.report()["writer"], false);
        assert!(queue.push(facts(3), material(10, 0)), "a tab after its end starts another");
        // A writer that couldn't start leaves what's queued for the next.
        queue.writer_gone();
        assert!(queue.push(facts(4), material(10, 0)));
        assert_eq!(queue.queue.len(), 2);
    }

    /// Failure: a tab whose file's written straight from the page view, as a flush or going dormant has it, is written again after from the older pictures queued for it, overwriting the newer. What's queued for it goes.
    #[test]
    fn a_tab_written_from_newer_forgets_what_is_queued_for_it() {
        let mut queue = LeavingQueue::default();
        let _ = queue.push(facts(1), material(MIB, 0));
        let _ = queue.push(facts(2), material(MIB, 0));
        queue.forget(TabId(1));
        queue.forget(TabId(9));
        assert_eq!(queue.queue.len(), 1);
        assert_eq!(queue.bytes, MIB as u64);
        assert_eq!(queue.take_next().unwrap().0.id, TabId(2));
    }

    /// Failure (review 1): the file being written isn't counted once it's taken from the queue, so what's held is the cap and one more tab's pictures, and the peak says less than there is. It counts till its write's done, and what's queued after it gives way.
    #[test]
    fn what_is_being_written_counts_against_the_cap_till_it_is_done() {
        let mut queue = LeavingQueue::default();
        let big = MAX_LEAVING_BYTES as usize * 5 / 8;
        let _ = queue.push(facts(1), material(big, 0));
        assert!(queue.take_next().is_some());
        assert_eq!(queue.bytes, big as u64, "still held while it's written");
        let _ = queue.push(facts(2), material(big, 0));
        assert!(queue.bytes <= MAX_LEAVING_BYTES, "{} held", queue.bytes);
        assert!(queue.queue[0].material.pictures.is_empty(), "what's queued gave way to what's being written");
        assert!(queue.report()["peak_bytes"].as_u64().unwrap() <= MAX_LEAVING_BYTES);
        queue.done();
        assert_eq!(queue.bytes, 0);
        let _ = queue.take_next();
        let _ = queue.take_next();
        assert_eq!(queue.bytes, 0, "nothing's held at rest");
        assert_eq!(queue.report()["in_flight"], false);
    }

    /// Failure (review 1): a tab's older pictures, taken for writing just before a flush wrote its newer ones, are written after them and overwrite them, as `forget` looked only in the queue. Marked, it isn't written; another tab's write doesn't mark it.
    #[test]
    fn a_tab_written_newer_while_its_older_is_taken_isnt_overwritten() {
        let mut queue = LeavingQueue::default();
        let _ = queue.push(facts(1), material(10, 0));
        let _ = queue.push(facts(2), material(10, 0));
        assert_eq!(queue.take_next().unwrap().0.id, TabId(1));
        queue.forget(TabId(2));
        assert!(queue.still_wanted(), "another tab's write says nothing about this one");
        queue.forget(TabId(1));
        assert!(!queue.still_wanted(), "something newer's been written for it");
        assert_eq!(queue.report()["stale_skipped"], 1);
        assert!(queue.take_next().is_none(), "and the other was forgotten");
    }

    /// Failure (review 1): a writer that panics leaves the queue saying there's a writer for good, so no sleep file is ever written again. Its guard says it's gone, what it held goes, and the next tab queued starts another.
    #[test]
    fn a_writer_that_panics_lets_the_next_one_start() {
        let queue = Mutex::new(LeavingQueue::default());
        assert!(lock(&queue).push(facts(1), material(10, 0)));
        let ended = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _running = WriterRunning::enter(&queue);
                    let _taken = lock(&queue).take_next();
                    panic!("a writer panicking, on purpose");
                })
                .join()
        });
        assert!(ended.is_err());
        let mut queue = lock(&queue);
        let report = queue.report();
        assert_eq!(report["writer"], false, "{report}");
        assert_eq!(report["writers_running"], 0);
        assert_eq!(report["queued_bytes"], 0, "what it held went with it");
        assert_eq!(report["writer_panics"], 1);
        assert!(queue.push(facts(2), material(10, 0)), "the next tab starts another");
    }

    /// Failure (review 1): two writers run at once, as one is started while the last is still ending, or one stays when there's nothing to write. Tabs queued from four threads at once, each writer counted from its thread's start: never more than one, and none at rest with nothing held.
    #[test]
    fn writers_never_overlap_and_none_is_left_at_rest() {
        let queue = Mutex::new(LeavingQueue::default());
        let write = || {
            let _running = WriterRunning::enter(&queue);
            loop {
                let next = lock(&queue).take_next();
                if next.is_none() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
        };
        std::thread::scope(|scope| {
            for from in 0..4u32 {
                let write = &write;
                let queue = &queue;
                scope.spawn(move || {
                    for at in 0..200 {
                        if lock(queue).push(facts(from * 1000 + at), material(1024, 0)) {
                            scope.spawn(write);
                        }
                        if at % 50 == 0 {
                            std::thread::sleep(std::time::Duration::from_millis(2));
                        }
                    }
                });
            }
        });
        let report = lock(&queue).report();
        assert_eq!(report["peak_writers"], 1, "{report}");
        assert!(report["writers_started"].as_u64().unwrap() >= 1);
        assert_eq!(report["writer"], false, "{report}");
        assert_eq!(report["writers_running"], 0);
        assert_eq!(report["queued"], 0);
        assert_eq!(report["queued_bytes"], 0);
    }
}
