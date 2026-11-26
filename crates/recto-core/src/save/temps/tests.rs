//! The writer's temporary names, held against what the pinned lopdf makes.

use super::*;
use crate::save::open_for_save;
use lopdf::{AppendDestination, AppendOptions};
use recto_testpdf::TempDir;
use std::cell::RefCell;

/// The entries of `dir` while a save is in the middle of its work, as (name, shape read by the rule, whether it is a folder holding `clone`).
fn temps_during(existing: bool, name: &str) -> (std::path::PathBuf, Vec<(String, Option<TempShape>, bool)>) {
    let dir = TempDir::new("recto-temps-");
    let path = dir.join(name);
    if existing {
        std::fs::write(&path, recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["one"]))])).unwrap();
    } else {
        // A Save As starts from a reader of some other file.
        std::fs::write(
            dir.join("source.pdf"),
            recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["one"]))]),
        )
        .unwrap();
    }
    let source = if existing { path.clone() } else { dir.join("source.pdf") };
    let reader = open_for_save(&source).unwrap();
    let seen = RefCell::new(Vec::new());
    // Nothing is added, so the save ends in `EmptyUpdate` and its temporary is removed; what is wanted is what stood there while it ran.
    let _ = lopdf::append_atomically(
        &reader,
        if existing {
            AppendDestination::SameFile(&path)
        } else {
            AppendDestination::NewFile(&path)
        },
        AppendOptions::new(),
        |_writer| {
            for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
                let name = entry.file_name().into_string().unwrap();
                if name == "source.pdf" || Path::new(&name) == Path::new(path.file_name().unwrap()) {
                    continue;
                }
                let shape = writer_temp_shape(name.as_bytes()).map(|(_, shape)| shape);
                let holds_clone = entry.path().join("clone").exists();
                seen.borrow_mut().push((name, shape, holds_clone));
            }
            Ok(())
        },
    );
    (path, seen.into_inner())
}

/// Failure: the fork renames its temporary (a different prefix, hash, suffix or a flat file for the clone), and a survey reads "none found" in a folder
/// that holds a crashed save's whole copy of the document. This calls the real `append_atomically` and reads what it makes.
#[test]
fn the_rule_reads_what_the_pinned_append_makes() {
    let (path, same) = temps_during(true, "mine.pdf");
    assert_eq!(same.len(), 1, "one temporary while a save over the file runs: {same:?}");
    assert!(same[0].0.starts_with(&writer_temp_prefix(&path)), "the target's prefix: {}", same[0].0);
    assert_eq!(same[0].1, Some(TempShape::Folder), "a save over the file works in a folder");
    assert!(same[0].2, "holding the clone");
    let (path, new) = temps_during(false, "copy.pdf");
    assert_eq!(new.len(), 1, "one temporary for a Save As: {new:?}");
    assert!(new[0].0.starts_with(&writer_temp_prefix(&path)), "the target's prefix: {}", new[0].0);
    assert_eq!(new[0].1, Some(TempShape::File));
}

/// Failure: the hash is taken from the whole path, so two saves of one name in different folders name different temps (or one folder's temps
/// match another document), or a path with no file name gets no prefix.
#[test]
fn the_prefix_is_of_the_file_name_alone() {
    let a = writer_temp_prefix(Path::new("/one/folder/report.pdf"));
    assert_eq!(a, writer_temp_prefix(Path::new("/another/report.pdf")));
    assert_ne!(a, writer_temp_prefix(Path::new("/one/folder/other.pdf")));
    assert_eq!(
        writer_temp_prefix(Path::new("/")),
        writer_temp_prefix(Path::new("")),
        "no file name is `document`"
    );
    assert_eq!(a.len(), ".append-".len() + 16 + 1);
    let name = format!("{a}12-0.tmp");
    let (hash, shape) = writer_temp_shape(name.as_bytes()).unwrap();
    assert_eq!((hash, shape), (&a[".append-".len()..a.len() - 1], TempShape::File));
}

/// Failure: a person's own file with a similar name is taken for a temporary (every removal then refuses on it), or lopdf's own is missed.
#[test]
fn only_the_exact_shape_reads_as_a_temporary() {
    let hash = "0123456789abcdef";
    let ok = |name: String| writer_temp_shape(name.as_bytes()).map(|(_, shape)| shape);
    assert_eq!(ok(format!(".append-{hash}-123-4.tmp")), Some(TempShape::File));
    assert_eq!(ok(format!(".append-{hash}-1-0.d")), Some(TempShape::Folder));
    for bad in [
        format!(".append-{hash}-123-4"),
        format!(".append-{hash}-123-4.TMP"),
        format!(".append-{hash}-123-4.tmp.bak"),
        format!("append-{hash}-123-4.tmp"),
        format!(".append-{}-123-4.tmp", &hash[1..]),
        format!(".append-{hash}0-123-4.tmp"),
        format!(".append-{}-123-4.tmp", hash.to_uppercase()),
        format!(".append-{}z-123-4.tmp", &hash[..15]),
        format!(".append-{hash}--4.tmp"),
        format!(".append-{hash}-123-.tmp"),
        format!(".append-{hash}-12a-4.tmp"),
        format!(".append-{hash}-123.tmp"),
        format!(".append-{hash}-1-2-3.tmp"),
        format!(".append-{hash}junk"),
        ".append-zz".to_string(),
        "report.pdf".to_string(),
        String::new(),
    ] {
        assert_eq!(ok(bad.clone()), None, "{bad}");
    }
    assert_eq!(writer_temp_shape(b".append-0123456789abcdef-1-2.tmp\xff"), None, "bytes that are not text");
}
