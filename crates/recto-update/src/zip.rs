//! **A release's zip, read by a small parser of our own.** A zip says everything twice, once in a header before each file and again in a list at the end, and tools disagree about which they believe: `unzip` lists the one and `ditto` unpacks the other, so checking with the first said nothing about what the second did. Here one reader checks that both say the same, leaves nothing in between them, and does the unpacking itself. The zip's hostile: a name goes through [`name_is_ours`], only files and folders are made and never a link, and what's written is counted as it comes out, however little the zip claims.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

/// Most files a release's zip may have. The app has a handful.
pub const MAX_ENTRIES: usize = 20_000;
/// Most it may unpack to, counted as it comes out.
pub const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_NAME_BYTES: usize = 1024;
const MAX_DEPTH: usize = 32;
const MAX_LIST_BYTES: u64 = 16 * 1024 * 1024;
/// The end record, and the longest comment a zip can have after it.
const END_LEN: usize = 22;
const MAX_END_SEARCH: u64 = 22 + 65_535;
/// Where macOS's own zips keep what isn't a file. Checked like the rest, and not unpacked.
const SIDE_FOLDER: &str = "__MACOSX";

/// What went wrong.
#[derive(Debug)]
pub enum ZipError {
    /// The zip itself: trying again won't help.
    Bad(&'static str),
    /// Reading or writing failed, as a full disk does. It might go better later.
    Io(io::Error),
    TooSlow,
}

impl From<io::Error> for ZipError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_folder: bool,
    pub executable: bool,
    stored: bool,
    crc: u32,
    compressed: u64,
    size: u64,
    data_at: u64,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// A name that stays in the folder it's unpacked into and is part of the app. Plain characters only: nothing a file system might read two ways, which is also what makes checking for duplicates sound.
#[must_use]
pub fn name_is_ours(name: &str, app_folder: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME_BYTES || !name.bytes().all(|byte| (0x20..0x7F).contains(&byte) && byte != b'\\' && byte != b':') {
        return false;
    }
    let trimmed = name.strip_suffix('/').unwrap_or(name);
    let mut count = 0;
    for part in trimmed.split('/') {
        count += 1;
        if part.is_empty() || part == "." || part == ".." {
            return false;
        }
    }
    let first = trimmed.split('/').next().unwrap_or("");
    count <= MAX_DEPTH && (first == app_folder || first == SIDE_FOLDER)
}

/// The list of what's in a zip, once it's been shown that the list and the headers agree and that there's nothing in the file but those. Nothing's unpacked.
pub fn read_directory<R: Read + Seek>(file: &mut R, app_folder: &str) -> Result<Vec<Entry>, ZipError> {
    const BAD_END: ZipError = ZipError::Bad("it isn't a zip");
    let length = file.seek(SeekFrom::End(0))?;
    let tail_len = length.min(MAX_END_SEARCH);
    file.seek(SeekFrom::Start(length - tail_len))?;
    let mut tail = vec![0u8; usize::try_from(tail_len).map_err(|_| BAD_END)?];
    file.read_exact(&mut tail)?;
    let end_at = (0..=tail.len().saturating_sub(END_LEN))
        .rev()
        .find(|&at| u32_at(&tail, at) == Some(0x0605_4b50) && u16_at(&tail, at + 20).is_some_and(|comment| at + END_LEN + usize::from(comment) == tail.len()))
        .ok_or(BAD_END)?;
    let field = |at: usize| u16_at(&tail, end_at + at).ok_or(BAD_END);
    let word = |at: usize| u32_at(&tail, end_at + at).ok_or(BAD_END);
    if field(4)? != 0 || field(6)? != 0 || field(8)? != field(10)? {
        return Err(ZipError::Bad("it's in more than one part"));
    }
    let count = usize::from(field(10)?);
    let (list_len, list_at) = (u64::from(word(12)?), u64::from(word(16)?));
    // Those at their largest are how a zip says it's too large for them, and a release isn't.
    if count == 0xFFFF || list_at == 0xFFFF_FFFF || list_len == 0xFFFF_FFFF {
        return Err(ZipError::Bad("it's larger than a release could be"));
    }
    if count > MAX_ENTRIES || list_len > MAX_LIST_BYTES {
        return Err(ZipError::Bad("it holds more than a release could"));
    }
    if list_at.checked_add(list_len) != Some(length - tail_len + end_at as u64) {
        return Err(ZipError::Bad("it has something in it that isn't the app"));
    }
    file.seek(SeekFrom::Start(list_at))?;
    let mut list = vec![0u8; usize::try_from(list_len).map_err(|_| BAD_END)?];
    file.read_exact(&mut list)?;

    let mut entries = Vec::with_capacity(count);
    let mut places: Vec<(u64, usize, bool)> = Vec::with_capacity(count);
    let mut seen = HashSet::with_capacity(count);
    let mut file_names = HashSet::new();
    let mut at = 0usize;
    const BAD_LIST: ZipError = ZipError::Bad("its list of files is damaged");
    for _ in 0..count {
        if u32_at(&list, at) != Some(0x0201_4b50) {
            return Err(BAD_LIST);
        }
        let made_by = u16_at(&list, at + 4).ok_or(BAD_LIST)?;
        let flags = u16_at(&list, at + 8).ok_or(BAD_LIST)?;
        let method = u16_at(&list, at + 10).ok_or(BAD_LIST)?;
        let crc = u32_at(&list, at + 16).ok_or(BAD_LIST)?;
        let compressed = u64::from(u32_at(&list, at + 20).ok_or(BAD_LIST)?);
        let size = u64::from(u32_at(&list, at + 24).ok_or(BAD_LIST)?);
        let name_len = usize::from(u16_at(&list, at + 28).ok_or(BAD_LIST)?);
        let extra_len = usize::from(u16_at(&list, at + 30).ok_or(BAD_LIST)?);
        let comment_len = usize::from(u16_at(&list, at + 32).ok_or(BAD_LIST)?);
        let disk = u16_at(&list, at + 34).ok_or(BAD_LIST)?;
        let mode = u32_at(&list, at + 38).ok_or(BAD_LIST)? >> 16;
        let local = u64::from(u32_at(&list, at + 42).ok_or(BAD_LIST)?);
        let name_bytes = list.get(at + 46..at + 46 + name_len).ok_or(BAD_LIST)?;
        at += 46 + name_len + extra_len + comment_len;
        if at > list.len() {
            return Err(BAD_LIST);
        }
        let name = std::str::from_utf8(name_bytes).map_err(|_| ZipError::Bad("a name in it isn't plain"))?;
        if !name_is_ours(name, app_folder) {
            return Err(ZipError::Bad("it has something in it that isn't the app"));
        }
        if flags & 0x0001 != 0 || flags & 0x0040 != 0 || flags & 0x2000 != 0 {
            return Err(ZipError::Bad("it's encrypted"));
        }
        if disk != 0 || compressed == 0xFFFF_FFFF || size == 0xFFFF_FFFF || local == 0xFFFF_FFFF {
            return Err(ZipError::Bad("it's larger than a release could be"));
        }
        if method != 0 && method != 8 {
            return Err(ZipError::Bad("it's packed in a way a release isn't"));
        }
        if method == 0 && compressed != size {
            return Err(BAD_LIST);
        }
        // A link, a device or anything but a file or a folder: there's no call for any of them, and a link's how one zip gets to write outside its folder.
        let unix = made_by >> 8 == 3;
        let kind = mode & 0o170_000;
        let says_folder = name.ends_with('/');
        if unix && !(kind == 0 || kind == 0o100_000 || (kind == 0o040_000 && says_folder)) {
            return Err(ZipError::Bad("it has a link or something else that isn't a file"));
        }
        if says_folder && size != 0 {
            return Err(BAD_LIST);
        }
        let trimmed = name.strip_suffix('/').unwrap_or(name);
        if !seen.insert(trimmed.to_ascii_lowercase()) {
            return Err(ZipError::Bad("it lists one name twice"));
        }
        if !says_folder {
            file_names.insert(trimmed.to_ascii_lowercase());
        }
        places.push((local, entries.len(), flags & 0x0008 != 0));
        entries.push(Entry {
            name: name.to_owned(),
            is_folder: says_folder,
            executable: unix && mode & 0o111 != 0,
            stored: method == 0,
            crc,
            compressed,
            size,
            data_at: 0,
        });
    }
    if at != list.len() {
        return Err(BAD_LIST);
    }
    // A file can't also be a folder that something else is in.
    for entry in &entries {
        let lower = entry.name.to_ascii_lowercase();
        let mut parent = lower.strip_suffix('/').unwrap_or(&lower);
        while let Some((above, _)) = parent.rsplit_once('/') {
            if file_names.contains(above) {
                return Err(ZipError::Bad("a file in it is also a folder"));
            }
            parent = above;
        }
    }

    // Now the headers, in the order they're in the file: each says what the list does, and each ends where the next begins, so nothing's left in between for another tool to find.
    places.sort_unstable_by_key(|place| place.0);
    let mut expected = 0u64;
    let mut total = 0u64;
    let mut header = [0u8; 30];
    for &(local, index, descriptor) in &places {
        if local != expected {
            return Err(ZipError::Bad("it has something in it that isn't the app"));
        }
        file.seek(SeekFrom::Start(local))?;
        file.read_exact(&mut header).map_err(|_| BAD_LIST)?;
        let bad_header = ZipError::Bad("a file's header doesn't say what the list says");
        if u32_at(&header, 0) != Some(0x0403_4b50) {
            return Err(bad_header);
        }
        let entry = &mut entries[index];
        let flags = u16_at(&header, 6).unwrap_or(0xFFFF);
        let method = u16_at(&header, 8).unwrap_or(0xFFFF);
        if (flags & 0x0008 != 0) != descriptor || method != if entry.stored { 0 } else { 8 } {
            return Err(bad_header);
        }
        // With a descriptor these come after the data and the header's are zero. Without one they're here, and have to be the same.
        if !descriptor
            && (u32_at(&header, 14) != Some(entry.crc)
                || u32_at(&header, 18).map(u64::from) != Some(entry.compressed)
                || u32_at(&header, 22).map(u64::from) != Some(entry.size))
        {
            return Err(bad_header);
        }
        let name_len = usize::from(u16_at(&header, 26).unwrap_or(u16::MAX));
        let extra_len = u64::from(u16_at(&header, 28).unwrap_or(u16::MAX));
        if name_len != entry.name.len() {
            return Err(bad_header);
        }
        let mut local_name = vec![0u8; name_len];
        file.read_exact(&mut local_name).map_err(|_| BAD_LIST)?;
        if local_name != entry.name.as_bytes() {
            return Err(bad_header);
        }
        entry.data_at = local + 30 + name_len as u64 + extra_len;
        let data_end = entry.data_at.checked_add(entry.compressed).ok_or(BAD_LIST)?;
        total = total.saturating_add(entry.size);
        expected = if descriptor {
            // Twelve bytes, or sixteen with its own mark. Too short for another file to be hiding there.
            file.seek(SeekFrom::Start(data_end))?;
            let mut mark = [0u8; 4];
            file.read_exact(&mut mark).map_err(|_| BAD_LIST)?;
            data_end + if u32::from_le_bytes(mark) == 0x0807_4b50 { 16 } else { 12 }
        } else {
            data_end
        };
        if expected > list_at {
            return Err(BAD_LIST);
        }
    }
    if expected != list_at {
        return Err(ZipError::Bad("it has something in it that isn't the app"));
    }
    if total > MAX_UNPACKED_BYTES {
        return Err(ZipError::Bad("it's larger than the app could be"));
    }
    Ok(entries)
}

/// Unpacks what [`read_directory`] passed into `into`, an empty folder of its own. Only files and folders are made, with modes of our choosing: none writable by anyone else, whatever the zip says. What comes out is counted as it comes, so a zip that lies about its sizes stops there, and it has to be the size and checksum the list says.
pub fn unpack_entries<R: Read + Seek>(file: &mut R, entries: &[Entry], into: &Path, deadline: Instant) -> Result<(), ZipError> {
    const WRONG: ZipError = ZipError::Bad("a file in it isn't what the list says");
    let mut written = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    for entry in entries {
        if entry.name.split('/').next() == Some(SIDE_FOLDER) {
            continue;
        }
        let path = into.join(entry.name.trim_end_matches('/'));
        if entry.is_folder {
            make_folders(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            make_folders(parent)?;
        }
        // Never over something: a link can't be there, as nothing here makes them, and a second file of a name can't.
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, if entry.executable { 0o755 } else { 0o644 });
        let mut out = options.open(&path)?;
        file.seek(SeekFrom::Start(entry.data_at))?;
        let raw = (&mut *file).take(entry.compressed);
        let mut reader: Box<dyn Read> = if entry.stored {
            Box::new(raw)
        } else {
            Box::new(flate2::read::DeflateDecoder::new(raw))
        };
        let mut crc = crc32fast::Hasher::new();
        let mut out_bytes = 0u64;
        loop {
            if Instant::now() >= deadline {
                return Err(ZipError::TooSlow);
            }
            let count = match reader.read(&mut buffer) {
                Ok(count) => count,
                // The reader of a zip that's been read whole failing is the data, not the disk.
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput | io::ErrorKind::UnexpectedEof
                    ) =>
                {
                    return Err(WRONG);
                }
                Err(error) => return Err(ZipError::Io(error)),
            };
            if count == 0 {
                break;
            }
            out_bytes += count as u64;
            written = written.saturating_add(count as u64);
            if out_bytes > entry.size || written > MAX_UNPACKED_BYTES {
                return Err(WRONG);
            }
            crc.update(&buffer[..count]);
            out.write_all(&buffer[..count])?;
        }
        if out_bytes != entry.size || crc.finalize() != entry.crc {
            return Err(WRONG);
        }
    }
    Ok(())
}

/// Folders open to read and closed to write, whatever this process's umask would have let through.
fn make_folders(path: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o755);
    builder.create(path)
}

/// Checks and unpacks a zip into an empty folder of its own.
pub fn unpack(zip: &Path, app_folder: &str, into: &Path, deadline: Instant) -> Result<(), ZipError> {
    let mut file = std::fs::File::open(zip)?;
    let entries = read_directory(&mut file, app_folder)?;
    if !entries.iter().any(|entry| entry.name.starts_with(&format!("{app_folder}/"))) {
        return Err(ZipError::Bad("it doesn't hold the app"));
    }
    unpack_entries(&mut file, &entries, into, deadline)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::Duration;

    const APP: &str = "RectoPDF.app";

    /// One entry of a zip being made by hand, with every field a hostile zip might lie in.
    #[derive(Clone)]
    pub(crate) struct Made {
        pub central_name: Vec<u8>,
        pub local_name: Vec<u8>,
        pub data: Vec<u8>,
        pub deflate: bool,
        pub mode: u32,
        /// What the list says it comes out to. Otherwise, what it does.
        pub claimed_size: Option<u32>,
        pub unlisted: bool,
    }

    impl Made {
        pub(crate) fn file(name: &str, data: &[u8]) -> Self {
            Self {
                central_name: name.as_bytes().to_vec(),
                local_name: name.as_bytes().to_vec(),
                data: data.to_vec(),
                deflate: false,
                mode: 0o100_644,
                claimed_size: None,
                unlisted: false,
            }
        }

        fn with_mode(self, mode: u32) -> Self {
            Self { mode, ..self }
        }

        fn folder(name: &str) -> Self {
            Self::file(name, b"").with_mode(0o040_755)
        }
    }

    pub(crate) fn make_zip(entries: &[Made]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut list = Vec::new();
        let mut count = 0u16;
        for entry in entries {
            let packed = if entry.deflate {
                let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(&entry.data).unwrap();
                encoder.finish().unwrap()
            } else {
                entry.data.clone()
            };
            let mut crc = crc32fast::Hasher::new();
            crc.update(&entry.data);
            let crc = crc.finalize();
            let claimed = entry.claimed_size.unwrap_or(entry.data.len() as u32);
            let at = out.len() as u32;
            let method: u16 = if entry.deflate { 8 } else { 0 };
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&claimed.to_le_bytes());
            out.extend_from_slice(&(entry.local_name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&entry.local_name);
            out.extend_from_slice(&packed);
            if entry.unlisted {
                continue;
            }
            count += 1;
            list.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            list.extend_from_slice(&((3u16 << 8) | 20).to_le_bytes());
            list.extend_from_slice(&20u16.to_le_bytes());
            list.extend_from_slice(&0u16.to_le_bytes());
            list.extend_from_slice(&method.to_le_bytes());
            list.extend_from_slice(&[0; 4]);
            list.extend_from_slice(&crc.to_le_bytes());
            list.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            list.extend_from_slice(&claimed.to_le_bytes());
            list.extend_from_slice(&(entry.central_name.len() as u16).to_le_bytes());
            list.extend_from_slice(&[0; 8]);
            list.extend_from_slice(&(entry.mode << 16).to_le_bytes());
            list.extend_from_slice(&at.to_le_bytes());
            list.extend_from_slice(&entry.central_name);
        }
        let list_at = out.len() as u32;
        out.extend_from_slice(&list);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&(list.len() as u32).to_le_bytes());
        out.extend_from_slice(&list_at.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("recto-zip-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn check(zip: &[u8]) -> Result<Vec<Entry>, ZipError> {
        read_directory(&mut Cursor::new(zip), APP)
    }

    fn refused(zip: &[u8], what: &str) {
        match check(zip) {
            Err(ZipError::Bad(_)) => {}
            other => panic!("{what}: {other:?}"),
        }
    }

    /// Failure: a name that leaves the folder, or isn't part of the app, is taken. Plain-text rules, ahead of anything being unpacked.
    #[test]
    fn a_zips_names_have_to_be_the_apps() {
        for fine in [
            "RectoPDF.app/",
            "RectoPDF.app/Contents/MacOS/RectoPDF",
            "__MACOSX/RectoPDF.app/._x",
            "RectoPDF.app/Contents/a..b",
        ] {
            assert!(name_is_ours(fine, APP), "{fine}");
        }
        let deep = format!("RectoPDF.app/{}", "a/".repeat(40));
        for wrong in [
            "",
            "/etc/passwd",
            "../x",
            "RectoPDF.app/../../x",
            "RectoPDF.app/./x",
            "Other.app/x",
            "RectoPDF.app.evil/x",
            "RectoPDF.app\\..\\x",
            "RectoPDF.app/a\u{1}b",
            "RectoPDF.app/nl\nRectoPDF.app/second",
            "x",
            "RectoPDF.app//x",
            "RectoPDF.app/é",
            "RectoPDF.app/a:b",
            deep.as_str(),
        ] {
            assert!(!name_is_ours(wrong, APP), "{wrong}");
        }
    }

    /// Failure: a zip this reader takes unpacks to something other than what it listed. Each is what a review got past the system's tools: a header naming another path than the list does, a file the list doesn't mention, the list mentioning something twice, and a link. Nothing's written for any of them.
    #[test]
    fn what_a_zip_lists_is_what_it_holds() {
        let good = Made::file("RectoPDF.app/x", b"data");
        check(&make_zip(std::slice::from_ref(&good))).expect("the plain case");

        let mut other_name = good.clone();
        other_name.local_name = b"../../outside/local-escape".to_vec();
        refused(&make_zip(&[other_name]), "a header naming somewhere outside");
        let mut same_length = good.clone();
        same_length.local_name = b"RectoPDF.app/y".to_vec();
        refused(&make_zip(&[same_length]), "a header naming another file");
        let hidden = Made {
            unlisted: true,
            ..Made::file("RectoPDF.app/unlisted", b"y")
        };
        refused(&make_zip(&[good.clone(), hidden.clone()]), "a file the list doesn't mention");
        refused(&make_zip(&[hidden, good.clone()]), "and before the first it does");
        refused(&make_zip(&[good.clone(), good.clone()]), "one name twice");
        refused(
            &make_zip(&[good.clone(), Made::file("rectopdf.APP/X", b"z")]),
            "the same name, as this disk reads it",
        );
        refused(&make_zip(&[Made::file("RectoPDF.app/l", b"../../outside").with_mode(0o120_777)]), "a link");
        refused(&make_zip(&[Made::file("RectoPDF.app", b"/etc").with_mode(0o120_777)]), "the app itself a link");
        refused(
            &make_zip(&[
                Made::folder("RectoPDF.app/link/"),
                Made::file("RectoPDF.app/link", b"/etc").with_mode(0o120_777),
                Made::file("RectoPDF.app/link/pwned", b"p"),
            ]),
            "a link over a folder",
        );
        refused(
            &make_zip(&[Made::file("RectoPDF.app/f", b"x"), Made::file("RectoPDF.app/f/g", b"y")]),
            "a file that's also a folder",
        );
        refused(&make_zip(&[Made::file("RectoPDF.app/s", b"x").with_mode(0o060_777)]), "a device");
        refused(&make_zip(&[Made::file("/tmp/abs", b"x")]), "an absolute name");
        refused(&make_zip(&[Made::file("Evil.app/x", b"x")]), "another app");
        refused(b"this isn't a zip", "junk");
        refused(b"", "nothing");
        let mut trailing = make_zip(std::slice::from_ref(&good));
        trailing.extend_from_slice(b"trailing bytes");
        refused(&trailing, "bytes after its end");
        let mut prefixed = b"prefix".to_vec();
        prefixed.extend_from_slice(&make_zip(&[good]));
        refused(&prefixed, "bytes before its start");
    }

    /// Failure: a zip that says it's small unpacks to something enormous and fills the disk, which the system's `ditto` did, 50 MiB from a zip saying 1 KiB, with only a time limit on it. Here it's stopped as soon as it's written more than it said.
    #[test]
    fn a_zip_that_lies_about_its_size_stops_where_it_said() {
        let dir = folder("liar");
        let liar = Made {
            deflate: true,
            claimed_size: Some(1024),
            ..Made::file("RectoPDF.app/bomb", &vec![0u8; 50 * 1024 * 1024])
        };
        let zip = make_zip(&[liar]);
        assert!(zip.len() < 200 * 1024, "it's small as it comes: {}", zip.len());
        let entries = check(&zip).expect("its sizes alone don't show it");
        let far = Instant::now() + Duration::from_secs(60);
        let result = unpack_entries(&mut Cursor::new(&zip), &entries, &dir, far);
        assert!(matches!(result, Err(ZipError::Bad(_))), "{result:?}");
        let written = std::fs::metadata(dir.join("RectoPDF.app/bomb")).map_or(0, |meta| meta.len());
        assert!(written < 256 * 1024, "stopped soon after what it said: {written}");
        // And one that comes out smaller than it says, or with the wrong sum, isn't the file either.
        let short = Made {
            deflate: true,
            claimed_size: Some(100),
            ..Made::file("RectoPDF.app/s", b"short")
        };
        let bytes = make_zip(&[short]);
        let entries = check(&bytes).expect("it says the same twice");
        let elsewhere = dir.join("short");
        std::fs::create_dir_all(&elsewhere).unwrap();
        assert!(matches!(
            unpack_entries(&mut Cursor::new(&bytes), &entries, &elsewhere, far),
            Err(ZipError::Bad(_))
        ));
        let mut flipped = make_zip(&[Made::file("RectoPDF.app/f", b"data")]);
        let data_at = 30 + "RectoPDF.app/f".len();
        flipped[data_at] ^= 0xFF;
        let entries = check(&flipped).unwrap();
        let third = dir.join("flipped");
        std::fs::create_dir_all(&third).unwrap();
        assert!(
            matches!(unpack_entries(&mut Cursor::new(&flipped), &entries, &third, far), Err(ZipError::Bad(_))),
            "the wrong sum"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: a zip decides the modes of what it unpacks, so a re-zipped real build leaves the installed app writable by anyone, or setuid. They're ours, whatever it says. And nothing's written outside the folder.
    #[test]
    fn a_zip_doesnt_choose_its_modes() {
        let dir = folder("modes");
        let zip = make_zip(&[
            Made::folder("RectoPDF.app/d/").with_mode(0o040_777),
            Made::file("RectoPDF.app/d/open", b"#!/bin/sh\n").with_mode(0o106_777),
            Made::file("RectoPDF.app/d/plain", b"x").with_mode(0o100_666),
            Made::file("__MACOSX/RectoPDF.app/._x", b"not unpacked"),
        ]);
        let entries = check(&zip).unwrap();
        unpack_entries(&mut Cursor::new(&zip), &entries, &dir, Instant::now() + Duration::from_secs(10)).unwrap();
        let mode = |name: &str| std::fs::metadata(dir.join(name)).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode("RectoPDF.app/d/open"), 0o755 & !umask_bits());
        assert_eq!(mode("RectoPDF.app/d/plain"), 0o644 & !umask_bits());
        assert_eq!(mode("RectoPDF.app/d") & 0o022, 0, "no folder writable by others");
        assert!(!dir.join("__MACOSX").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn umask_bits() -> u32 {
        0o022
    }

    /// Failure: a damaged zip brings down the app or writes outside its folder. Every single-byte change and a good many cuts to a small real zip, through the whole of it.
    #[test]
    fn no_damage_to_a_zip_does_anything_but_refuse_it() {
        let good = make_zip(&[
            Made::folder("RectoPDF.app/"),
            Made {
                deflate: true,
                ..Made::file("RectoPDF.app/a", &[7u8; 2048])
            },
            Made::file("RectoPDF.app/b", b"plain"),
        ]);
        let far = Instant::now() + Duration::from_secs(120);
        let dir = folder("damage");
        let mut unpacked = 0;
        for at in 0..good.len() {
            for flip in [0x01u8, 0xFF] {
                let mut bytes = good.clone();
                bytes[at] ^= flip;
                if let Ok(entries) = check(&bytes) {
                    let into = dir.join(format!("{at}-{flip}"));
                    std::fs::create_dir_all(&into).unwrap();
                    let _ = unpack_entries(&mut Cursor::new(&bytes), &entries, &into, far);
                    unpacked += 1;
                    let _ = std::fs::remove_dir_all(&into);
                }
            }
        }
        for keep in (0..good.len()).step_by(7) {
            assert!(check(&good[..keep]).is_err(), "cut at {keep}");
        }
        assert!(unpacked < good.len(), "most of those weren't taken: {unpacked}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: a zip made the way a release is, by `ditto`, isn't what this reader takes, so no real update could ever unpack. With its file modes, folders and data descriptors.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_zip_from_ditto_unpacks_whole() {
        let dir = folder("ditto");
        let app = dir.join(APP);
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), "plist").unwrap();
        std::fs::write(app.join("Contents/MacOS/RectoPDF"), vec![9u8; 300_000]).unwrap();
        std::fs::set_permissions(app.join("Contents/MacOS/RectoPDF"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let zip = dir.join("r.zip");
        assert!(
            std::process::Command::new("/usr/bin/ditto")
                .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
                .arg(&app)
                .arg(&zip)
                .status()
                .unwrap()
                .success()
        );
        let into = dir.join("out");
        std::fs::create_dir_all(&into).unwrap();
        unpack(&zip, APP, &into, Instant::now() + Duration::from_secs(30)).unwrap();
        assert_eq!(std::fs::read(into.join("RectoPDF.app/Contents/MacOS/RectoPDF")).unwrap().len(), 300_000);
        assert_eq!(
            std::fs::metadata(into.join("RectoPDF.app/Contents/MacOS/RectoPDF"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(std::fs::read_to_string(into.join("RectoPDF.app/Contents/Info.plist")).unwrap(), "plist");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
