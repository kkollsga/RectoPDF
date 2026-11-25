use super::*;

fn stamp(inode: u64, size: u64, mtime_ns: i64) -> FileStamp {
    FileStamp {
        size,
        mtime_ns,
        device: 1,
        inode,
    }
}

fn at(stamp: FileStamp, path: &str) -> Link {
    Link::At {
        stamp,
        path: PathBuf::from(path),
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
    let l = lineage(&links, s1, current, &LEDGER_LIMITS).unwrap();
    assert_eq!(
        sides_of(&l),
        [("/p2".into(), Side::Target), ("/p1".into(), Side::Target), ("/p2".into(), Side::Target)]
    );
    assert!(l.stamps().all(|(_, side)| side == Side::Target));
    assert_eq!(l.stamps().count(), 4);
}

/// Failure: the history of a `cp -p` copy (same size and time, another inode) is purged with this document's.
#[test]
fn a_copy_is_another_document() {
    let copy = stamp(8, 100, 1);
    let links = [at(copy, "/copy.pdf")];
    let l = lineage(&links, stamp(7, 100, 1), Path::new("/doc.pdf"), &LEDGER_LIMITS).unwrap();
    assert_eq!(sides_of(&l), [("/doc.pdf".into(), Side::Target), ("/copy.pdf".into(), Side::Other)]);
}

/// Failure: a different file written at the same path by another program is taken for this document by its path alone, and its original
/// backup is purged.
#[test]
fn a_file_changed_elsewhere_at_the_same_path_is_unknown() {
    let links = [at(stamp(7, 100, 1), "/p.pdf")];
    let l = lineage(&links, stamp(9, 50, 5), Path::new("/p.pdf"), &LEDGER_LIMITS).unwrap();
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
    let l = lineage(&links, zero, Path::new("/cur"), &LEDGER_LIMITS).unwrap();
    assert_eq!(sides_of(&l), [("/cur".into(), Side::Target), ("/a".into(), Side::Other)]);
    assert!(l.stamps().all(|(_, side)| side == Side::Other));
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
    let run = |links: &[Link], current: &str| lineage(links, s, Path::new(current), &small).map(|_| ());
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

/// Failure: a block-level clone or a restored image keeps inode, size and time, so an independent copy on another volume joins this document
/// and its original backup is purged (`docs/design/removal.md` §4: never equate files by inode, size and time across volumes).
#[test]
fn a_clone_on_another_volume_is_another_document() {
    let here = stamp(7, 100, 1);
    let there = FileStamp { device: 2, ..here };
    let links = [
        at(there, "/clone.pdf"),
        Link::Saved {
            from: there,
            to: FileStamp { size: 120, ..there },
        },
    ];
    let l = lineage(&links, here, Path::new("/doc.pdf"), &LEDGER_LIMITS).unwrap();
    assert_eq!(sides_of(&l), [("/doc.pdf".into(), Side::Target), ("/clone.pdf".into(), Side::Other)]);
    assert!(l.stamps().all(|(_, side)| side == Side::Other));
}
