use super::*;
use crate::removal::{Gap, Ledger};

const VOL: [u8; 16] = [1; 16];

fn stamp(inode: u64, size: u64, mtime_ns: i64) -> FileVersion {
    FileVersion {
        volume: Some(VOL),
        inode,
        size,
        mtime_ns,
    }
}

fn at(version: FileVersion, path: &str) -> Link {
    Link::At {
        version,
        path: PathBuf::from(path),
    }
}

fn ledger(links: &[Link]) -> Ledger {
    Ledger {
        links: links.to_vec(),
        began_ns: 0,
        gap: None,
        stopped: None,
        good_len: 0,
        frames: 0,
        overflowed: false,
    }
}

fn sides_of(l: &Lineage<'_>) -> Vec<(String, Side)> {
    l.paths().map(|(p, s)| (p.display().to_string(), s)).collect()
}

/// Failure: a file that was renamed, or saved over by this app, is not its own history, so its backup under the old path is left behind (or
/// the plan refuses the file as another document's).
#[test]
fn a_rename_and_a_recto_save_stay_the_documents() {
    let (s0, s1) = (stamp(7, 100, 1), stamp(7, 120, 2));
    let links = [at(s0, "/p1"), at(s0, "/p2"), Link::Saved { from: s0, to: s1 }];
    let current = Path::new("/p2");
    let led = ledger(&links);
    let l = lineage(&led, s1, current, &LEDGER_LIMITS).unwrap();
    assert_eq!(
        sides_of(&l),
        [("/p2".into(), Side::Target), ("/p1".into(), Side::Target), ("/p2".into(), Side::Target)]
    );
    assert!(l.versions().all(|(_, side)| side == Side::Target));
    assert_eq!(l.versions().count(), 4);
}

/// Failure: the history of a `cp -p` copy (same size and time, another inode) is purged with this document's.
#[test]
fn a_copy_is_another_document() {
    let copy = stamp(8, 100, 1);
    let links = [at(copy, "/copy.pdf")];
    let led = ledger(&links);
    let l = lineage(&led, stamp(7, 100, 1), Path::new("/doc.pdf"), &LEDGER_LIMITS).unwrap();
    assert_eq!(sides_of(&l), [("/doc.pdf".into(), Side::Target), ("/copy.pdf".into(), Side::Other)]);
}

/// Failure: a different file written at the same path by another program is taken for this document by its path alone, and its original
/// backup is purged.
#[test]
fn a_file_changed_elsewhere_at_the_same_path_is_unknown() {
    let links = [at(stamp(7, 100, 1), "/p.pdf")];
    let led = ledger(&links);
    let l = lineage(&led, stamp(9, 50, 5), Path::new("/p.pdf"), &LEDGER_LIMITS).unwrap();
    let mut sides = Sides::default();
    for (path, side) in l.paths() {
        assert_eq!(path, Path::new("/p.pdf"));
        sides = sides.with(side);
    }
    assert_eq!(sides.owner(TargetMode::Save), Owner::Unknown);
}

/// Failure: unrelated files whose inode is unknown (zero) look the same and are joined into one document.
#[test]
fn inode_zero_joins_nothing() {
    let zero = stamp(0, 100, 1);
    let links = [at(zero, "/a"), Link::Saved { from: zero, to: zero }];
    let led = ledger(&links);
    let l = lineage(&led, zero, Path::new("/cur"), &LEDGER_LIMITS).unwrap();
    assert_eq!(sides_of(&l), [("/cur".into(), Side::Target), ("/a".into(), Side::Other)]);
    assert!(l.versions().all(|(_, side)| side == Side::Other));
}

/// Failure: an artifact nobody can place becomes a guess, or Save As deletes the source's history.
#[test]
fn sides_decide_the_owner() {
    let none = Sides::default();
    let target = none.with(Side::Target);
    let other = none.with(Side::Other);
    let both = target.with(Side::Other);
    for (sides, save, save_as) in [
        (none, Owner::Unknown, Owner::Unknown),
        (target, Owner::Target, Owner::SaveAsSource),
        (other, Owner::Unrelated, Owner::Unrelated),
        (both, Owner::Unknown, Owner::Unknown),
    ] {
        assert_eq!(sides.owner(TargetMode::Save), save);
        assert_eq!(sides.owner(TargetMode::SaveAs), save_as);
    }
}

/// Failure: a hostile or runaway ledger makes the survey hold or hash without bound. Each cap passes at its limit and refuses at one over.
#[test]
fn ledger_caps_refuse_at_limit_plus_one() {
    let s = stamp(1, 1, 1);
    let small = LedgerLimits {
        max_links: 2,
        max_path_bytes: 4,
        max_total_path_bytes: 10,
    };
    let run = |links: &[Link], current: &str| lineage(&ledger(links), s, Path::new(current), &small).map(|_| ());
    let link = |p: &str| at(s, p);
    assert_eq!(run(&[link("a"), link("b")], "c"), Ok(()));
    assert_eq!(run(&[link("a"), link("b"), link("c")], "d"), Err(Refusal::TooLarge));
    assert_eq!(run(&[link("abcd")], "c"), Ok(()));
    assert_eq!(run(&[link("abcde")], "c"), Err(Refusal::TooLarge));
    assert_eq!(run(&[], "abcd"), Ok(()));
    assert_eq!(run(&[], "abcde"), Err(Refusal::TooLarge));
    assert_eq!(run(&[link("abcd"), link("abcd")], "ab"), Ok(()));
    assert_eq!(run(&[link("abcd"), link("abcd")], "abc"), Err(Refusal::TooLarge));
}

/// Failure: a different disk, whose volume has the same inode, size and time (a restored or cloned image attached under a reused device
/// number), is joined to this document and its original backup is purged (`docs/design/removal.md` §4: the gate).
#[test]
fn a_different_volume_with_the_same_numbers_is_another_document() {
    let here = stamp(7, 100, 1);
    let there = FileVersion { volume: Some([2; 16]), ..here };
    let links = [
        at(there, "/clone.pdf"),
        Link::Saved {
            from: there,
            to: FileVersion { size: 120, ..there },
        },
    ];
    let led = ledger(&links);
    let l = lineage(&led, here, Path::new("/doc.pdf"), &LEDGER_LIMITS).unwrap();
    assert_eq!(sides_of(&l), [("/doc.pdf".into(), Side::Target), ("/clone.pdf".into(), Side::Other)]);
    assert!(l.versions().all(|(_, side)| side == Side::Other));
}

/// Failure: a disk that was ejected and attached again (so under another device number, which the version no longer holds) is treated as
/// another disk, and every removal on an external drive refuses (the cost decided on 2026-11-25).
#[test]
fn the_same_volume_is_the_same_document_whatever_mount_it_had() {
    let (s0, s1) = (stamp(7, 100, 1), stamp(7, 120, 2));
    let links = [at(s0, "/old/doc.pdf"), Link::Saved { from: s0, to: s1 }];
    let led = ledger(&links);
    let l = lineage(&led, s1, Path::new("/new/doc.pdf"), &LEDGER_LIMITS).unwrap();
    assert!(l.versions().all(|(_, side)| side == Side::Target));
    assert!(l.complete());
}

/// Failure: a version with no volume identity (a network volume, FAT, or an unreadable answer) joins another by inode, size and time alone.
#[test]
fn a_version_without_a_volume_joins_nothing() {
    let none = FileVersion {
        volume: None,
        ..stamp(7, 100, 1)
    };
    let links = [at(none, "/a"), Link::Saved { from: none, to: none }];
    let led = ledger(&links);
    let l = lineage(&led, none, Path::new("/cur"), &LEDGER_LIMITS).unwrap();
    assert_eq!(sides_of(&l), [("/cur".into(), Side::Target), ("/a".into(), Side::Other)]);
    let zero = FileVersion {
        volume: Some([0; 16]),
        ..stamp(7, 100, 1)
    };
    let led = ledger(&[at(zero, "/a")]);
    let l = lineage(&led, zero, Path::new("/cur"), &LEDGER_LIMITS).unwrap();
    assert_eq!(sides_of(&l)[1].1, Side::Other);
}

/// Failure: after a block copy of a disk both copies are saved from the same version, and the history is read as one line, so a removal on one
/// purges the backup the other still needs. One version saved into two different versions leaves the lineage incomplete.
#[test]
fn a_fork_makes_the_lineage_incomplete() {
    let (s0, s1, s2) = (stamp(7, 100, 1), stamp(7, 110, 2), stamp(7, 120, 3));
    let straight = [
        Link::Saved { from: s0, to: s1 },
        Link::Saved { from: s0, to: s1 },
        Link::Saved { from: s1, to: s2 },
    ];
    let led = ledger(&straight);
    let l = lineage(&led, s2, Path::new("/p"), &LEDGER_LIMITS).unwrap();
    assert!(l.complete(), "the same save seen twice is not a fork");
    let forked = [Link::Saved { from: s0, to: s1 }, Link::Saved { from: s0, to: s2 }];
    let led = ledger(&forked);
    let l = lineage(&led, s2, Path::new("/p"), &LEDGER_LIMITS).unwrap();
    assert_eq!((l.complete(), l.gap()), (false, Some(Gap::Forked)));
    // A fork in some other document's history is not this one's.
    let led = ledger(&forked);
    let elsewhere = lineage(&led, stamp(99, 1, 1), Path::new("/p"), &LEDGER_LIMITS).unwrap();
    assert!(elsewhere.complete());
}

/// Failure: a path the ledger saw a file at but could not verify is placed on one side, so a backup under it is purged with a document that
/// may not be its own.
#[test]
fn an_unplaced_path_is_unknown() {
    let links = [Link::Unplaced { path: "/p.pdf".into() }];
    let led = ledger(&links);
    let l = lineage(&led, stamp(7, 100, 1), Path::new("/q.pdf"), &LEDGER_LIMITS).unwrap();
    let mut sides = Sides::default();
    for (_, side) in l.paths().filter(|(p, _)| *p == Path::new("/p.pdf")) {
        sides = sides.with(side);
    }
    assert_eq!(sides.owner(TargetMode::Save), Owner::Unknown);
}

/// Failure: a ledger that lost something (a gap) reads as complete, and the backup folder is marked fully surveyed.
#[test]
fn a_gap_in_the_ledger_is_the_lineages_gap() {
    let mut gapped = ledger(&[]);
    gapped.gap = Some(Gap::Torn);
    gapped.began_ns = 77;
    let l = lineage(&gapped, stamp(7, 100, 1), Path::new("/p"), &LEDGER_LIMITS).unwrap();
    assert_eq!((l.complete(), l.gap(), l.began_ns()), (false, Some(Gap::Torn), 77));
}
