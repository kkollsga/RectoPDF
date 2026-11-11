//! **Sleep files for tabs pushed out of the warm ones: one queue, one writer, and a cap on both** (AGENTS rule 1: "No thread, task, buffer or handle per idle item, caps on everything from outside"). Each tab leaving used to get a thread of its own, which held its pictures while it waited on the one-at-a-time lock: on a slow disk 39 of them at once in `a_thousand_tabs_come_back_cold` (slow mode, 2026-11-11), so threads and memory grew with the tabs leaving.
//!
//! **One writer, only while there's work.** The first tab queued starts it; it writes what's queued, oldest first, and ends when there's nothing left, which it finds out under the same lock a tab's queued under, so a tab queued as it ends starts the next one and is never left behind. At rest there's no thread.
//!
//! **Full, memory stays flat**, decided here and tested without the app:
//! - A tab queued again keeps only its newest: the older is what the newer says, less up to date.
//! - Past [`MAX_LEAVING_BYTES`] of pictures, the oldest queued lose theirs first. What's left of them is small, their facts, and writing that keeps the pictures already in their files when they're still of the same pages (`write_with_material`).
//! - Past [`MAX_LEAVING_TABS`], the oldest queued goes, and is counted. It costs only the quick way back: that tab wakes from its PDF, or from an older file only if that's still about the same document, and shows bare paper till it's drawn. Its state is in the record, which never waits for any of this.

use recto_reader::SleepMaterial;
use recto_session::{SleepFacts, TabId};
use serde_json::{Value, json};
use std::collections::VecDeque;

/// Tabs whose sleep files may wait to be written at once. Each is a tab off screen, so a handful covers going through tabs at any pace a person does; past it, tabs are leaving faster than the disk takes their files, and the oldest go.
pub(crate) const MAX_LEAVING_TABS: usize = 8;
/// Pictures that may wait to be written, in all, whatever comes of the tabs leaving: four tabs' worth at their most (`DocView::left_pictures` keeps 4 MiB a tab), and about twenty letter pages at a Retina window's fit, 730 KiB each.
pub(crate) const MAX_LEAVING_BYTES: u64 = 16 * 1024 * 1024;

/// One tab's sleep file, waiting.
struct Leaving {
    facts: SleepFacts,
    material: SleepMaterial,
    /// What its pictures hold.
    bytes: u64,
}

fn bytes_of(facts: &SleepFacts, material: &SleepMaterial) -> u64 {
    let material: usize = material.pictures.iter().map(|picture| picture.bgra.len()).sum();
    let facts: usize = facts.sleep.pictures.iter().map(|picture| picture.bgra.len()).sum();
    (material + facts) as u64
}

/// **The queue,** behind one lock in the sleeper.
#[derive(Default)]
pub(crate) struct LeavingQueue {
    queue: VecDeque<Leaving>,
    bytes: u64,
    /// Whether the writer's there, which is whether there's anything queued or being written.
    writer: bool,
    peak_bytes: u64,
    peak_tabs: usize,
    peak_writers: u32,
    writers_started: u64,
    /// Tabs queued again, whose older one went for the newer.
    replaced: u64,
    /// Tabs that lost their pictures to the byte cap.
    pictures_dropped: u64,
    /// Tabs whose sleep file wasn't written, past the cap on tabs.
    dropped: u64,
}

impl LeavingQueue {
    /// Queues a tab's sleep file, by the rules above. Says whether a writer has to be started: only when there isn't one.
    #[must_use]
    pub(crate) fn push(&mut self, facts: SleepFacts, material: SleepMaterial) -> bool {
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
        // The oldest first, and the one just queued too if it's more than the whole cap by itself.
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
        self.peak_writers = self.peak_writers.max(1);
        true
    }

    /// The writer's next, oldest first. `None` is the writer's end, said here under the same lock as [`Self::push`], so whatever's queued after starts another.
    pub(crate) fn next(&mut self) -> Option<(SleepFacts, SleepMaterial)> {
        let Some(leaving) = self.queue.pop_front() else {
            self.writer = false;
            return None;
        };
        self.bytes -= leaving.bytes;
        Some((leaving.facts, leaving.material))
    }

    /// The writer couldn't be started, or what it needs has gone. What's queued stays, and the next tab queued starts one.
    pub(crate) fn writer_gone(&mut self) {
        self.writer = false;
    }

    /// A tab's sleep file's being written from what's newer than what's queued for it: that goes.
    pub(crate) fn forget(&mut self, id: TabId) {
        if let Some(at) = self.queue.iter().position(|leaving| leaving.facts.id == id) {
            self.remove(at);
        }
    }

    fn remove(&mut self, at: usize) {
        if let Some(leaving) = self.queue.remove(at) {
            self.bytes -= leaving.bytes;
        }
    }

    pub(crate) fn report(&self) -> Value {
        json!({
            "queued": self.queue.len(),
            "queued_bytes": self.bytes,
            "writer": self.writer,
            "peak_tabs": self.peak_tabs,
            "peak_bytes": self.peak_bytes,
            "peak_writers": self.peak_writers,
            "writers_started": self.writers_started,
            "cap_tabs": MAX_LEAVING_TABS,
            "cap_bytes": MAX_LEAVING_BYTES,
            "replaced": self.replaced,
            "pictures_dropped": self.pictures_dropped,
            "dropped": self.dropped,
        })
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
        assert_eq!(report["peak_writers"], 1);
        assert!(report["peak_bytes"].as_u64().unwrap() <= MAX_LEAVING_BYTES);
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
        let (first, _) = queue.next().unwrap();
        assert_eq!(first.id, TabId(2), "the one queued again went to the back");
        let (second, newest) = queue.next().unwrap();
        assert_eq!(second.id, TabId(1));
        assert_eq!(newest.pictures[0].bgra[0], 2, "its newest pictures");
        assert!(queue.next().is_none());
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
        assert!(queue.next().is_some());
        assert!(!queue.push(facts(2), material(10, 0)), "still writing");
        assert!(queue.next().is_some());
        assert!(queue.next().is_none(), "nothing left, so it ends");
        assert_eq!(queue.report()["writer"], false);
        assert!(queue.push(facts(3), material(10, 0)), "a tab after its end starts another");
        assert_eq!(queue.report()["peak_writers"], 1);
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
        assert_eq!(queue.next().unwrap().0.id, TabId(2));
    }
}
