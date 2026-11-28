//! The names the Find text's folders use, restated here so the survey can tell what an entry is by its name alone (`docs/design/removal.md` §10 "D2d as built").
//! Pure: bytes in, a kind out. Nothing is opened. Every name below is written by `recto-index` (`corpus.rs`, `corpus/versions.rs`, `corpus/plain.rs`,
//! `corpus/agent_life.rs`, `generated.rs`) and is pinned against the real writer by `pins.rs`: a writer that renames one fails there, not in a user's folder.

use recto_index::corpus::CorpusKey;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;

/// The folder of a file's version notes, in the corpus root.
pub(super) const VERSIONS: &str = "by-file";
/// The two markers in the corpus root: text from before it was sealed was looked for, and an agent's folder may be there to sweep.
pub(super) const MARKERS: [&str; 2] = ["sealed", "agent-folders"];
/// The older build's folder of plain text, beside a corpus root named [`CORPUS_ROOT_NAME`].
pub(super) const OLD_FOLDER: &str = "corpus";
/// The corpus root's own name, which is what the older folder is beside.
pub(super) const CORPUS_ROOT_NAME: &str = "corpus.noindex";
/// A corpus's format folder, which the clear-all and the purge open for its lock files (`corpus::FORMAT`; pinned by `pins.rs`).
pub(super) const FORMAT_FOLDER: &str = "v4";
/// The folder of what a search reads, in a corpus's format folder, and the lock a build holds in it for as long as it builds.
pub(super) const APP_FOLDER: &str = "app";
pub(super) const BUILD_LOCK: &str = "build.lock";
/// An agent's folder's lock, held by its session, and the file that says it is finished.
pub(super) const SESSION_FILE: &str = ".session";
pub(super) const MANIFEST: &str = "manifest.json";
/// What makes a corpus usable: written last, so the first thing the purge takes out of a corpus, and made durable before any other (`ready.json` is the same
/// mark of the older format, which no writer makes any more, so it isn't pinned: only the order depends on it).
pub(super) const READY: [&str; 2] = ["ready.sealed", "ready.json"];
/// The first thing to go from a corpus's tree, which makes it unusable (`rank` 0 of the purge's walk).
pub(super) fn is_first_to_go(name: &OsStr) -> bool {
    READY.iter().any(|ready| name == *ready) || name == MANIFEST
}

/// Most keys one alias file holds, and the most bytes it is read for (a key and its line break).
pub(super) const MAX_ALIAS_KEYS: usize = 32;
pub(super) const ALIAS_BYTES: usize = MAX_ALIAS_KEYS * 65;

/// `<digits>` of a name ending in `.tmp<digits>`: what a write's temporary is called (`with_extension("tmp<pid>")`).
fn temp_of(rest: &str) -> bool {
    rest.strip_prefix("tmp")
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
}

/// `name` is `stem` or `stem.tmp<digits>`.
fn is_stem_or_temp(name: &str, stem: &str) -> bool {
    name == stem || name.strip_prefix(stem).and_then(|rest| rest.strip_prefix('.')).is_some_and(temp_of)
}

fn key_text(text: &str) -> Option<CorpusKey> {
    CorpusKey::from_hex(text)
}

/// The hash a path's alias is named by: `hex(SHA-256("path" 0x00 path bytes))`.
pub(super) fn path_alias(path: &[u8]) -> String {
    alias_of(b"path", path)
}

/// The hash an inode's alias is named by: `hex(SHA-256("inode" 0x00 device LE inode LE))`.
pub(super) fn inode_alias(device: u64, inode: u64) -> String {
    let mut bytes = device.to_le_bytes().to_vec();
    bytes.extend_from_slice(&inode.to_le_bytes());
    alias_of(b"inode", &bytes)
}

fn alias_of(kind: &[u8], bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(kind);
    hash.update([0]);
    hash.update(bytes);
    hex(&hash.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The start of a generated-contents name for a document at `path`: the first 8 bytes of `SHA-256(path bytes)` in lowercase hex.
pub(super) fn contents_tag(path: &[u8]) -> String {
    hex(&Sha256::digest(path)[..8])
}

/// What a name in the corpus root is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RootName {
    /// A corpus's folder: 64 lowercase hex.
    Key(CorpusKey),
    /// `sealed` or `agent-folders`, or the temporary of a write of one.
    Marker,
    /// The folder of version notes: surveyed by its own pass.
    Versions,
    /// Anything else, a name that isn't text included.
    Other,
}

pub(super) fn root_name(name: &OsStr) -> RootName {
    let Some(name) = name.to_str() else { return RootName::Other };
    if let Some(key) = key_text(name) {
        RootName::Key(key)
    } else if name == VERSIONS {
        RootName::Versions
    } else if MARKERS.iter().any(|marker| is_stem_or_temp(name, marker)) {
        RootName::Marker
    } else {
        RootName::Other
    }
}

/// The 64 hex a version note is named by, for `<hex>` and `<hex>.tmp<digits>`.
pub(super) fn alias_stem(name: &OsStr) -> Option<&str> {
    let name = name.to_str()?;
    let stem = name.get(..64)?;
    (key_text(stem).is_some() && is_stem_or_temp(name, stem)).then_some(stem)
}

/// A generated-contents name: `<tag16>.<key64>.json`, or `<tag16>.<key64>.tmp<digits>` while it is written. The tag (lowercase hex) and the key.
pub(super) fn contents_name(name: &OsStr) -> Option<(&str, CorpusKey)> {
    let name = name.to_str()?;
    let (tag, rest) = name.split_once('.')?;
    if tag.len() != 16 || !tag.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
        return None;
    }
    let key = key_text(rest.get(..64)?)?;
    let tail = rest.get(64..)?.strip_prefix('.')?;
    (tail == "json" || temp_of(tail)).then_some((tag, key))
}

/// An agent's folder in a corpus's format folder: `agent`, or one set aside, which `set_aside` finds by the start `agent.gone`.
pub(super) fn is_agent_folder(name: &OsStr) -> bool {
    name == "agent" || name.as_encoded_bytes().starts_with(b"agent.gone")
}

/// The keys the app itself reads out of a version note (`corpus/versions.rs`, `read_alias`): every line that is a key, whatever else is in the file, and none
/// when it is over the cap. What `index clear` of a file would remove.
pub(super) fn keys_the_app_reads(bytes: &[u8]) -> Vec<CorpusKey> {
    if bytes.len() > ALIAS_BYTES {
        return Vec::new();
    }
    String::from_utf8_lossy(bytes).lines().filter_map(key_text).collect()
}

/// The keys of a version note that is exactly what the app writes: 1 to 32 keys, one a line, no empty line, no carriage return, no line break at the end, none twice.
/// Anything else is not read by this build's own rule and is `None` (`Format::Foreign`).
pub(super) fn keys_strictly(bytes: &[u8]) -> Option<Vec<CorpusKey>> {
    if bytes.is_empty() || bytes.len() > ALIAS_BYTES {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut keys = Vec::new();
    for line in text.split('\n') {
        let key = key_text(line)?;
        if keys.len() == MAX_ALIAS_KEYS || keys.contains(&key) {
            return None;
        }
        keys.push(key);
    }
    Some(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> String {
        format!("{byte:02x}").repeat(32)
    }

    /// Failure: a name the writers make is read as something else, so a corpus's folder, its note or its marker is skipped as "none found" or called another
    /// document's; or a name nobody writes (an uppercase key, a stray) is taken for ours.
    #[test]
    fn the_names_the_writers_make_have_their_kinds() {
        let k = key(0xab);
        for (name, want) in [
            (k.as_str(), RootName::Key(CorpusKey::from_hex(&k).unwrap())),
            ("by-file", RootName::Versions),
            ("sealed", RootName::Marker),
            ("agent-folders", RootName::Marker),
            ("sealed.tmp123", RootName::Marker),
            ("agent-folders.tmp9", RootName::Marker),
            ("sealed.tmp", RootName::Other),
            ("sealed.tmp1x", RootName::Other),
            ("sealed.bak", RootName::Other),
            ("Sealed", RootName::Other),
            ("by-file.tmp1", RootName::Other),
            ("stray", RootName::Other),
            ("", RootName::Other),
        ] {
            assert_eq!(root_name(OsStr::new(name)), want, "{name:?}");
        }
        let upper = k.to_uppercase();
        assert_eq!(root_name(OsStr::new(&upper)), RootName::Other, "uppercase hex");
        assert_eq!(root_name(OsStr::new(&format!("{k}.tmp1"))), RootName::Other, "a key's temp isn't a key");
        assert_eq!(root_name(OsStr::new(&k[1..])), RootName::Other, "63 digits");
    }

    /// Failure: a version note's temporary is left out of the survey (its keys are still on disk), or a stray name is read as a note.
    #[test]
    fn a_version_note_is_its_hash_or_the_hash_and_a_temp() {
        let k = key(0xab);
        assert_eq!(alias_stem(OsStr::new(&k)), Some(k.as_str()));
        assert_eq!(alias_stem(OsStr::new(&format!("{k}.tmp77"))), Some(k.as_str()));
        for bad in [
            format!("{k}.tmp"),
            format!("{k}.json"),
            format!("{k}x"),
            k.to_uppercase(),
            k[2..].to_string(),
            format!("{k}.tmp7.tmp8"),
        ] {
            assert_eq!(alias_stem(OsStr::new(&bad)), None, "{bad:?}");
        }
    }

    /// Failure: a kept contents file is taken for another thing, so its headings stay when its document is removed.
    #[test]
    fn a_contents_name_is_tag_key_and_json_or_a_temp() {
        let k = key(0xcd);
        let tag = "0123456789abcdef";
        let ok = CorpusKey::from_hex(&k).unwrap();
        assert_eq!(contents_name(OsStr::new(&format!("{tag}.{k}.json"))), Some((tag, ok)));
        assert_eq!(contents_name(OsStr::new(&format!("{tag}.{k}.tmp42"))), Some((tag, ok)));
        for bad in [
            format!("{}.{k}.json", tag.to_uppercase()),
            format!("{}.{k}.json", &tag[1..]),
            format!("{tag}.{k}.json.bak"),
            format!("{tag}.{k}.tmp"),
            format!("{tag}.{}.json", k.to_uppercase()),
            format!("{tag}.{k}"),
            format!("{tag}{k}.json"),
        ] {
            assert_eq!(contents_name(OsStr::new(&bad)), None, "{bad:?}");
        }
    }

    /// Failure: a set-aside agent folder isn't found, or `agentx` is.
    #[test]
    fn an_agent_folder_is_agent_or_set_aside() {
        for (name, want) in [
            ("agent", true),
            ("agent.gone.12.00ff", true),
            ("agent.gone", true),
            ("agentx", false),
            ("Agent", false),
            ("app", false),
        ] {
            assert_eq!(is_agent_folder(OsStr::new(name)), want, "{name}");
        }
    }

    /// Failure: the survey calls a note Current that this build would not write, so a rewrite of it later is trusted; or reads a good one as foreign.
    #[test]
    fn a_note_is_strict_about_its_shape() {
        let (a, b) = (key(1), key(2));
        assert_eq!(keys_strictly(a.as_bytes()).map(|k| k.len()), Some(1));
        assert_eq!(keys_strictly(format!("{a}\n{b}").as_bytes()).map(|k| k.len()), Some(2));
        let many = |n: u8| (0..n).map(key).collect::<Vec<_>>().join("\n");
        assert_eq!(keys_strictly(many(32).as_bytes()).map(|k| k.len()), Some(32));
        for (what, bytes) in [
            ("33 keys", many(33).into_bytes()),
            ("a bad line", format!("{a}\nnot a key").into_bytes()),
            ("crlf", format!("{a}\r\n{b}").into_bytes()),
            ("trailing line break", format!("{a}\n").into_bytes()),
            ("empty line", format!("{a}\n\n{b}").into_bytes()),
            ("a duplicate", format!("{a}\n{a}").into_bytes()),
            ("empty", Vec::new()),
            ("not text", vec![0xff; 64]),
            ("over the byte cap", vec![b'a'; ALIAS_BYTES + 1]),
        ] {
            assert_eq!(keys_strictly(&bytes), None, "{what}");
        }
    }

    /// Failure: the target's keys are read more strictly than the app's own clear reads them, so a note the app would clear is left (a leak), or less strictly, so
    /// a file that is not a note adds keys. The app's reader takes each line that is a key and nothing past the cap.
    #[test]
    fn the_targets_keys_are_read_as_the_app_reads_them() {
        let (a, b) = (key(1), key(2));
        let n = |bytes: &[u8]| keys_the_app_reads(bytes).len();
        assert_eq!(
            n(format!("{a}\r\n{b}\n").as_bytes()),
            2,
            "carriage returns and a trailing break are lines' ends"
        );
        assert_eq!(n(format!("{a}\nnoise\n{a}").as_bytes()), 2, "a bad line is skipped, a duplicate is read");
        assert_eq!(n(&vec![b'a'; ALIAS_BYTES + 1]), 0);
        assert_eq!(n(&[0xff, 0xfe]), 0);
    }
}
