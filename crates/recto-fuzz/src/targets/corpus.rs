//! The page-text corpus as it's read back from disk: a folder something may have damaged or changed between a build and a search, which `search`, `recto index`, an agent's folder and the next build all read. Each input damages a corpus made here, a few pages of text with a line-end hyphen, hidden text and a page with none. Half the inputs damage it as bytes of its files: overwritten, cut, grown, replaced, removed, a bit turned, a page's entry pointed elsewhere, a link or a file others can write in its place, which the seal must refuse. The other half damage what's *inside* the seal: each page's chunk is opened with the corpus's own key, its record and text changed, and sealed again, and so are the summary and the title and outline, so what reads a page past the seal (its record, what's beside its text, its UTF-8, the summary's numbers) meets hostile bytes too, or nearly nothing would get past the seal.
//!
//! What must hold: no panic, bounded time and memory; what's read back is UTF-8 and no more than its record said; the agent's folder made from it keeps to its cap; a build over it, going on from what's there, doesn't panic; and once it's cleared, a build reads back every page as it was written.

use crate::guard::{Budget, within};
use crate::input::Take;
use recto_index::corpus::{self, BuildLimits, BuildOutcome, CorpusKey, DocMeta, FailReason, FedPage, FeedError, PageFeed, ReadyCorpus};
use recto_index::seal::{self, CorpusSeal, Purpose};
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
/// A table entry on disk: where a page's chunk is, and how long.
const RECORD_LEN: usize = 12;

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

/// The Find key the fuzzed corpora are sealed with.
fn fk() -> &'static recto_platform::find_key::FindKey {
    static KEY: std::sync::OnceLock<recto_platform::find_key::FindKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| recto_platform::find_key::FindKey::in_memory().expect("a key"))
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
            frame: None,
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
    let built = corpus::build_for_file(fk(), &file, &root, key(), &mut Feed, &meta(), &limits()).expect("a corpus builds");
    assert!(matches!(built, BuildOutcome::Ready { .. }), "{built:?}");
    corpus::ensure_agent_folder(fk(), &root, key(), &limits()).expect("its agent folder");
    let texts = read_all(&root).expect("what was built reads back");
    let mut entries = Vec::new();
    walk(&root, &root, &mut entries);
    let _ = std::fs::remove_dir_all(&root);
    Made { entries, texts, file }
}

/// Every page of a corpus that's there and adds up.
fn read_all(root: &Path) -> Option<Vec<String>> {
    let corpus = ReadyCorpus::open(fk(), root, key()).ok()??;
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

/// Damages the files as the bytes say.
fn damage(root: &Path, made: &Made, take: &mut Take<'_>) {
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
                // A page's entry pointed elsewhere: where its chunk is, or how long.
                let mut table_bytes = std::fs::read(&table).unwrap_or_default();
                let records = table_bytes.len() / RECORD_LEN;
                if records > 0 {
                    let record = usize::from(take.u8()) % records * RECORD_LEN;
                    let value = take.u32();
                    if take.u8().is_multiple_of(2) {
                        table_bytes[record..record + 8].copy_from_slice(&u64::from(value).to_le_bytes());
                    } else {
                        table_bytes[record + 8..record + 12].copy_from_slice(&value.to_le_bytes());
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
}

/// Changes plain bytes as the input says: overwritten, cut, grown, a bit turned, or a word of a page's record (its kind, flags and reason, its characters and lengths) set.
fn mutate(plain: &mut Vec<u8>, take: &mut Take<'_>) {
    for _ in 0..take.u8() % 4 {
        if take.is_empty() {
            return;
        }
        let at = usize::from(take.u16()) % (plain.len() + 1);
        match take.u8() % 5 {
            0 => {
                for index in 0..usize::from(take.u8() % 16) {
                    let byte = take.u8();
                    match plain.get_mut(at + index) {
                        Some(slot) => *slot = byte,
                        None => plain.push(byte),
                    }
                }
            }
            1 => plain.truncate(at),
            2 => {
                for _ in 0..take.u8() % 32 {
                    plain.push(take.u8());
                }
            }
            3 => {
                if let Some(byte) = plain.get_mut(at) {
                    *byte ^= 1 << (take.u8() % 8);
                }
            }
            _ => {
                let word = 4 * usize::from(take.u8() % 4);
                let value = take.u32().to_le_bytes();
                if let Some(slot) = plain.get_mut(word..word + 4) {
                    slot.copy_from_slice(&value);
                }
            }
        }
    }
}

/// **Damage inside the seal.** Each page's chunk that opens is opened with the corpus's key, changed as the input says, and sealed again for the same page, the pack and table written anew to hold the new lengths; a chunk that doesn't open is carried as it is. The summary and the title and outline are changed the same way. So the reader's checks after the seal see hostile plaintext, as they would if a key were ever had by someone else.
fn reseal(root: &Path, take: &mut Take<'_>) {
    let app = root.join(key().hex()).join(corpus::FORMAT).join("app");
    let Some(seal) = std::fs::read(app.join("seal.bin"))
        .ok()
        .and_then(|header| CorpusSeal::open(fk(), key(), &header).ok())
    else {
        return;
    };
    let (Ok(table), Ok(pack)) = (std::fs::read(app.join("pages.bin")), std::fs::read(app.join("text.pack"))) else {
        return;
    };
    let (mut new_table, mut new_pack, mut sealed) = (Vec::new(), Vec::new(), Vec::new());
    for (page, entry) in table.as_chunks::<RECORD_LEN>().0.iter().enumerate() {
        let offset = u64::from_le_bytes(entry[..8].try_into().unwrap_or([0; 8]));
        let len = u32::from_le_bytes(entry[8..12].try_into().unwrap_or([0; 4]));
        let Some(chunk) = usize::try_from(offset).ok().and_then(|start| pack.get(start..start.checked_add(len as usize)?)) else {
            new_table.extend_from_slice(entry);
            continue;
        };
        let mut chunk = chunk.to_vec();
        let purpose = Purpose::Page(page as u32);
        if let Ok(plain) = seal.open_in_place(purpose, &mut chunk) {
            let mut plain = chunk[plain].to_vec();
            mutate(&mut plain, take);
            if seal.seal(purpose, &mut sealed, |out| out.extend_from_slice(&plain)).is_ok() {
                chunk = std::mem::take(&mut sealed);
            }
        }
        new_table.extend_from_slice(&(new_pack.len() as u64).to_le_bytes());
        new_table.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        new_pack.extend_from_slice(&chunk);
    }
    let _ = std::fs::write(app.join("text.pack"), &new_pack);
    let _ = std::fs::write(app.join("pages.bin"), &new_table);
    for (name, purpose) in [("ready.sealed", Purpose::Summary), ("meta.sealed", Purpose::Meta)] {
        let Ok(mut chunk) = std::fs::read(app.join(name)) else { continue };
        let Ok(plain) = seal.open_in_place(purpose, &mut chunk) else { continue };
        let mut plain = chunk[plain].to_vec();
        mutate(&mut plain, take);
        if seal.seal(purpose, &mut sealed, |out| out.extend_from_slice(&plain)).is_ok() {
            let _ = std::fs::write(app.join(name), &sealed);
        }
    }
}

/// Everything that reads a corpus, on what's there now.
fn read_back(root: &Path, made: &Made) {
    let _ = corpus::disk_state(fk(), root, key());
    if let Ok(Some(found)) = ReadyCorpus::open(fk(), root, key()) {
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
                        stored.text.len() as u64 + seal::OVERHEAD as u64 <= u64::from(said),
                        "page {page}: {} bytes of text read from a chunk of {said}",
                        stored.text.len()
                    );
                }
            }
        }
    }
    if let Ok(folder) = corpus::ensure_agent_folder(fk(), root, key(), &limits()) {
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
        if take.u8() & 1 == 1 {
            reseal(&root, &mut take);
        } else {
            damage(&root, made, &mut take);
        }
        read_back(&root, made);
        // A build goes on from what's there, believing only records that hold up.
        let _ = corpus::build(fk(), &root, key(), &mut Feed, &meta(), &limits());
        read_back(&root, made);
        // Cleared, it's built again from the file, and every page reads back as it was written.
        corpus::forget_file(&root, &made.file);
        if corpus::clear(&root, key()).is_ok() || !root.join(key().hex()).exists() {
            let built = corpus::build(fk(), &root, key(), &mut Feed, &meta(), &limits());
            assert!(matches!(built, Ok(BuildOutcome::Ready { .. })), "{built:?}");
            assert_eq!(read_all(&root).as_ref(), Some(&made.texts), "built again, it reads back as written");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: resealing no longer matches the reader's seal (another page's purpose, another key, the pack and table out of step), so half of what the fuzzer sends stops at the seal and the reader's checks after it go untested. An untouched corpus, resealed with nothing changed, reads back the same; one whose text is changed inside the seal opens, and reads back changed.
    #[test]
    fn resealing_gets_past_the_seal() {
        let folder = Folder::fresh(&state_base());
        let made = &made(&folder.0);
        let root = folder.0.join("reseal-test");
        lay_out(&root, made);
        reseal(&root, &mut Take(&[]));
        assert_eq!(read_all(&root).as_ref(), Some(&made.texts), "nothing changed, resealed");
        // One mutation of page 0: a bit of its text's first byte, past the 16-byte record, turned. Its UTF-8 still holds ('I' to 'H').
        reseal(&root, &mut Take(&[1, 16, 0, 3, 0]));
        let read = read_all(&root).expect("a changed page still opens");
        assert_ne!(read, made.texts, "the change didn't get past the seal");
        assert_eq!(read[1..], made.texts[1..], "only page 0 changed");
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
