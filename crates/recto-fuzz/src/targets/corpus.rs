//! The page-text corpus as it's read back from disk: a folder something may have damaged or changed between a build and a search, which `search`, `recto index`, an agent's folder and the next build all read. Each input damages a corpus made here, a few pages of text with a line-end hyphen, hidden text and a page with none, as bytes of its files: overwritten, cut, grown, replaced, removed, a bit turned, a page's record pointed elsewhere, a link or a file others can write in its place. Half the inputs then go in with every page's checksum put right, or nearly nothing would get past them.
//!
//! What must hold: no panic, bounded time and memory; what's read back is UTF-8 and no more than its record said; the agent's folder made from it keeps to its cap; a build over it, going on from what's there, doesn't panic; and once it's cleared, a build reads back every page as it was written.

use crate::guard::{Budget, within};
use crate::input::Take;
use recto_index::corpus::{self, BuildLimits, BuildOutcome, CorpusKey, DocMeta, FailReason, FedPage, FeedError, PageFeed, ReadyCorpus};
use recto_platform::local;
use recto_proto::CharRecord;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

const BUDGET: Budget = Budget {
    time: Duration::from_secs(60),
    memory: 512 * 1024 * 1024,
};

/// The pages a corpus is made of. `#` starts characters with no ink and `|` ends them, so a page has hidden spans; U+FFFE is PDFium's line-end hyphen, so a page has another spelling.
const PAGES: &[&str] = &[
    "Introduction to valu\u{FFFE}\r\nable things,\r\nwith a second line.",
    "Results #hidden words| and more",
    "",
    "Last page \u{2014} \u{6570} \u{1F600} ﬁn",
];

/// Most pages read back from a damaged corpus: the made one has four, and a forged table can say 200,000.
const MAX_PAGES_READ: u32 = 64;
/// Most damage done to one corpus.
const MAX_OPS: usize = 12;
const RECORD_LEN: usize = 28;

/// A file's mode, where there are modes.
fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

/// A link at `path` to `target`, where there are links.
fn link(target: &Path, path: &Path) {
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(target, path);
    #[cfg(not(unix))]
    let _ = (target, path);
}

fn key() -> CorpusKey {
    CorpusKey::from_hex(&"c3".repeat(32)).unwrap_or_else(|| unreachable!())
}

/// A page's characters, as the worker would give them, with no boxes.
fn chars(text: &str) -> Vec<CharRecord> {
    let mut no_ink = false;
    let mut out = Vec::new();
    for ch in text.chars() {
        match ch {
            '#' => no_ink = true,
            '|' => no_ink = false,
            '\u{FFFE}' => out.push(CharRecord::new(0xFFFE, CharRecord::HYPHEN)),
            c => out.push(CharRecord::new(c as u32, if no_ink && !c.is_whitespace() { CharRecord::NO_INK } else { 0 })),
        }
    }
    out
}

struct Feed;

impl PageFeed for Feed {
    fn page_count(&mut self) -> Result<u32, FeedError> {
        Ok(PAGES.len() as u32)
    }

    fn read(&mut self, page: u32) -> Result<FedPage, FeedError> {
        let text = PAGES.get(page as usize).ok_or(FeedError::Page(FailReason::Internal))?;
        Ok(FedPage {
            chars: chars(text),
            boxes: Vec::new(),
            hidden_checked: true,
        })
    }

    fn still_the_same_file(&mut self) -> bool {
        true
    }
}

fn meta() -> DocMeta {
    DocMeta {
        title: Some("A title".into()),
        outline: vec![(0, Some(0), "Introduction".into()), (1, Some(3), "Last".into())],
        renderer_only: false,
        file_bytes: 1024,
    }
}

/// Small enough that a forged corpus can't make the agent's folder big.
fn limits() -> BuildLimits {
    BuildLimits {
        max_doc_bytes: 4 * 1024 * 1024,
        max_total_bytes: 64 * 1024 * 1024,
        ..BuildLimits::default()
    }
}

static NEXT_FOLDER: AtomicU32 = AtomicU32::new(0);

/// Where runs make their folders: under `RECTO_FUZZ_STATE` if there's one, as `tools/fuzz.sh` clears that up, and the temp folder if not.
fn state_base() -> PathBuf {
    std::env::var_os("RECTO_FUZZ_STATE").map_or_else(|| std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir()), PathBuf::from)
}

/// One run's own folder: made fresh, never one that was there, and removed when the run ends, by a panic too. The temp folder is shared, so nothing else in it is touched.
struct Folder(PathBuf);

impl Folder {
    fn fresh(base: &Path) -> Self {
        std::fs::create_dir_all(base).expect("a folder to work in");
        loop {
            let path = base.join(format!("corpus-{}-{}", std::process::id(), NEXT_FOLDER.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    set_mode(&path, 0o700);
                    return Self(path);
                }
                // Another process's, or one left by a process with this number before: not ours to use or remove.
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("a folder to work in: {error}"),
            }
        }
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A corpus as a build leaves it, with its agent folder and a file's note of its key: each file's place under the root and its bytes, or `None` for a folder. Made in each run's own folder, as the note of the key goes by the file's path.
struct Made {
    entries: Vec<(PathBuf, Option<Vec<u8>>)>,
    /// What each page reads back as, from the made corpus itself.
    texts: Vec<String>,
    /// The file the corpus is of, for the note of its key.
    file: PathBuf,
}

fn walk(root: &Path, at: &Path, out: &mut Vec<(PathBuf, Option<Vec<u8>>)>) {
    let Ok(entries) = std::fs::read_dir(at) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        let Ok(relative) = path.strip_prefix(root) else { continue };
        if path.is_dir() {
            out.push((relative.to_path_buf(), None));
            walk(root, &path, out);
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.push((relative.to_path_buf(), Some(bytes)));
        }
    }
}

fn made(base: &Path) -> Made {
    let file = base.join("document.pdf");
    std::fs::write(&file, b"%PDF-1.7 made for the corpus target").expect("the file a corpus is of");
    let root = base.join("made");
    local::ensure_private_dir(&root).expect("a private folder");
    let built = corpus::build_for_file(&file, &root, key(), &mut Feed, &meta(), &limits()).expect("a corpus builds");
    assert!(matches!(built, BuildOutcome::Ready { .. }), "{built:?}");
    corpus::ensure_agent_folder(&root, key(), &limits()).expect("its agent folder");
    let texts = read_all(&root).expect("what was built reads back");
    let mut entries = Vec::new();
    walk(&root, &root, &mut entries);
    let _ = std::fs::remove_dir_all(&root);
    Made { entries, texts, file }
}

/// Every page of a corpus that's there and adds up.
fn read_all(root: &Path) -> Option<Vec<String>> {
    let corpus = ReadyCorpus::open(root, key()).ok()??;
    let mut buffer = Vec::new();
    (0..corpus.pages())
        .map(|page| corpus.page(page, &mut buffer).ok().map(|stored| stored.text.to_string()))
        .collect()
}

/// The made corpus again, in a folder of this input's own.
fn lay_out(root: &Path, made: &Made) {
    let _ = std::fs::remove_dir_all(root);
    local::ensure_private_dir(root).expect("a private folder");
    for (relative, bytes) in &made.entries {
        let path = root.join(relative);
        match bytes {
            None => local::ensure_private_dir(&path).expect("a private folder"),
            Some(bytes) => {
                std::fs::write(&path, bytes).expect("a file of the corpus");
                set_mode(&path, 0o600);
            }
        }
    }
}

/// Damages the files as the bytes say. Says whether to put the checksums right after.
fn damage(root: &Path, made: &Made, take: &mut Take<'_>) -> bool {
    let first = take.u8();
    let files: Vec<PathBuf> = made
        .entries
        .iter()
        .filter(|(_, bytes)| bytes.is_some())
        .map(|(relative, _)| root.join(relative))
        .chain([root.join(key().hex()).join(corpus::FORMAT).join("app").join("refused.json")])
        .collect();
    let table = root.join(key().hex()).join(corpus::FORMAT).join("app").join("pages.bin");
    for _ in 0..MAX_OPS {
        if take.is_empty() {
            break;
        }
        let op = take.u8() % 10;
        let path = files[usize::from(take.u8()) % files.len()].clone();
        let mut bytes = std::fs::read(&path).unwrap_or_default();
        let at = |take: &mut Take<'_>, len: usize| usize::from(take.u16()) % (len + 1);
        match op {
            0 => {
                let offset = at(take, bytes.len());
                let count = usize::from(take.u8() % 32);
                for index in 0..count {
                    let byte = take.u8();
                    match bytes.get_mut(offset + index) {
                        Some(slot) => *slot = byte,
                        None => bytes.push(byte),
                    }
                }
            }
            1 => bytes.truncate(at(take, bytes.len())),
            2 => {
                for _ in 0..take.u8() % 64 {
                    bytes.push(take.u8());
                }
            }
            3 => bytes = take.rest().to_vec(),
            4 => {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            5 => {
                if !bytes.is_empty() {
                    let offset = at(take, bytes.len() - 1);
                    bytes[offset] ^= 1 << (take.u8() % 8);
                }
            }
            6 => {
                // A page's record pointed elsewhere, one field at a time: where its text is, how long, what kind and what flags.
                let mut table_bytes = std::fs::read(&table).unwrap_or_default();
                let records = table_bytes.len() / RECORD_LEN;
                if records > 0 {
                    let record = usize::from(take.u8()) % records * RECORD_LEN;
                    let value = take.u32();
                    match take.u8() % 7 {
                        0 => table_bytes[record..record + 8].copy_from_slice(&u64::from(value).to_le_bytes()),
                        field @ 1..=3 => {
                            let start = record + 4 + 4 * usize::from(field);
                            table_bytes[start..start + 4].copy_from_slice(&value.to_le_bytes());
                        }
                        field => table_bytes[record + 20 + usize::from(field - 4)] = value as u8,
                    }
                    let _ = std::fs::write(&table, &table_bytes);
                }
                continue;
            }
            7 => {
                let code: String = (0..take.u8() % 16).map(|_| char::from(b'a' + take.u8() % 26)).collect();
                bytes = format!("{{\"code\":\"{code}\"}}").into_bytes();
            }
            8 => {
                // A link in a file's place, which nothing should follow.
                let target = files[usize::from(take.u8()) % files.len()].clone();
                let _ = std::fs::remove_file(&path);
                link(&target, &path);
                continue;
            }
            _ => {
                // Others can write it, so it can't be believed.
                set_mode(&path, 0o666);
                continue;
            }
        }
        if path.symlink_metadata().is_ok_and(|meta| meta.file_type().is_symlink()) {
            continue;
        }
        let _ = std::fs::write(&path, &bytes);
    }
    first & 1 == 1
}

/// Puts every whole record's checksum right for what its text and what's beside it are now, as the reader checks it: the page's number, the record as decoded with its offset, its unread byte and its checksum as zeros, then the bytes. The pack is read as it stands.
fn reseal(root: &Path) {
    let app = root.join(key().hex()).join(corpus::FORMAT).join("app");
    let (Ok(mut table), Ok(pack)) = (std::fs::read(app.join("pages.bin")), std::fs::read(app.join("text.pack"))) else {
        return;
    };
    for (page, record) in table.as_chunks_mut::<RECORD_LEN>().0.iter_mut().enumerate() {
        let word = |at: usize| u32::from_le_bytes([record[at], record[at + 1], record[at + 2], record[at + 3]]) as usize;
        let offset = u64::from_le_bytes(record[..8].try_into().unwrap_or([0; 8]));
        let length = word(8).saturating_add(word(12));
        let Some(blob) = usize::try_from(offset).ok().and_then(|start| pack.get(start..start.checked_add(length)?)) else {
            continue;
        };
        // As the reader hashes it: the fields it decodes, so the byte between them, which it never reads, as zero.
        let mut fields = [0u8; RECORD_LEN];
        fields[8..23].copy_from_slice(&record[8..23]);
        let mut hash = crc32fast::Hasher::new();
        hash.update(&(page as u32).to_le_bytes());
        hash.update(&fields);
        hash.update(blob);
        record[24..28].copy_from_slice(&hash.finalize().to_le_bytes());
    }
    let _ = std::fs::write(app.join("pages.bin"), &table);
}

/// Everything that reads a corpus, on what's there now.
fn read_back(root: &Path, made: &Made) {
    let _ = corpus::disk_state(root, key());
    if let Ok(Some(found)) = ReadyCorpus::open(root, key()) {
        // The table as it is on disk, to hold what's read to what its record said.
        let table = std::fs::read(root.join(key().hex()).join(corpus::FORMAT).join("app").join("pages.bin")).unwrap_or_default();
        let mut buffer = Vec::new();
        for page in 0..found.pages().min(MAX_PAGES_READ) {
            // What's beside the text is believed only as numbers and words: the agent's folder, next, is what holds spans to the text.
            if let Ok(stored) = found.page(page, &mut buffer) {
                let _ = (stored.also_spelled(), stored.hidden_spans());
                let at = page as usize * RECORD_LEN;
                if let Some(said) = table.get(at + 8..at + 12) {
                    let said = u32::from_le_bytes([said[0], said[1], said[2], said[3]]);
                    assert!(
                        stored.text.len() as u64 <= u64::from(said),
                        "page {page}: {} bytes of text read, where its record said {said}",
                        stored.text.len()
                    );
                }
            }
        }
    }
    if let Ok(folder) = corpus::ensure_agent_folder(root, key(), &limits()) {
        assert!(folder.path.join("manifest.json").is_file(), "a finished agent folder has its manifest");
        let size: u64 = std::fs::read_dir(folder.path.join("pages"))
            .map(|entries| entries.flatten().filter_map(|entry| entry.metadata().ok()).map(|meta| meta.len()).sum())
            .unwrap_or(0);
        assert!(size <= limits().max_doc_bytes, "the agent's folder kept to its cap: {size}");
    }
    let _ = corpus::keys_of_file(root, &made.file);
}

pub fn check(data: &[u8]) {
    check_in(&state_base(), data);
}

fn check_in(base: &Path, data: &[u8]) {
    within("corpus", BUDGET, || {
        let folder = Folder::fresh(base);
        let made = &made(&folder.0);
        let root = folder.0.join("work");
        lay_out(&root, made);
        let mut take = Take(data);
        if damage(&root, made, &mut take) {
            reseal(&root);
        }
        read_back(&root, made);
        // A build goes on from what's there, believing only records that hold up.
        let _ = corpus::build(&root, key(), &mut Feed, &meta(), &limits());
        read_back(&root, made);
        // Cleared, it's built again from the file, and every page reads back as it was written.
        corpus::forget_file(&root, &made.file);
        if corpus::clear(&root, key()).is_ok() || !root.join(key().hex()).exists() {
            let built = corpus::build(&root, key(), &mut Feed, &meta(), &limits());
            assert!(matches!(built, Ok(BuildOutcome::Ready { .. })), "{built:?}");
            assert_eq!(read_all(&root).as_ref(), Some(&made.texts), "built again, it reads back as written");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: putting checksums right no longer matches the reader's, so half of what the fuzzer sends stops at a checksum and the reader's later checks go untested. An untouched corpus, resealed, must read back the same, and so must one with only the byte the reader never reads damaged.
    #[test]
    fn resealing_an_untouched_corpus_changes_nothing() {
        let folder = Folder::fresh(&state_base());
        let made = &made(&folder.0);
        let root = folder.0.join("reseal-test");
        lay_out(&root, made);
        let before = std::fs::read(root.join(key().hex()).join(corpus::FORMAT).join("app").join("pages.bin")).unwrap();
        reseal(&root);
        let after = std::fs::read(root.join(key().hex()).join(corpus::FORMAT).join("app").join("pages.bin")).unwrap();
        assert_eq!(before, after);
        assert_eq!(read_all(&root).as_ref(), Some(&made.texts));
        // The byte the reader never reads, damaged, still passes once resealed, as the reader hashes it as zero.
        let table = root.join(key().hex()).join(corpus::FORMAT).join("app").join("pages.bin");
        let mut bytes = std::fs::read(&table).unwrap();
        bytes[23] = 0xAB;
        std::fs::write(&table, &bytes).unwrap();
        reseal(&root);
        assert_eq!(read_all(&root).as_ref(), Some(&made.texts), "a damaged unread byte, resealed");
    }

    /// Failure: every process that runs the target leaves a `corpus-<pid>` folder in the shared temp folder, one more for each run of the smoke tests or a fuzz campaign, and a run that panics leaves its folder too. A run's folder is its own, fresh, and gone at its end, whether it ends well or by a panic.
    #[test]
    fn a_run_leaves_no_folder_behind_even_when_it_panics() {
        let outer = Folder::fresh(&state_base());
        let left = || {
            std::fs::read_dir(&outer.0)
                .map(|entries| entries.flatten().map(|entry| entry.file_name()).collect::<Vec<_>>())
                .unwrap()
        };
        check_in(&outer.0, &[]);
        check_in(&outer.0, &[1, 4, 0, 7, 3, 9, 200, 1]);
        assert_eq!(left(), Vec::<std::ffi::OsString>::new(), "runs that ended well");
        // A folder already there with the next name is someone else's: a run takes another, and leaves that one alone.
        let theirs = outer.0.join(format!("corpus-{}-{}", std::process::id(), NEXT_FOLDER.load(Ordering::Relaxed)));
        std::fs::create_dir(&theirs).unwrap();
        let panicked = std::panic::catch_unwind(|| {
            let folder = Folder::fresh(&outer.0);
            assert_ne!(folder.0, theirs);
            std::fs::write(folder.0.join("document.pdf"), b"made").unwrap();
            panic!("a check fails mid-run");
        });
        assert!(panicked.is_err());
        assert_eq!(
            left(),
            vec![theirs.file_name().unwrap().to_owned()],
            "a run that panicked, beside a folder that isn't its own"
        );
    }
}
