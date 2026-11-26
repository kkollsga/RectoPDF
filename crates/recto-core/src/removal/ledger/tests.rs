use super::*;
use crate::removal::LEDGER_LIMITS;
use std::path::PathBuf;

const A: [u8; 16] = [0xA; 16];
/// A plausible beginning (2027-01-15).
const BEGAN: i64 = 1_800_000_000_000_000_000;

fn version(inode: u64) -> FileVersion {
    FileVersion {
        volume: Some(A),
        inode,
        size: 100,
        mtime_ns: 5,
    }
}

fn at(inode: u64, path: &str) -> Link {
    Link::At {
        version: version(inode),
        path: PathBuf::from(path),
    }
}

fn file_of(entries: &[Entry]) -> Vec<u8> {
    let mut bytes = encode_header(BEGAN).to_vec();
    for e in entries {
        bytes.extend(encode(e).unwrap());
    }
    bytes
}

fn sample() -> Vec<Entry> {
    vec![
        Entry::Link(at(7, "/docs/a.pdf")),
        Entry::Link(Link::Saved {
            from: version(7),
            to: FileVersion {
                volume: None,
                inode: 7,
                size: 120,
                mtime_ns: -3,
            },
        }),
        Entry::Link(Link::Unplaced { path: "/docs/b.pdf".into() }),
        Entry::Gap(Gap::BackupMismatch),
    ]
}

/// Failure: what is written is not what is read back (a field misplaced, a path or a version mangled), so a removal places backups wrongly.
#[test]
fn what_is_encoded_reads_back() {
    let bytes = file_of(&sample());
    let ledger = read(&bytes, &LEDGER_LIMITS);
    let links: Vec<Entry> = ledger.links.iter().cloned().map(Entry::Link).collect();
    assert_eq!(links, sample()[..3]);
    assert_eq!((ledger.began_ns, ledger.gap, ledger.good_len), (BEGAN, Some(Gap::BackupMismatch), bytes.len()));
}

/// Failure: the format drifts without anyone choosing it, and an older build can't read the file a newer one wrote (or the reverse): the
/// exact bytes of the header and of one frame of each kind are pinned.
#[test]
fn golden_bytes() {
    let header = encode_header(0x0102);
    let mut expect = b"RECTOLEDGER\0".to_vec();
    expect.extend([1, 0, 0, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    expect.extend(crc32fast::hash(&expect).to_le_bytes());
    assert_eq!(header.to_vec(), expect);
    assert_eq!(&expect[..32], &header[..32]);

    let gap = encode(&Entry::Gap(Gap::Torn)).unwrap();
    assert_eq!(gap[..4], [1, 0, 4, 4]);
    assert_eq!(gap[4..], crc32fast::hash(&[4, 4]).to_le_bytes());

    let frame = encode(&Entry::Link(Link::Unplaced { path: "/x".into() })).unwrap();
    assert_eq!(frame[..7], [4, 0, 3, 2, 0, b'/', b'x']);
    assert_eq!(frame[7..], crc32fast::hash(&[3, 2, 0, b'/', b'x']).to_le_bytes());

    let frame = encode(&Entry::Link(at(7, "/p"))).unwrap();
    let mut body = vec![1];
    body.extend(A);
    body.extend(7u64.to_le_bytes());
    body.extend(100u64.to_le_bytes());
    body.extend(5i64.to_le_bytes());
    body.extend([2, 0, b'/', b'p']);
    let mut whole = vec![u8::try_from(body.len()).unwrap(), 0, 1];
    whole.extend(&body);
    let mut crc_input = vec![1];
    crc_input.extend(&body);
    whole.extend(crc32fast::hash(&crc_input).to_le_bytes());
    assert_eq!(frame, whole);
}

/// Failure: a crash in the middle of an append loses the links before it, or is mistaken for damage, so the ledger is thrown away for the
/// loss of its last half-written frame. Every cut inside the last frame keeps the earlier links, says `Torn`, and `good_len` is where to cut.
#[test]
fn a_torn_tail_keeps_the_links_before_it() {
    let whole = file_of(&sample());
    let before_last = file_of(&sample()[..3]).len();
    for cut in before_last + 1..whole.len() {
        let ledger = read(&whole[..cut], &LEDGER_LIMITS);
        assert_eq!(ledger.links.len(), 3, "cut at {cut}");
        assert_eq!(ledger.gap, Some(Gap::Torn), "cut at {cut}");
        assert_eq!(ledger.good_len, before_last);
    }
    // A last frame whole in length but with a bad checksum is a torn write too.
    let mut bad = whole.clone();
    *bad.last_mut().unwrap() ^= 1;
    assert_eq!(read(&bad, &LEDGER_LIMITS).gap, Some(Gap::Torn));
    // A file cut inside the header has nothing to keep.
    assert_eq!(read(&whole[..10], &LEDGER_LIMITS).gap, Some(Gap::Torn));
}

/// Failure: a body byte flipped in a frame that has frames after it is read as a torn tail (and so repaired by cutting off everything after
/// it, which loses good links), or its later links are trusted.
#[test]
fn a_bad_checksum_mid_file_is_damage() {
    let mut bytes = file_of(&sample());
    bytes[HEADER_LEN + 6] ^= 1;
    let ledger = read(&bytes, &LEDGER_LIMITS);
    assert_eq!(ledger.gap, Some(Gap::Damaged));
    assert_eq!((ledger.links.len(), ledger.good_len), (0, HEADER_LEN));
}

/// Failure: a ledger made by a newer build is read as damaged and written to (its newer frames cut), or read as if it were version 1.
#[test]
fn a_newer_version_is_too_new() {
    let mut bytes = file_of(&sample());
    bytes[12] = 2;
    let crc = crc32fast::hash(&bytes[..32]);
    bytes[32..36].copy_from_slice(&crc.to_le_bytes());
    let ledger = read(&bytes, &LEDGER_LIMITS);
    assert_eq!((ledger.gap, ledger.links.len()), (Some(Gap::TooNew), 0));
}

/// Failure: a header that is not ours (a wrong magic, a bad checksum, nonzero reserved bytes) is taken as a ledger.
#[test]
fn a_bad_header_is_damage() {
    for at in [0, 17, 25, 33] {
        let mut bytes = file_of(&sample());
        bytes[at] ^= 1;
        assert_eq!(read(&bytes, &LEDGER_LIMITS).gap, Some(Gap::Damaged), "byte {at}");
    }
}

/// Failure: an input of any shape makes the reader panic or read out of bounds. Every prefix and every single-byte change of a real ledger.
#[test]
fn nothing_read_ever_panics() {
    let bytes = file_of(&sample());
    for cut in 0..=bytes.len() {
        let _ = read(&bytes[..cut], &LEDGER_LIMITS);
    }
    for i in 0..bytes.len() {
        for flip in [1u8, 0x80, 0xFF] {
            let mut b = bytes.clone();
            b[i] ^= flip;
            let _ = read(&b, &LEDGER_LIMITS);
        }
    }
    let _ = read(&[0xFF; 300], &LEDGER_LIMITS);
}

/// A frame with a valid checksum around `body`.
fn forged(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut bytes = encode_header(1).to_vec();
    bytes.extend(u16::try_from(body.len()).unwrap().to_le_bytes());
    bytes.push(kind);
    bytes.extend(body);
    bytes.extend(checksum(kind, body).to_le_bytes());
    bytes
}

fn path_body(path: &[u8]) -> Vec<u8> {
    let mut body = u16::try_from(path.len()).unwrap().to_le_bytes().to_vec();
    body.extend(path);
    body
}

/// Failure: a frame that is sound as bytes but means nothing (a relative path, a NUL, an empty or over-long path, a volume flag over a zero
/// UUID, an unknown kind or gap reason, trailing bytes) is trusted, and a hostile ledger names a path or a volume that was never recorded.
#[test]
fn invalid_fields_are_damage_without_a_panic() {
    let ok = read(&forged(3, &path_body(b"/ok")), &LEDGER_LIMITS);
    assert_eq!((ok.gap, ok.links.len()), (None, 1));
    let long = vec![b'a'; MAX_PATH_BYTES];
    let mut absolute_long = vec![b'/'];
    absolute_long.extend(&long);
    let mut zero_flagged = vec![1];
    zero_flagged.extend([0u8; 16 + 24]);
    zero_flagged.extend(path_body(b"/p"));
    let mut unflagged_volume = vec![0];
    unflagged_volume.extend([9u8; 16]);
    unflagged_volume.extend([0u8; 24]);
    unflagged_volume.extend(path_body(b"/p"));
    let mut trailing = path_body(b"/ok");
    trailing.push(0);
    let bad: [(u8, Vec<u8>); 10] = [
        (3, path_body(b"relative")),
        (3, path_body(b"/a\0b")),
        (3, path_body(b"")),
        (3, path_body(&absolute_long)),
        (3, trailing),
        (3, vec![5, 0, b'/']),
        (1, zero_flagged),
        (1, unflagged_volume),
        (9, vec![1]),
        (4, vec![200]),
    ];
    for (kind, body) in bad {
        let ledger = read(&forged(kind, &body), &LEDGER_LIMITS);
        assert_eq!((ledger.gap, ledger.links.len()), (Some(Gap::Damaged), 0), "kind {kind}");
    }
    // A length over the most any frame holds is refused before it is read.
    let mut huge = encode_header(1).to_vec();
    huge.extend([0xFF, 0xFF, 3]);
    assert_eq!(read(&huge, &LEDGER_LIMITS).gap, Some(Gap::Damaged));
    // And the encoder refuses what the reader would.
    for path in ["relative", "/a\0b", ""] {
        assert!(encode(&Entry::Link(Link::Unplaced { path: path.into() })).is_none());
    }
    let zero = FileVersion {
        volume: Some([0; 16]),
        ..version(1)
    };
    assert!(encode(&Entry::Link(Link::Saved { from: zero, to: zero })).is_none());
}

/// Failure: a hostile or runaway ledger makes the reader hold or hash without bound: each cap passes at its limit and is damage at one over,
/// with small limits, and a file over the byte cap is not looked at.
#[test]
fn each_cap_passes_at_the_limit_and_fails_one_over() {
    let small = LedgerLimits {
        max_links: 3,
        max_path_bytes: 4,
        max_total_path_bytes: 8,
    };
    let gap_of = |entries: &[Entry]| {
        let ledger = read(&file_of(entries), &small);
        (ledger.gap, ledger.links.len())
    };
    let unplaced = |p: &str| Entry::Link(Link::Unplaced { path: p.into() });
    let (three, four) = (
        [unplaced("/a"), unplaced("/b"), unplaced("/c")],
        [unplaced("/a"), unplaced("/b"), unplaced("/c"), unplaced("/d")],
    );
    assert_eq!(gap_of(&three), (None, 3));
    assert_eq!(gap_of(&four), (Some(Gap::Damaged), 3));
    // Gap frames count toward the frames too.
    assert_eq!(gap_of(&[Entry::Gap(Gap::Torn), unplaced("/a"), unplaced("/b")]).0, Some(Gap::Torn));
    assert_eq!(
        gap_of(&[Entry::Gap(Gap::Torn), Entry::Gap(Gap::Torn), Entry::Gap(Gap::Torn), Entry::Gap(Gap::Torn)]).1,
        0
    );
    assert_eq!(gap_of(&[unplaced("/abc")]), (None, 1));
    assert_eq!(gap_of(&[unplaced("/abcd")]), (Some(Gap::Damaged), 0));
    assert_eq!(gap_of(&[unplaced("/abc"), unplaced("/abc")]), (None, 2));
    assert_eq!(gap_of(&[unplaced("/abc"), unplaced("/abc"), unplaced("/a")]), (Some(Gap::Damaged), 2));
    let over = vec![0u8; MAX_LEDGER_BYTES + 1];
    assert_eq!(read(&over, &LEDGER_LIMITS).gap, Some(Gap::Damaged));
}

/// Failure: a ledger full at every cap does not fit under the file's size cap, so a legitimate full ledger is refused as damaged and
/// every removal on that machine refuses until it is rotated.
#[test]
fn a_ledger_full_at_every_cap_fits_under_the_file_cap() {
    let l = LEDGER_LIMITS;
    let longest = |n: usize| PathBuf::from(format!("/{}", "a".repeat(n - 1)));
    // Frames carrying paths, the 1 MiB of paths spread over as few frames as the path cap allows, and the rest `Saved` (the widest frame).
    let (paths, rest) = (l.max_total_path_bytes as usize / l.max_path_bytes as usize, l.max_links as usize - 1);
    let mut size = HEADER_LEN;
    for _ in 0..paths {
        size += encode(&Entry::Link(Link::Unplaced { path: longest(MAX_PATH_BYTES) })).unwrap().len();
    }
    let saved = Entry::Link(Link::Saved {
        from: version(1),
        to: version(2),
    });
    size += (rest - paths) * encode(&saved).unwrap().len();
    size += encode(&Entry::Gap(Gap::Overflowed)).unwrap().len();
    assert!(size <= MAX_LEDGER_BYTES, "{size}");
    // The other extreme: every frame a short path, with the total path budget used up.
    let each = l.max_total_path_bytes as usize / rest;
    let frame = encode(&Entry::Link(at(1, longest(each).to_str().unwrap()))).unwrap().len();
    assert!(HEADER_LEN + rest * frame + 11 <= MAX_LEDGER_BYTES);
    // And a real ledger of 4,095 `Saved` frames and the gap is read whole.
    let mut bytes = encode_header(1).to_vec();
    for _ in 0..rest {
        bytes.extend(encode(&saved).unwrap());
    }
    bytes.extend(encode(&Entry::Gap(Gap::Overflowed)).unwrap());
    let ledger = read(&bytes, &l);
    assert_eq!((ledger.links.len(), ledger.gap), (rest, Some(Gap::Overflowed)));
}

/// Failure: a frame length flipped mid-file past the end of the file reads as a cut-short tail (`Torn`), and the writer, which repairs a torn
/// tail by cutting back to `good_len`, throws away the good frames after it. A length that can't be its kind's is damage, whatever the file's size.
#[test]
fn a_length_flipped_mid_file_is_damage_not_a_torn_tail() {
    let bytes = file_of(&sample());
    // The first frame's length, then the later frames' (each frame's length is two bytes at the frame's start).
    let mut at = HEADER_LEN;
    while at < bytes.len() {
        let len = usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]));
        let mut flipped = bytes.clone();
        flipped[at + 1] ^= 0x7F;
        let ledger = read(&flipped, &LEDGER_LIMITS);
        assert_eq!((ledger.stopped, ledger.good_len), (Some(Gap::Damaged), at), "length at {at}");
        at += 3 + len + 4;
    }
    // A length that is another valid size of the kind (a longer path than the bytes say) is damage as well.
    let mut one_off = bytes.clone();
    one_off[HEADER_LEN] += 1;
    assert_eq!(read(&one_off, &LEDGER_LIMITS).stopped, Some(Gap::Damaged));
}

/// Failure: why the read stopped is hidden by a gap an earlier frame recorded, so a damaged file looks like a merely gapped one and the writer
/// appends to it. `stopped` is the stop alone; `gap` is the first reason of any.
#[test]
fn why_the_read_stopped_is_not_hidden_by_an_earlier_gap() {
    let mut entries = vec![Entry::Gap(Gap::BackupMismatch)];
    entries.extend(sample().into_iter().take(2));
    let mut bytes = file_of(&entries);
    let ok = read(&bytes, &LEDGER_LIMITS);
    assert_eq!((ok.gap, ok.stopped), (Some(Gap::BackupMismatch), None), "a frame's gap is not a stop");
    bytes[HEADER_LEN + 3 + 1 + 4 + 6] ^= 1;
    let damaged = read(&bytes, &LEDGER_LIMITS);
    assert_eq!((damaged.gap, damaged.stopped), (Some(Gap::BackupMismatch), Some(Gap::Damaged)));
    let torn = read(&bytes[..bytes.len() - 2], &LEDGER_LIMITS);
    assert_eq!(torn.stopped, Some(Gap::Damaged), "the damage comes first");
    let cut = read(&file_of(&entries)[..file_of(&entries).len() - 2], &LEDGER_LIMITS);
    assert_eq!((cut.gap, cut.stopped), (Some(Gap::BackupMismatch), Some(Gap::Torn)));
}

/// Failure: a file of ten bytes of anything is called a torn ledger, and the writer, which repairs a torn tail, writes a header over it.
/// Only a true prefix of the header (an empty file included) is torn.
#[test]
fn a_short_file_is_torn_only_if_it_starts_like_a_ledger() {
    let header = encode_header(BEGAN);
    for cut in 0..HEADER_LEN {
        assert_eq!(read(&header[..cut], &LEDGER_LIMITS).stopped, Some(Gap::Torn), "{cut} bytes of a header");
    }
    for garbage in [&b"0123456789"[..], b"RECTOLEDGEX", b"\0\0\0"] {
        assert_eq!(read(garbage, &LEDGER_LIMITS).stopped, Some(Gap::Damaged), "{garbage:?}");
    }
}

/// Failure: a header with a time of zero, or from before this code existed, makes every backup read as made after the ledger began, so one
/// made before anything was recorded is placed by a path the ledger only saw later.
#[test]
fn an_implausible_beginning_makes_everything_older() {
    for began in [0, -5, 1_000, 1_767_225_599_999_999_999] {
        let ledger = read(&encode_header(began), &LEDGER_LIMITS);
        assert_eq!((ledger.gap, ledger.began_ns), (None, i64::MAX), "began {began}");
    }
    let ledger = read(&encode_header(1_767_225_600_000_000_000), &LEDGER_LIMITS);
    assert_eq!(ledger.began_ns, 1_767_225_600_000_000_000);
}
