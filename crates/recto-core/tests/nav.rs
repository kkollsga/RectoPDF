//! Links and page labels, read from real bytes.

use recto_core::nav::{LinkTarget, MAX_LABEL_RANGES, MAX_PAGE_LINKS, Refused, page_links, page_links_bytes};
use recto_core::{inspect, inspect_bytes};
use recto_testpdf::{LINKS_ON_PAGE_ONE, Object, TempPdf, build, linked_document};
use std::sync::Arc;

/// Failure: a link goes to the wrong page or the wrong depth, a named destination isn't found, or a script, a launch or a `file:` address comes back as something to open.
#[test]
fn each_kind_of_link_is_judged_as_it_should_be() {
    let file = TempPdf::new("links.pdf", &linked_document());
    let read = page_links(&file.0, &[0, 3]).unwrap();
    assert_eq!(read.len(), 2);
    let (page, links) = &read[0];
    assert_eq!(*page, 0);
    assert_eq!(links.links.len(), LINKS_ON_PAGE_ONE.len());
    let targets: Vec<&LinkTarget> = links.links.iter().map(|link| &link.target).collect();
    assert!(matches!(targets[0], LinkTarget::Page { page: 8, within: Some(within) } if (within - 1.0 / 3.0).abs() < 0.01), "{:?}", targets[0]);
    assert_eq!(targets[1], &LinkTarget::Page { page: 6, within: Some(0.0) });
    assert_eq!(targets[2], &LinkTarget::Web("https://example.com/report?id=7".into()));
    assert_eq!(targets[3], &LinkTarget::Refused(Refused::Script));
    assert_eq!(targets[4], &LinkTarget::Refused(Refused::Launch));
    assert_eq!(targets[5], &LinkTarget::Refused(Refused::File));
    assert_eq!(targets[6], &LinkTarget::Page { page: 1, within: None });
    assert_eq!(targets[7], &LinkTarget::Nowhere);
    for (link, (rect, _)) in links.links.iter().zip(LINKS_ON_PAGE_ONE) {
        for (got, want) in link.rect.iter().zip(rect) {
            assert!((got - want).abs() < 0.01, "{:?} against {rect:?}", link.rect);
        }
    }
    let middle = |rect: &[f32; 4]| ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    let (x, y) = middle(&LINKS_ON_PAGE_ONE[2].0);
    assert_eq!(links.at(x, y).map(|link| &link.target), Some(&LinkTarget::Web("https://example.com/report?id=7".into())));
    assert!(links.at(10.0, 10.0).is_none());
    assert!(read[1].1.links.is_empty(), "page 4 has none");
    assert!(page_links(&file.0, &[99]).unwrap()[0].1.links.is_empty(), "a page that isn't there has none, and doesn't fail");
}

/// Failure: page labels aren't read from the file, so roman front matter shows as plain numbers.
#[test]
fn page_labels_come_from_the_file() {
    let file = TempPdf::new("labels.pdf", &linked_document());
    let facts = inspect(&file.0).unwrap();
    let labels = &facts.toc.labels;
    let shown: Vec<String> = (0..12).map(|page| labels.label(page).unwrap()).collect();
    assert_eq!(shown, ["i", "ii", "iii", "iv", "1", "2", "3", "4", "5", "6", "A-1", "A-2"]);
    assert_eq!(labels.find("A-2", facts.page_count), Some(11));
}

/// A page tree of `pages` pages, the first with `annots`, and the catalog with `catalog_extra`.
fn document(pages: usize, annots: &str, catalog_extra: &str, extra: Vec<Object>) -> Vec<u8> {
    let kids: Vec<String> = (0..pages).map(|index| format!("{} 0 R", 3 + index)).collect();
    let mut objects = vec![Object::Dict(format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>")), Object::Dict(format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", kids.join(" ")))];
    for index in 0..pages {
        objects.push(Object::Dict(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {} >>", if index == 0 { annots } else { "" })));
    }
    objects.extend(extra);
    build(&objects)
}

/// Failure: a page with a hundred thousand links, or a label tree of a million ranges, is read in full, so a hostile file holds the page view's thread and its memory.
#[test]
fn hostile_links_and_labels_are_capped() {
    let many = (0..MAX_PAGE_LINKS * 3).map(|at| format!("<< /Subtype /Link /Rect [{at} 0 {} 10] /Dest [3 0 R /Fit] >>", at + 1)).collect::<Vec<_>>().join(" ");
    let bytes = document(1, &format!("/Annots [{many}]"), "", Vec::new());
    let read = page_links_bytes(Arc::from(bytes), &[0]).unwrap();
    assert_eq!(read[0].1.links.len(), MAX_PAGE_LINKS);
    assert!(read[0].1.truncated);
    let nums = (0..MAX_LABEL_RANGES * 4).map(|at| format!("{at} << /S /D /P (p{at}-) >>")).collect::<Vec<_>>().join(" ");
    let bytes = document(MAX_LABEL_RANGES * 5, "", &format!("/PageLabels << /Nums [{nums}] >>"), Vec::new());
    let facts = inspect_bytes(Arc::from(bytes)).unwrap();
    assert_eq!(facts.toc.labels.ranges().len(), MAX_LABEL_RANGES);
    assert!(facts.toc.labels.truncated);
}

/// Failure: a label tree that lists itself, or a named destination that names itself, loops for ever.
#[test]
fn trees_that_loop_end() {
    let looped = vec![Object::Dict("<< /Kids [4 0 R 4 0 R] /Nums [0 << /S /r >>] >>".into())];
    let bytes = document(1, "", "/PageLabels 4 0 R", looped);
    let facts = inspect_bytes(Arc::from(bytes)).unwrap();
    assert_eq!(facts.toc.labels.label(0).as_deref(), Some("i"));
    let bytes = document(1, "/Annots [<< /Subtype /Link /Rect [0 0 10 10] /Dest /a >>]", "/Dests << /a /b /b /a >>", Vec::new());
    let read = page_links_bytes(Arc::from(bytes), &[0]).unwrap();
    assert_eq!(read[0].1.links[0].target, LinkTarget::Nowhere);
}

/// Failure: links are read from whatever is at the file's path now, so after the file's replaced and before the page view takes that up, the new version's links are offered over the old version's pages.
#[test]
fn links_through_an_open_file_are_that_files() {
    let file = TempPdf::new("links.pdf", &linked_document());
    let held = std::fs::File::open(&file.0).unwrap();
    // Replaced by a rename, as an editor saves: the path now has a file with no links.
    let other = file.0.with_file_name("other.pdf");
    std::fs::write(&other, document(1, "", "", Vec::new())).unwrap();
    std::fs::rename(&other, &file.0).unwrap();
    assert!(page_links(&file.0, &[0]).unwrap()[0].1.links.is_empty(), "the path is the new file");
    let read = recto_core::nav::page_links_in(held, None, &[0]).unwrap();
    assert_eq!(read[0].1.links.len(), LINKS_ON_PAGE_ONE.len(), "the open file is the old one, whose pages are showing");
}
