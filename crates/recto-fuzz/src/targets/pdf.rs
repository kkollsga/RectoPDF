//! `recto_core::inspect` and the signature inventory, on hostile bytes. These run in the session's own process, so a panic loses unsaved edits and a stack overflow can't even be caught.
//!
//! What must hold, whatever the file: it returns, in bounded time and memory. What it returns is within its caps and clean to show. What couldn't be read is never reported as absent.

use crate::guard::{Budget, within};
use crate::input::Take;
use crate::targets::DocOnly;
use recto_agent::{Level, parse, run};
use recto_core::nav::page_links_bytes;
use recto_core::{
    DocFacts, MAX_SIGNATURES, MAX_TITLE_BYTES, MAX_TOC_DEPTH, MAX_TOC_ENTRIES, NONE, SignatureInventory, SignatureState, clean_line, clean_text, inspect_bytes,
    is_invisible_format, signatures_in_bytes,
};
use recto_session::Session;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Far past anything real, and well under the 1 GiB that's a failure by definition. Two walks of 5 s each, with room for a loaded machine and a slow build around them.
const BUDGET: Budget = Budget {
    time: Duration::from_secs(30),
    memory: 512 * 1024 * 1024,
};

/// Mutated real files. Catches: a panic, a hang or a bomb in opening, the page tree, the outline, name trees, or the field tree, on anything a damaged or hostile file can hold.
pub fn check_bytes(data: &[u8]) {
    read(data);
}

/// Files built from the fuzz bytes, always with a true cross-reference table, so structure is what's hostile rather than the framing. Catches: loops, depth, width and dangling references in the walks, which random bytes in a real file rarely make.
pub fn check_tree(data: &[u8]) {
    read(&build_tree(data));
}

/// Save's planner, copier and remapper on a hostile file, as the file pages are added from and as the file saved: they refuse or write a file that reads back with
/// the pages they said, within the time and memory a check has. The copy's own bounds are what keep a file that names millions of references inside them.
pub fn check_save(data: &[u8]) {
    let hostile = build_tree(data);
    let plain = build_tree(&[]);
    within("planning and copying a save", BUDGET, || {
        let _ = recto_core::save::plan_in_memory(&plain, &hostile);
        let _ = recto_core::save::plan_in_memory(&hostile, &plain);
        // Highlights alone on a hostile file's own pages, the path that writes no new tree, and one of its own taken out by the rewrite (R4.3).
        // What a save says of its highlights holds in what it wrote: each place a `/Highlight` at its `/Rect`, and what it took out gone (S1, S2).
        for file in [&hostile, &highlight_tree(data)] {
            let held = recto_core::save::highlights_in_memory(file);
            let broken = recto_core::save::SaveError::CheckFailed {
                detail: recto_core::save::HIGHLIGHTS_DONT_HOLD,
            };
            assert_ne!(held, Err(broken), "what the save says of its highlights doesn't hold");
        }
    });
    // The rewrite: a delete in a hostile file, with and without pages added, and hostile pages added to a plain file. What it writes passes the fork's verify, or it refuses.
    within("planning and writing a rewrite", BUDGET, || {
        let _ = recto_core::save::rewrite_in_memory(&hostile, &plain, false);
        let _ = recto_core::save::rewrite_in_memory(&hostile, &plain, true);
        let _ = recto_core::save::rewrite_in_memory(&plain, &hostile, true);
    });
    // A form's pages added to a hostile file's form, and a hostile file's added to a form; the signature rules a save decides by; and the kept copy's rewrite.
    let form = form_pdf();
    within("merging forms, the signature rules and a kept copy", BUDGET, || {
        let _ = recto_core::save::plan_in_memory(&hostile, &form);
        let _ = recto_core::save::plan_in_memory(&form, &hostile);
        let pages: Vec<u32> = data
            .iter()
            .take(8)
            .map(|byte| u32::from(*byte % 16))
            .collect::<std::collections::BTreeSet<u32>>()
            .into_iter()
            .collect();
        let _ = recto_core::save::signatures_and_keep_in_memory(&hostile, &pages);
    });
}

/// A small PDF with a form whose field's widgets are on its page, a field chain, a signature widget, and fonts in its form's resources.
fn form_pdf() -> Vec<u8> {
    use recto_testpdf::Object as Raw;
    recto_testpdf::build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 8 0 R] /DR << /Font << /Helv 7 0 R >> >> /SigFlags 3 >> >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [6 0 R 8 0 R] >>".into()),
        Raw::Stream {
            dict: String::new(),
            data: b"BT /F1 12 Tf 10 10 Td (form) Tj ET".to_vec(),
        },
        Raw::Dict("<< /T (topmostSubform[0]) /Kids [6 0 R] >>".into()),
        Raw::Dict("<< /Type /Annot /Subtype /Widget /FT /Tx /T (f1) /V (value) /Rect [0 0 50 20] /Parent 5 0 R >>".into()),
        Raw::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()),
        Raw::Dict("<< /Type /Annot /Subtype /Widget /FT /Sig /T (Sig) /V 9 0 R /Rect [0 0 50 20] >>".into()),
        Raw::Dict("<< /Type /Sig /ByteRange [0 1 2 3] /Contents <00> >>".into()),
    ])
}

fn read(bytes: &[u8]) {
    let shared: Arc<[u8]> = bytes.into();
    within("reading a PDF's structure", BUDGET, || {
        if let Ok(facts) = inspect_bytes(Arc::clone(&shared)) {
            assert_facts_hold(&facts, bytes.len() as u64);
            through_the_session(facts);
        }
        if let Ok(read) = page_links_bytes(Arc::clone(&shared), &[0, 1, 2, 7, u32::MAX]) {
            assert_links_hold(&read);
        }
        // A file's highlights (R4.3): bounded, and every rectangle finite, of some area and inside its page.
        for page in [0, 1, u32::MAX] {
            if let Ok(read) = recto_core::highlights::page_highlights_bytes(Arc::clone(&shared), page) {
                assert_highlights_hold(&read);
            }
        }
        // The agreement gate's half: bounded, a size for each page compared and no more, and every size a real one.
        if let Ok(view) = recto_core::agreement::structure_view_bytes(Arc::clone(&shared)) {
            assert_eq!(view.sizes.len(), recto_core::agreement::compared_pages(view.page_count).len());
            assert!(
                view.sizes
                    .iter()
                    .flatten()
                    .all(|(width, height)| width.is_finite() && height.is_finite() && *width > 0.0 && *height > 0.0)
            );
        }
        // What an SVG export asks of a page before it starts: bounded, or `None`, on any page a file can name.
        for page in [0, 1, 7, u32::MAX] {
            let _ = recto_core::page_effects_bytes(Arc::clone(&shared), page);
        }
        let _ = recto_core::copy_restricted_bytes(Arc::clone(&shared));
        if let Ok(inventory) = signatures_in_bytes(shared) {
            assert_inventory_holds(&inventory, bytes.len() as u64);
        }
    });
}

fn assert_shown_text_is_clean(what: &str, text: &str) {
    assert!(text.len() <= MAX_TITLE_BYTES, "{what} is {} bytes", text.len());
    assert!(
        !text.chars().any(|c| is_invisible_format(c) || c.is_control()),
        "{what} still holds something a reader can't see: {text:?}"
    );
    assert_eq!(clean_line(text), text, "{what} isn't what cleaning it gives");
}

fn assert_facts_hold(facts: &DocFacts, file_bytes: u64) {
    assert_eq!(facts.byte_len, file_bytes);
    if let Some(title) = &facts.title {
        assert_shown_text_is_clean("the title", title);
    }
    let toc = &facts.toc;
    assert!(toc.entries().len() <= MAX_TOC_ENTRIES);
    for (index, entry) in toc.entries().iter().enumerate() {
        assert!(entry.depth <= MAX_TOC_DEPTH, "depth {}", entry.depth);
        assert!(
            entry.page == NONE || entry.page < facts.page_count,
            "page {} of {}",
            entry.page,
            facts.page_count
        );
        assert!(
            entry.parent == NONE || (entry.parent as usize) < index,
            "entry {index} has parent {}",
            entry.parent
        );
        assert!(entry.top <= recto_core::TOP_SCALE, "entry {index} is {} down its page", entry.top);
        assert_shown_text_is_clean("a contents title", toc.title(entry));
    }
    for page in [0, facts.page_count / 2, u32::MAX] {
        if let Some(found) = toc.section_at(page) {
            assert!(found < toc.entries().len());
        }
        for top in [0, 500, recto_core::TOP_SCALE] {
            if let Some(found) = toc.current_at(page, top, |_| true) {
                assert!(found < toc.entries().len());
            }
            let band = toc.band_at(page, top, |_| true);
            assert!(band.lo <= band.hi || band.hi == 0, "{band:?}");
        }
        if let Some(label) = toc.labels.label(page) {
            assert!(label.len() <= recto_core::nav::MAX_LABEL_BYTES, "{} bytes", label.len());
            assert!(!label.chars().any(|c| is_invisible_format(c) || c.is_control()), "{label:?}");
            assert!(toc.labels.find(&label, facts.page_count).is_none_or(|found| found < facts.page_count.max(1)));
        }
    }
    assert!(toc.labels.ranges().len() <= recto_core::nav::MAX_LABEL_RANGES);
}

/// A file's highlights: within their caps, and every rectangle finite, of some area and inside its page.
fn assert_highlights_hold(read: &recto_core::highlights::PageHighlights) {
    use recto_core::highlights::{MAX_PAGE_HIGHLIGHTS, MAX_QUADS};
    assert!(read.found.len() <= MAX_PAGE_HIGHLIGHTS);
    let (width, height) = read.frame.shown_size().unwrap_or((0.0, 0.0));
    for found in &read.found {
        assert!(!found.rects.is_empty() && found.rects.len() <= MAX_QUADS);
        for [x, y, w, h] in &found.rects {
            assert!(
                [*x, *y, *w, *h].iter().all(|edge| edge.is_finite()) && *w > 0.0 && *h > 0.0,
                "{:?}",
                found.rects
            );
            assert!(
                *x >= 0.0 && *y >= 0.0 && x + w <= width * 1.0001 && y + h <= height * 1.0001,
                "{:?} on {width} by {height}",
                found.rects
            );
        }
        assert!(found.rgb.is_none_or(|rgb| rgb <= 0xFF_FF_FF));
    }
}

/// Links: within their caps, boxes real, addresses only of the kinds that may be offered, and a page a link goes to one there is.
fn assert_links_hold(read: &[(u32, recto_core::nav::PageLinks)]) {
    use recto_core::nav::{LinkTarget, MAX_PAGE_LINKS, MAX_PAGES_PER_READ, MAX_URI_BYTES};
    assert!(read.len() <= MAX_PAGES_PER_READ);
    for (_, links) in read {
        assert!(links.links.len() <= MAX_PAGE_LINKS);
        for link in &links.links {
            assert!(
                link.rect.iter().all(|edge| edge.is_finite()) && link.rect[2] > link.rect[0] && link.rect[3] > link.rect[1],
                "{:?}",
                link.rect
            );
            match &link.target {
                LinkTarget::Web(address) => {
                    assert!(address.len() <= MAX_URI_BYTES && address.bytes().all(|b| b.is_ascii_graphic()), "{address:?}");
                    let lower = address.to_ascii_lowercase();
                    assert!(
                        lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:"),
                        "{address:?}"
                    );
                }
                LinkTarget::Page { within, .. } => assert!(within.is_none_or(|within| (0.0..=1.0).contains(&within))),
                _ => {}
            }
        }
    }
}

fn assert_inventory_holds(inventory: &SignatureInventory, file_bytes: u64) {
    assert!(inventory.entries.len() <= MAX_SIGNATURES, "{} signatures", inventory.entries.len());
    // "None" is only ever said of a file that was really read.
    assert!(inventory.structure_read || inventory.entries.is_empty());
    for entry in &inventory.entries {
        for text in [
            Some(&entry.field),
            entry.filter.as_ref(),
            entry.sub_filter.as_ref(),
            entry.stated_name.as_ref(),
            entry.stated_reason.as_ref(),
            entry.stated_location.as_ref(),
            entry.stated_time.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            assert_shown_text_is_clean("a signature's text", text);
        }
        assert!(entry.bytes_after_range.is_none_or(|after| after <= file_bytes));
        if entry.state != SignatureState::Signed {
            assert!(
                entry.byte_range.is_none() && entry.stated_name.is_none(),
                "only a signed field has anything stated"
            );
        }
    }
}

/// What a session does with those facts: the sidebar and the agent's view of the contents. A hostile outline reaches these too.
fn through_the_session(facts: DocFacts) {
    let path = Path::new("/fuzz/doc.pdf");
    let mut session = Session::new(DocOnly(Arc::new(facts.clone())));
    let Ok(id) = session.open_with_facts(path, facts, None, None) else { return };
    let _ = session.sidebar();
    let _ = session.tap(id);
    for line in [
        r#"{"cmd":"contents"}"#,
        r#"{"cmd":"contents","depth":0,"limit":3}"#,
        r#"{"cmd":"contents","from":4294967295,"depth":255}"#,
        r#"{"cmd":"facts"}"#,
        r#"{"cmd":"where"}"#,
        r#"{"cmd":"sidebar"}"#,
    ] {
        let command = parse(line).expect("a command written here");
        let _ = run(&mut session, Level::Control, &command);
    }
}

/// Hostile numbers a structure field gets.
const NUMBERS: &[&str] = &["0", "1", "2", "-1", "65535", "2147483647", "4294967296", "99999999999999999999", "0.5"];

fn reference(take: &mut Take<'_>, count: usize) -> String {
    match take.u8() {
        0..=239 => format!("{} 0 R", 1 + usize::from(take.u8()) % count),
        240..=247 => format!("{} 0 R", take.pick(NUMBERS)),
        248..=251 => "null".to_string(),
        _ => take.pick(NUMBERS).to_string(),
    }
}
fn references(take: &mut Take<'_>, count: usize) -> String {
    let many = take.u8() % 6;
    (0..many).map(|_| reference(take, count)).collect::<Vec<_>>().join(" ")
}
fn text(take: &mut Take<'_>) -> String {
    const TEXTS: &[&str] = &[
        "(a)",
        "(chapter)",
        "()",
        "<FEFF0041202E0042>",
        "(\\200\\237)",
        "<FEFFD800>",
        "(line\\nbreak)",
        "<EFBBBF41>",
        "/NotAString",
        "12",
    ];
    take.pick(TEXTS).to_string()
}

/// What an outline node says of where it goes: a page and a place down it from numbers a file can write, a name, an action, or nothing.
fn outline_dest(take: &mut Take<'_>, count: usize) -> String {
    match take.u8() % 9 {
        0 => format!("/Dest [{} /Fit]", reference(take, count)),
        1 => format!("/Dest {}", text(take)),
        2 => "/Dest /chapter".to_string(),
        3 => format!("/A << /S /GoTo /D {} >>", reference(take, count)),
        4 => format!("/Dest {}", reference(take, count)),
        5 => format!("/Dest [{} /XYZ {} {} 0]", reference(take, count), take.pick(NUMBERS), take.pick(NUMBERS)),
        6 => format!("/Dest [{} /FitH {}]", reference(take, count), take.pick(NUMBERS)),
        7 => format!(
            "/Dest [{} /FitR {} {} {} {}]",
            reference(take, count),
            take.pick(NUMBERS),
            take.pick(NUMBERS),
            take.pick(NUMBERS),
            take.pick(NUMBERS)
        ),
        _ => String::new(),
    }
}

/// A node of a page tree, as hostile as its kids, count and parent.
fn pages_node(take: &mut Take<'_>, count: usize) -> String {
    format!(
        "<< /Type /Pages /Kids [{}] /Count {} /Parent {} >>",
        references(take, count),
        take.pick(NUMBERS),
        reference(take, count)
    )
}

/// A highlight, a reply to one, a popup, or a structure element naming one (second review of R4.3, SF6): what a save's removal of a file's
/// highlight takes out, reached however the file likes (a list naming it twice, another page's list, `/IRT`, `/Popup`, `/Parent`, `/OBJR`), and
/// sometimes a page or a catalog dressed as one.
fn markup(take: &mut Take<'_>, count: usize) -> String {
    match take.u8() % 5 {
        0 | 1 => format!(
            "<< /Type {} /Subtype /Highlight /Rect [{} {} {} {}] /Popup {} /AP << /N {} >> /Contents {} /StructParent {} >>",
            take.pick(&["/Annot", "/Annot", "/Page"]),
            take.pick(&["50", "50", "0", "-1e9"]),
            take.pick(NUMBERS),
            take.pick(NUMBERS),
            take.pick(NUMBERS),
            reference(take, count),
            reference(take, count),
            text(take),
            take.pick(NUMBERS)
        ),
        2 => format!("<< /Subtype /Text /IRT {} /Rect [0 0 9 9] /Contents {} >>", reference(take, count), text(take)),
        3 => format!(
            "<< /Type /StructElem /S /Annot /Pg {} /K << /Type /OBJR /Obj {} >> >>",
            reference(take, count),
            reference(take, count)
        ),
        _ => format!(
            "<< /Type {} /Subtype /Popup /Parent {} /Pages {} >>",
            take.pick(&["/Annot", "/Page", "/Catalog"]),
            reference(take, count),
            reference(take, count)
        ),
    }
}

/// **A file whose first page lists a highlight, among hostile markup** (second review of R4.3, SF6): two pages and a fixed tree, so the save's
/// removal path is reached on most inputs; everything the lists and the highlight name comes from the fuzz bytes. Page 1's first item is a highlight
/// at `[50 700 150 712]` that a save can take out.
pub fn highlight_tree(data: &[u8]) -> Vec<u8> {
    let mut take = Take(data);
    let count = 6 + usize::from(take.u8() % 24);
    let mut objects = vec![
        recto_testpdf::Object::Dict(format!(
            "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot << /K [{}] >> >>",
            references(&mut take, count)
        )),
        recto_testpdf::Object::Dict("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into()),
        recto_testpdf::Object::Dict(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R {}] >>",
            references(&mut take, count)
        )),
        recto_testpdf::Object::Dict(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{}] >>",
            references(&mut take, count)
        )),
        recto_testpdf::Object::Dict(format!(
            "<< /Type /Annot /Subtype /Highlight /Rect [50 700 150 712] /Popup {} /AP << /N {} >> /Contents (note) >>",
            reference(&mut take, count),
            reference(&mut take, count)
        )),
    ];
    while objects.len() < count {
        objects.push(recto_testpdf::Object::Dict(markup(&mut take, count)));
    }
    recto_testpdf::build(&objects)
}

/// Builds a PDF out of the bytes: objects that are pieces of what the walks read, joined by references the bytes choose, some to themselves, some to nothing. Object 1 is the catalog, and it names object 2 to 6 as its pages, outline, names, form and permissions.
pub fn build_tree(data: &[u8]) -> Vec<u8> {
    let mut take = Take(data);
    let count = 7 + usize::from(take.u8() % 64);
    // A reference to some object, or now and then to one that isn't there.
    let mut objects = Vec::with_capacity(count);
    objects.push(recto_testpdf::Object::Dict(format!(
        "<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R /Names 4 0 R /Dests {} /AcroForm 5 0 R /Perms 6 0 R /PageLabels {} /OCProperties << /OCGs [{}] >> >>",
        reference(&mut take, count),
        reference(&mut take, count),
        references(&mut take, count)
    )));
    while objects.len() < count {
        let which = if objects.len() < 6 { objects.len() as u8 - 1 } else { take.u8() % 12 };
        let object = match which {
            0 | 6 => recto_testpdf::Object::Dict(pages_node(&mut take, count)),
            1 | 7 => {
                let dest = outline_dest(&mut take, count);
                recto_testpdf::Object::Dict(format!(
                    "<< /Title {} /First {} /Next {} {dest} >>",
                    text(&mut take),
                    reference(&mut take, count),
                    reference(&mut take, count)
                ))
            }
            2 | 8 => recto_testpdf::Object::Dict(format!(
                "<< /Dests {} /Kids [{}] /Names [{} {}] /Limits [{} {}] >>",
                reference(&mut take, count),
                references(&mut take, count),
                text(&mut take),
                reference(&mut take, count),
                text(&mut take),
                text(&mut take)
            )),
            3 => recto_testpdf::Object::Dict(format!("<< /Fields [{}] >>", references(&mut take, count))),
            4 => recto_testpdf::Object::Dict(format!("<< /DocMDP {} >>", reference(&mut take, count))),
            5 => recto_testpdf::Object::Dict(format!(
                "<< /Type /Page /Parent {} /Resources {} /Contents {} /MediaBox [0 0 {} {}] /CropBox [{} {} {} {}] /Rotate {} /Annots {} >>",
                reference(&mut take, count),
                reference(&mut take, count),
                reference(&mut take, count),
                take.pick(NUMBERS),
                take.pick(NUMBERS),
                take.pick(NUMBERS),
                take.pick(NUMBERS),
                take.pick(NUMBERS),
                take.pick(NUMBERS),
                take.pick(NUMBERS),
                if take.u8().is_multiple_of(2) {
                    format!("[{}]", references(&mut take, count))
                } else {
                    reference(&mut take, count)
                }
            )),
            10 => {
                // A link, or a node of a page-label tree: what links and labels read.
                let action = match take.u8() % 8 {
                    0 => format!("/Dest [{} /XYZ {} {} 0]", reference(&mut take, count), take.pick(NUMBERS), take.pick(NUMBERS)),
                    1 => format!("/Dest {}", text(&mut take)),
                    2 => format!(
                        "/A << /S /URI /URI {} >>",
                        take.pick(&["(https://example.com)", "(javascript:x)", "(file:///x)", "<6874747073>", "()"])
                    ),
                    3 => format!(
                        "/A << /S /Named /N {} >>",
                        take.pick(&["/NextPage", "/PrevPage", "/LastPage", "/GoBack", "/Print"])
                    ),
                    4 => format!("/A << /S /GoTo /D {} >>", reference(&mut take, count)),
                    5 => format!("/A {}", reference(&mut take, count)),
                    _ => format!(
                        "/Nums [{} << /S {} /P {} /St {} >>] /Kids [{}]",
                        take.pick(NUMBERS),
                        take.pick(&["/D", "/r", "/R", "/a", "/A", "/X"]),
                        text(&mut take),
                        take.pick(NUMBERS),
                        references(&mut take, count)
                    ),
                };
                recto_testpdf::Object::Dict(format!(
                    "<< /Subtype /Link /Rect [{} {} {} {}] {action} >>",
                    take.pick(NUMBERS),
                    take.pick(NUMBERS),
                    take.pick(NUMBERS),
                    take.pick(NUMBERS)
                ))
            }
            9 => recto_testpdf::Object::Dict(format!(
                "<< /FT {} /T {} /V {} /P {} /Kids {} >>",
                take.pick(&["/Sig", "/Sig", "/Tx", "3 0 R"]),
                text(&mut take),
                reference(&mut take, count),
                reference(&mut take, count),
                if take.u8().is_multiple_of(2) {
                    format!("[{}]", references(&mut take, count))
                } else {
                    reference(&mut take, count)
                }
            )),
            _ => match take.u8() % 8 {
                7 => recto_testpdf::Object::Dict(markup(&mut take, count)),
                0 => recto_testpdf::Object::Dict(format!(
                    "<< /Filter /Adobe.PPKLite /SubFilter /ETSI.CAdES.detached /Name {} /M {} /ByteRange [{} {} {} {}] >>",
                    text(&mut take),
                    text(&mut take),
                    take.pick(NUMBERS),
                    take.pick(NUMBERS),
                    take.pick(NUMBERS),
                    take.pick(NUMBERS)
                )),
                1 => recto_testpdf::Object::Stream {
                    dict: format!("/Type /ObjStm /N {} /First {}", take.pick(NUMBERS), take.pick(NUMBERS)),
                    data: take.pick(&[b"1 0 << >>".as_slice(), b"9 0 2 3 << /Kids [1 0 R] >>", b""]).to_vec(),
                },
                // What `page_effects` walks: resources, the forms, images and patterns they name, graphics states and layers, and a page's own drawing.
                3 => recto_testpdf::Object::Dict(format!(
                    "<< /XObject << /X1 {} /X2 {} >> /Pattern << /P1 {} >> /ExtGState << /G1 << /SMask {} /BM {} >> /G2 {} >> /Properties << /MC1 {} >> /ColorSpace << /C1 {} >> >>",
                    reference(&mut take, count),
                    reference(&mut take, count),
                    reference(&mut take, count),
                    take.pick(&["/None", "1 0 R", "<< /S /Luminosity >>", "/Luminosity"]),
                    take.pick(&["/Normal", "/Multiply", "[/Screen /Normal]", "3 0 R"]),
                    reference(&mut take, count),
                    reference(&mut take, count),
                    take.pick(&["/DeviceRGB", "[/ICCBased 3 0 R]", "[/Indexed /DeviceRGB 255 <00>]", "/CMYK"])
                )),
                4 => recto_testpdf::Object::Stream {
                    dict: format!(
                        "/Type /XObject /Subtype {} /Filter {} /Decode {} /DecodeParms {} /SMask {} /SMaskInData {} /ColorSpace {} /OC {} /Resources {} /Width 1 /Height 1 /BitsPerComponent 8",
                        take.pick(&["/Form", "/Image", "/Form", "/PS"]),
                        take.pick(&["/DCTDecode", "[/FlateDecode /DCT]", "/FlateDecode", "[/Fl]", "3 0 R", "/JPXDecode"]),
                        take.pick(&["[1 0 1 0 1 0]", "[0 1 0 1 0 1]", "[]", "3 0 R"]),
                        take.pick(&["<< /ColorTransform 0 >>", "<< /Predictor 12 >>", "[null]", "3 0 R"]),
                        reference(&mut take, count),
                        take.pick(NUMBERS),
                        take.pick(&["/DeviceGray", "/DeviceCMYK", "[/ICCBased 3 0 R]", "/Lab"]),
                        reference(&mut take, count),
                        reference(&mut take, count)
                    ),
                    data: take
                        .pick(&[b"x\x9c\x03\x00".as_slice(), b"\xff\xd8\xff\xd9", b"q 1 0 0 1 0 0 cm /X1 Do Q", b""])
                        .to_vec(),
                },
                5 => recto_testpdf::Object::Dict(format!(
                    "<< /PatternType {} /Shading {} /Resources {} /Type /OCG /Name {} >>",
                    take.pick(NUMBERS),
                    reference(&mut take, count),
                    reference(&mut take, count),
                    text(&mut take)
                )),
                6 => recto_testpdf::Object::Stream {
                    dict: take
                        .pick(&[
                            "",
                            "/Filter /FlateDecode",
                            "/Filter [/FlateDecode /FlateDecode]",
                            "/Filter /FlateDecode /DecodeParms << /Predictor 2 >>",
                        ])
                        .to_string(),
                    data: take
                        .pick(&[
                            b"BI /W 1 /H 1 /F /DCT ID \xff\xd8\xff\xd9 EI".as_slice(),
                            b"BI /W 1 /H 1 /Filter [/DCTDecode] ID x EI",
                            b"x\x9c\xcb\x48\xcd\xc9\xc9\x07\x00\x06\x2c\x02\x15",
                            b"q /X1 Do Q (BI /F /DCT) Tj",
                        ])
                        .to_vec(),
                },
                _ => recto_testpdf::Object::Dict(format!("[{}]", references(&mut take, count))),
            },
        };
        objects.push(object);
    }
    // Now and then a document's information and an encryption dictionary, which the trailer names: what a title, `/P` and opening with no password read.
    let trailer = match take.u8() % 4 {
        0 => {
            objects.push(recto_testpdf::Object::Dict(format!(
                "<< /Title {} /Author {} >>",
                text(&mut take),
                text(&mut take)
            )));
            format!("/Info {} 0 R", objects.len())
        }
        1 => {
            objects.push(recto_testpdf::Object::Dict(format!(
                "<< /Filter /Standard /V {} /R {} /Length {} /P {} /O <{}> /U <{}> /CF << /StdCF << /CFM {} /Length 16 >> >> /StmF /StdCF /StrF /StdCF >>",
                take.pick(&["1", "2", "4", "5", "99"]),
                take.pick(&["2", "3", "4", "6", "-1"]),
                take.pick(&["40", "128", "256", "7"]),
                take.pick(&["-4", "-3904", "0", "4294967295", "1 0 R"]),
                "28BF4E5E4E758A4164004E56FFFA01082E2E00B6D0683E802F0CA9FE6453697A",
                take.pick(&["", "00", "28BF4E5E4E758A4164004E56FFFA01082E2E00B6D0683E802F0CA9FE6453697A"]),
                take.pick(&["/V2", "/AESV2", "/AESV3", "/None", "/Identity"])
            )));
            format!("/Encrypt {} 0 R /ID [<0123456789ABCDEF0123456789ABCDEF> <00>]", objects.len())
        }
        _ => String::new(),
    };
    let built = recto_testpdf::build(&objects);
    if trailer.is_empty() {
        return built;
    }
    // The trailer comes after the cross-reference table, so what's added there moves no offset.
    let root: &[u8] = b"/Root 1 0 R";
    match built.windows(root.len()).rposition(|window| window == root) {
        Some(at) => {
            let mut out = built[..at].to_vec();
            out.extend_from_slice(format!("/Root 1 0 R {trailer}").as_bytes());
            out.extend_from_slice(&built[at + "/Root 1 0 R".len()..]);
            out
        }
        None => built,
    }
}

/// Text cleaning alone. Catches: a cut inside a character, or a text string that keeps what a reader can't see.
pub fn check_text(data: &[u8]) {
    assert_shown_text_is_clean("cleaned text", &clean_text(data));
    assert_shown_text_is_clean("a cleaned line", &clean_line(&String::from_utf8_lossy(data)));
}
