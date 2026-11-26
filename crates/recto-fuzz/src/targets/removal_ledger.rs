//! A hostile ledger file (`<state>/removal/ledger.v1`): read under small and standard limits it never panics or holds more than its caps, what was
//! read encodes and reads back the same, and a lineage over it never panics and never calls a ledger with a gap complete.

use recto_core::removal::{
    Entry, FileVersion, Gap, HEADER_LEN, LEDGER_LIMITS, Ledger, LedgerLimits, Link, MAX_LEDGER_BYTES, MAX_PATH_BYTES, encode, encode_header, lineage,
    read_ledger,
};
use std::path::{Path, PathBuf};

/// Limits small enough that a short input reaches every cap.
const SMALL: LedgerLimits = LedgerLimits {
    max_links: 6,
    max_path_bytes: 24,
    max_total_path_bytes: 64,
};

fn version(inode: u64, volume: bool) -> FileVersion {
    FileVersion {
        volume: volume.then_some([7; 16]),
        inode,
        size: 10,
        mtime_ns: 3,
    }
}

/// Valid ledgers: each kind of frame, a fork, a gap, a torn tail.
pub fn seeds() -> Vec<Vec<u8>> {
    let file = |entries: &[Entry]| {
        let mut bytes = encode_header(1_000).to_vec();
        for entry in entries {
            bytes.extend(encode(entry).unwrap());
        }
        bytes
    };
    let at = |inode, path: &str| {
        Entry::Link(Link::At {
            version: version(inode, true),
            path: path.into(),
        })
    };
    let saved = |from, to| {
        Entry::Link(Link::Saved {
            from: version(from, true),
            to: version(to, true),
        })
    };
    let mut torn = file(&[at(1, "/a.pdf"), saved(1, 2)]);
    torn.truncate(torn.len() - 3);
    vec![
        file(&[]),
        file(&[at(1, "/a.pdf"), saved(1, 2), at(2, "/b.pdf")]),
        file(&[saved(1, 2), saved(1, 3)]),
        file(&[Entry::Link(Link::Unplaced { path: "/u.pdf".into() }), Entry::Gap(Gap::Overflowed)]),
        file(&[Entry::Link(Link::At {
            version: version(4, false),
            path: "/n.pdf".into(),
        })]),
        torn,
    ]
}

/// The input as frames with their checksums made good, so it gets past them: each step is a kind, a length and that many bytes.
fn framed(bytes: &[u8]) -> Vec<u8> {
    let mut out = encode_header(i64::from(bytes.first().copied().unwrap_or(0))).to_vec();
    let mut rest = bytes.get(1..).unwrap_or_default();
    while let [kind, len, tail @ ..] = rest {
        let n = usize::from(*len).min(tail.len());
        let (body, after) = tail.split_at(n);
        out.extend(u16::try_from(n).unwrap_or(0).to_le_bytes());
        out.push(*kind % 6);
        out.extend(body);
        let mut crc = crc32fast::Hasher::new();
        crc.update(&[*kind % 6]);
        crc.update(body);
        out.extend(crc.finalize().to_le_bytes());
        rest = after;
    }
    out
}

/// What a read may and may not say.
fn held(ledger: &Ledger, limits: &LedgerLimits, input_len: usize) {
    assert!(ledger.links.len() <= limits.max_links as usize, "a cap on frames holds");
    assert!(ledger.good_len <= input_len.max(HEADER_LEN) && (ledger.good_len == 0 || ledger.good_len >= HEADER_LEN));
    let mut total = 0usize;
    for link in &ledger.links {
        if let Link::At { path, .. } | Link::Unplaced { path } = link {
            let len = path.as_os_str().as_encoded_bytes().len();
            assert!((1..=limits.max_path_bytes as usize).contains(&len) && len <= MAX_PATH_BYTES && path.is_absolute());
            total += len;
        }
    }
    for path in &ledger.destinations {
        let len = path.as_os_str().as_encoded_bytes().len();
        assert!((1..=limits.max_path_bytes as usize).contains(&len) && len <= MAX_PATH_BYTES && path.is_absolute());
        total += len;
    }
    assert!(total <= limits.max_total_path_bytes as usize, "a cap on paths holds");
    if input_len > MAX_LEDGER_BYTES {
        assert_eq!(ledger.gap, Some(Gap::Damaged));
    }
}

fn one(bytes: &[u8]) {
    for limits in [&SMALL, &LEDGER_LIMITS] {
        let ledger = read_ledger(bytes, limits);
        held(&ledger, limits, bytes.len());
        // The sound part of any file is a ledger of its own that reads whole: what a writer's cut back to `good_len` leaves.
        if ledger.good_len >= HEADER_LEN {
            let sound = read_ledger(&bytes[..ledger.good_len], limits);
            assert_eq!(
                (&sound.links, sound.stopped),
                (&ledger.links, None),
                "the sound part of a file reads whole and the same"
            );
        }
        // What was read encodes, and reads back the same, under the larger limits.
        let mut again = encode_header(ledger.began_ns).to_vec();
        for link in &ledger.links {
            again.extend(encode(&Entry::Link(link.clone())).expect("a link that was read encodes"));
        }
        for path in &ledger.destinations {
            again.extend(encode(&Entry::Destination(path.clone())).expect("a destination that was read encodes"));
        }
        let back = read_ledger(&again, &LEDGER_LIMITS);
        assert_eq!((&back.links, back.gap), (&ledger.links, None), "a ledger read encodes and reads back as it was");
        // Lineage over it: never a panic, and never complete when there is a gap.
        let current = ledger
            .links
            .iter()
            .find_map(|l| if let Link::At { version, .. } = l { Some(*version) } else { None })
            .unwrap_or(version(1, true));
        for path in [Path::new("/cur.pdf"), Path::new("/a.pdf")] {
            if let Ok(lineage) = lineage(&ledger, current, path, limits) {
                assert!(!(ledger.gap.is_some() && lineage.complete()), "a ledger with a gap is never complete");
                let _: Vec<(PathBuf, _)> = lineage.paths().map(|(p, s)| (p.to_path_buf(), s)).collect();
                let _ = lineage.versions().count();
            }
        }
    }
}

/// The check libFuzzer, the smoke run and the regression tests share.
pub fn check(bytes: &[u8]) {
    one(bytes);
    one(&framed(bytes));
}
