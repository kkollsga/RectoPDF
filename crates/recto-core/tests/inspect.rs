mod support;

use recto_core::{NONE, OpenError, inspect};
use support::{TempPdf, three_page_pdf};

fn outline_objects() -> Vec<String> {
    vec![
        // 6: outline root. 7: "One" with child 8. 9: "Two".
        "<< /Type /Outlines /First 7 0 R /Last 9 0 R /Count 2 >>".to_string(),
        "<< /Title (One) /Parent 6 0 R /Next 9 0 R /First 8 0 R /Last 8 0 R /Dest [3 0 R /Fit] >>".to_string(),
        "<< /Title (One point one) /Parent 7 0 R /A << /S /GoTo /D [4 0 R /Fit] >> >>".to_string(),
        "<< /Title (Two) /Parent 6 0 R /Prev 7 0 R /Dest [5 0 R /Fit] >>".to_string(),
    ]
}

/// Failure: page count, title or version are read wrongly, or a plain PDF fails to open.
#[test]
fn reads_basic_facts() {
    let mut extra = outline_objects();
    extra.push("<< /Title (Harbour Survey) >>".to_string());
    let file = TempPdf::new("facts.pdf", &three_page_pdf("/Outlines 6 0 R", &extra, "/Info 10 0 R"));
    let facts = inspect(&file.0).unwrap();
    assert_eq!(facts.page_count, 3);
    assert_eq!(facts.title.as_deref(), Some("Harbour Survey"));
    assert!(!facts.encrypted);
    assert_eq!(facts.byte_len, std::fs::metadata(&file.0).unwrap().len());
}

/// Failure: contents lose their order or nesting, or a destination points at the wrong page.
#[test]
fn reads_contents_in_order_with_nesting_and_pages() {
    let file = TempPdf::new("toc.pdf", &three_page_pdf("/Outlines 6 0 R", &outline_objects(), ""));
    let toc = inspect(&file.0).unwrap().toc;
    let rows: Vec<_> = toc.entries().iter().map(|e| (toc.title(e), e.depth, e.page, e.parent)).collect();
    assert_eq!(rows, vec![("One", 0, 0, NONE), ("One point one", 1, 1, 0), ("Two", 0, 2, NONE)]);
}

/// Failure: the section being read is wrong, so the rail marks the wrong row.
#[test]
fn finds_the_section_for_a_page() {
    let file = TempPdf::new("section.pdf", &three_page_pdf("/Outlines 6 0 R", &outline_objects(), ""));
    let toc = inspect(&file.0).unwrap().toc;
    assert_eq!(toc.section_at(0), Some(0));
    assert_eq!(toc.section_at(1), Some(1));
    assert_eq!(toc.section_at(2), Some(2));
}

/// Failure: a hostile outline that loops back on itself hangs the reader or grows without limit.
#[test]
fn survives_an_outline_that_loops() {
    let extra = vec![
        "<< /Type /Outlines /First 7 0 R >>".to_string(),
        "<< /Title (Loop) /Next 7 0 R /First 7 0 R /Dest [3 0 R /Fit] >>".to_string(),
    ];
    let file = TempPdf::new("loop.pdf", &three_page_pdf("/Outlines 6 0 R", &extra, ""));
    let toc = inspect(&file.0).unwrap().toc;
    assert_eq!(toc.entries().len(), 1);
}

/// Failure: a named destination isn't followed through the catalog's name tree, so the contents can't jump.
#[test]
fn follows_named_destinations_through_the_name_tree() {
    let extra = vec![
        "<< /Type /Outlines /First 7 0 R >>".to_string(),
        "<< /Title (Named) /Dest (chapter) >>".to_string(),
        "<< /Dests 9 0 R >>".to_string(),
        "<< /Names [(chapter) [5 0 R /Fit]] >>".to_string(),
    ];
    let file = TempPdf::new("named.pdf", &three_page_pdf("/Outlines 6 0 R /Names 8 0 R", &extra, ""));
    let toc = inspect(&file.0).unwrap().toc;
    assert_eq!(toc.entries()[0].page, 2);
}

/// Failure: a missing or non-PDF file panics or reports the wrong reason.
#[test]
fn reports_plain_reasons_for_bad_files() {
    let missing = std::env::temp_dir().join("recto-core-test-does-not-exist.pdf");
    assert_eq!(inspect(&missing).unwrap_err(), OpenError::NotFound);
    let not_pdf = TempPdf::new("not.pdf", b"this is not a pdf at all");
    assert_eq!(inspect(&not_pdf.0).unwrap_err(), OpenError::NotPdf);
    assert!(!OpenError::NotPdf.message().contains("os error"));
}

/// Failure: a wide name tree under a long outline makes `open` resolve every child for every entry, billions of reads, while the session waits. It must stop at its budget, and say the contents aren't whole.
#[test]
fn a_hostile_name_tree_under_a_long_outline_ends_soon() {
    // 20,000 entries, each naming a destination that isn't in a tree with one node of 200,000 references to itself and to a leaf.
    let refs = "7 0 R ".repeat(200_000);
    let mut extra = vec!["<< /Type /Outlines /First 9 0 R >>".to_string(), format!("<< /Kids [{refs}] >>"), "<< /Dests 7 0 R >>".to_string()];
    let entries = 20_000;
    for i in 0..entries {
        let next = if i + 1 < entries { format!("/Next {} 0 R", 10 + i) } else { String::new() };
        extra.push(format!("<< /Title (Entry) /Dest (nowhere) {next} >>"));
    }
    let file = TempPdf::new("name-bomb.pdf", &three_page_pdf("/Outlines 6 0 R /Names 8 0 R", &extra, ""));
    let started = std::time::Instant::now();
    let toc = inspect(&file.0).unwrap().toc;
    assert!(started.elapsed() < std::time::Duration::from_secs(30), "took {:?}", started.elapsed());
    assert!(toc.truncated, "it must say the contents aren't whole");
}

/// Failure: an ordinary named destination stops being found, because the budget is spent too eagerly, or a tree with a good branch after a bad one loses it.
#[test]
fn an_ordinary_name_tree_is_still_followed_past_a_dead_branch() {
    let extra = vec![
        "<< /Type /Outlines /First 7 0 R >>".to_string(),
        "<< /Title (Named) /Dest (chapter) >>".to_string(),
        "<< /Dests 9 0 R >>".to_string(),
        "<< /Kids [10 0 R 99 0 R 11 0 R] >>".to_string(),
        "<< /Limits [(a) (b)] /Names [(a) [3 0 R /Fit]] >>".to_string(),
        "<< /Limits [(c) (d)] /Names [(chapter) [5 0 R /Fit]] >>".to_string(),
    ];
    let file = TempPdf::new("named-branch.pdf", &three_page_pdf("/Outlines 6 0 R /Names 8 0 R", &extra, ""));
    let toc = inspect(&file.0).unwrap().toc;
    assert_eq!(toc.entries()[0].page, 2);
    assert!(!toc.truncated);
}

/// A 40-page document with an outline from `outline`, objects numbered from 3 + 2 × 40, and a name tree of `p0` and `p30`, as the EU's competitiveness report, part B, names its pages.
fn outlined_by_names(outline: impl Fn(usize) -> Vec<String>) -> Vec<u8> {
    use recto_testpdf::{Object, build};
    let pages = 40;
    let page = |index: usize| 3 + index * 2;
    let first_extra = 3 + pages * 2;
    let kids: Vec<String> = (0..pages).map(|index| format!("{} 0 R", page(index))).collect();
    let names = first_extra;
    let mut objects = vec![
        Object::Dict(format!("<< /Type /Catalog /Pages 2 0 R /Outlines {} 0 R /Names << /Dests {names} 0 R >> >>", first_extra + 1)),
        Object::Dict(format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", kids.join(" "))),
    ];
    for index in 0..pages {
        objects.push(Object::Dict(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R >>", page(index) + 1)));
        objects.push(Object::Stream { dict: String::new(), data: Vec::new() });
    }
    objects.push(Object::Dict(format!("<< /Names [(p0) [{} 0 R /Fit] (p30) [{} 0 R /Fit]] >>", page(0), page(30))));
    objects.extend(outline(first_extra + 1).into_iter().map(Object::Dict));
    build(&objects)
}

/// Failure: the EU's competitiveness report, part B, has every bookmark of its first half pointing at a named destination `p0`, which is page 1, so nearly every entry read 1 and one 227, and the owner saw numbers that "isnt logical". The numbers are the file's own, read as every reader reads them, so they're kept, and the verdict says these contents look bad, which is what offers generated ones.
#[test]
fn bookmarks_all_named_for_the_first_page_read_as_page_one_and_look_bad() {
    let file = TempPdf::new("piled.pdf", &outlined_by_names(|root| {
        let (first, children) = (root + 1, 12);
        let mut objects = vec![format!("<< /Type /Outlines /First {first} 0 R /Last {} 0 R /Count 2 >>", first + children + 1)];
        objects.push(format!("<< /Title (Sectoral policies) /Parent {root} 0 R /Next {} 0 R /First {} 0 R /Last {} 0 R /Count {children} /A << /S /GoTo /D (p0) >> >>", first + children + 1, first + 1, first + children));
        for child in 0..children {
            let at = first + 1 + child;
            let links = format!("{}{}", if child > 0 { format!(" /Prev {} 0 R", at - 1) } else { String::new() }, if child + 1 < children { format!(" /Next {} 0 R", at + 1) } else { String::new() });
            objects.push(format!("<< /Title ({}. Section) /Parent {first} 0 R{links} /A << /S /GoTo /D (p0) /NewWindow true >> >>", child + 1));
        }
        objects.push(format!("<< /Title (Horizontal policies) /Parent {root} 0 R /Prev {first} 0 R /A << /S /GoTo /D (p30) >> >>"));
        objects
    }));
    let facts = inspect(&file.0).unwrap();
    let pages: Vec<u32> = facts.toc.entries().iter().map(|entry| entry.page).collect();
    assert_eq!(pages.len(), 14);
    assert!(pages[..13].iter().all(|page| *page == 0), "{pages:?}");
    assert_eq!(pages[13], 30);
    assert_eq!(recto_core::verdict::judge(&facts.toc, facts.page_count).code(), "piled_on_one_page");
}

/// Failure: part A of the same report nests chapters 2 to 6 inside chapter 1. The indents are the file's own and are shown as they are, and the verdict says they look bad, which offers generated contents.
#[test]
fn chapters_nested_inside_chapter_one_read_as_the_file_has_them_and_look_bad() {
    let file = TempPdf::new("misnested.pdf", &outlined_by_names(|root| {
        let one = root + 1;
        let mut objects = vec![format!("<< /Type /Outlines /First {one} 0 R /Last {one} 0 R /Count 1 >>")];
        objects.push(format!("<< /Title (1. The starting point) /Parent {root} 0 R /First {} 0 R /Last {} 0 R /Count 3 /A << /S /GoTo /D (p0) >> >>", one + 1, one + 3));
        for chapter in 0..3 {
            let at = one + 1 + chapter;
            let links = format!("{}{}", if chapter > 0 { format!(" /Prev {} 0 R", at - 1) } else { String::new() }, if chapter < 2 { format!(" /Next {} 0 R", at + 1) } else { String::new() });
            objects.push(format!("<< /Title ({}. Chapter) /Parent {one} 0 R{links} /A << /S /GoTo /D (p30) >> >>", chapter + 2));
        }
        objects
    }));
    let facts = inspect(&file.0).unwrap();
    let depths: Vec<u8> = facts.toc.entries().iter().map(|entry| entry.depth).collect();
    assert_eq!(depths, [0, 1, 1, 1], "as the file has them");
    assert_eq!(recto_core::verdict::judge(&facts.toc, facts.page_count).code(), "misnested");
}

/// A file that opens only with the user password `"open sesame"`, written by lopdf's eager writer, which alone encrypts.
#[allow(clippy::disallowed_methods)] // A test fixture built with lopdf's eager writer, which alone encrypts; never over a user's file.
fn locked_pdf() -> Vec<u8> {
    use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
    let mut document = lopdf::Document::load_mem(&recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["Behind a password"]))])).unwrap();
    document.trailer.set("ID", lopdf::Object::Array(vec![lopdf::Object::string_literal("0123456789abcdef"), lopdf::Object::string_literal("0123456789abcdef")]));
    let encryption = EncryptionState::try_from(EncryptionVersion::V2 { document: &document, owner_password: "owner", user_password: "open sesame", key_length: 128, permissions: Permissions::all() }).unwrap();
    document.encrypt(&encryption).unwrap();
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();
    bytes
}

/// Failure: a PDF that opens only with a user password reads as "not a PDF", because the reader's `PasswordRequired` was folded into every other open error, so the person is told the file is broken instead of being asked for its password.
#[test]
fn a_user_password_pdf_reads_as_needing_its_password_not_as_not_a_pdf() {
    let file = TempPdf::new("locked.pdf", &locked_pdf());
    assert_eq!(inspect(&file.0).unwrap_err(), OpenError::PasswordNeeded);
}

/// Failure: a protected PDF opens with no password or a wrong one, doesn't open with the right one, or a password can be printed or holds a NUL the renderer would cut at.
#[test]
fn a_protected_file_opens_only_with_its_password() {
    use recto_core::{Password, inspect_with_password};
    let file = TempPdf::new("locked.pdf", &locked_pdf());
    assert_eq!(inspect_with_password(&file.0, &Password::new("wrong").unwrap()).unwrap_err(), OpenError::PasswordNeeded);
    let facts = inspect_with_password(&file.0, &Password::new("open sesame").unwrap()).unwrap();
    assert_eq!((facts.page_count, facts.encrypted), (1, true));
    let password = Password::new("open sesame").unwrap();
    assert!(!format!("{password:?}").contains("sesame"), "never printed");
    assert!(Password::new("").is_none() && Password::new("a\0b").is_none() && Password::new(&"x".repeat(recto_core::MAX_PASSWORD_BYTES + 1)).is_none());
}
