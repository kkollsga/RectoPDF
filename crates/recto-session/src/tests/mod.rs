use super::*;
use recto_core::TocLine;
use recto_core::nav::{LabelRange, LabelStyle, PageLabels};
use std::cell::Cell;

mod compare_tests;
mod confined_tests;
mod contents;
mod editing;
mod files;
mod follow;
mod forgetting;
mod journal;
mod pages;
mod paper;
mod parts;
mod recent;
mod restore;
mod saving;
mod sidebar;
mod sleep_tests;
mod tabs;
mod tiers;
mod trail_tests;
mod view;

struct FakeLoader {
    pages: u32,
    loads: Cell<u32>,
}

impl DocLoader for FakeLoader {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        self.loads.set(self.loads.get() + 1);
        if path.to_string_lossy().contains("missing") {
            return Err(OpenError::NotFound);
        }
        Ok(DocFacts {
            page_count: self.pages,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

fn session(pages: u32) -> Session<FakeLoader> {
    Session::new(FakeLoader { pages, loads: Cell::new(0) })
}

fn journaled(pages: u32) -> Session<FakeLoader> {
    let mut s = session(pages);
    s.journals_mut().set_journaling(true);
    s
}

/// What the journal-writing thread does, into plain memory: what comes back from a crash is whatever it had got to.
fn write_journal(s: &mut Session<FakeLoader>, files: &mut HashMap<String, Vec<u8>>) {
    let work = s.journal_work();
    for name in work.retired {
        files.remove(&name);
    }
    let mut outcome = JournalOutcome::default();
    for append in work.appends {
        if let Some(base) = &append.base {
            let header = JournalHeader {
                base_seq: append.through,
                ..append.header.clone()
            };
            files.insert(append.name.clone(), record::journal_start(&header, Some(base)));
            if let Some(old) = append.replaces.as_ref().filter(|old| **old != append.name) {
                files.remove(old);
            }
        } else {
            let file = files.entry(append.name.clone()).or_insert_with(|| record::journal_start(&append.header, None));
            file.extend_from_slice(&record::journal_lines(&append.ops));
        }
        outcome.written.push((append.tab, append.through, append.name));
    }
    s.journal_outcome(outcome);
}

fn back_from(files: &HashMap<String, Vec<u8>>) -> HashMap<String, RestoredEdits> {
    files
        .iter()
        .map(|(name, bytes)| {
            let parsed = record::parse_journal(bytes, record::CURRENT_VERSION).unwrap();
            let (edits, applied) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).unwrap();
            assert_eq!(applied, parsed.ops.len());
            (
                name.clone(),
                RestoredEdits {
                    edits,
                    seq: parsed.header.base_seq + applied as u64,
                    rolled_off: false,
                    lost_lines: parsed.lost_lines,
                },
            )
        })
        .collect()
}

fn kept(name: &str, size: u64) -> edit::AddedSource {
    edit::AddedSource {
        path: PathBuf::from(format!("/state/journals/{name}")).into_boxed_path(),
        page_count: 5,
        stamp: Some(FileStamp {
            size,
            mtime_ns: 1,
            device: 1,
            inode: 7,
        }),
        kept: true,
        protected: false,
        holds: None,
    }
}

const KEPT_A: &str = "kept-00000000000000aa.pdf";
const KEPT_B: &str = "kept-00000000000000bb.pdf";

fn at(path: &str) -> FileStatus {
    FileStatus::Moved(
        PathBuf::from(path),
        FileStamp {
            size: 1,
            mtime_ns: 1,
            device: 1,
            inode: 1,
        },
    )
}

/// Twelve pages: `i` to `iv`, then `1` to `8`.
struct Labelled;

impl DocLoader for Labelled {
    fn load(&self, _: &Path) -> Result<DocFacts, OpenError> {
        let mut toc = Toc::default();
        toc.labels = PageLabels::from_ranges(
            vec![
                LabelRange {
                    start: 0,
                    first: 1,
                    style: LabelStyle::RomanLower,
                    prefix: "".into(),
                },
                LabelRange {
                    start: 4,
                    first: 1,
                    style: LabelStyle::Decimal,
                    prefix: "".into(),
                },
            ],
            false,
        );
        Ok(DocFacts {
            page_count: 12,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc,
            renderer_only: false,
        })
    }
}

/// A tab's record with nothing known of it but its number and page count, for a test of a component built without a session.
fn bare_tab(id: u32, page_count: u32) -> TabRecord {
    TabRecord {
        from_sleep: None,
        touched: 0,
        id: TabId(id),
        name: "bare".into(),
        page: 0,
        page_count,
        toc_open: true,
        problem: None,
        renderer_only: false,
        view_rotation: 0,
        spread: record::Spread::Single,
        place_dirty: false,
        contents_verdict: ContentsVerdict::Fine,
        generated_contents: false,
        folds: None,
        file: TabFile {
            dir: 0,
            file_name: OsStr::new("bare.pdf").into(),
            stamp: None,
            notice: None,
            encrypted: false,
            password: false,
        },
        reading: TabPlace {
            place: None,
            selection: None,
            trail: None,
        },
    }
}
