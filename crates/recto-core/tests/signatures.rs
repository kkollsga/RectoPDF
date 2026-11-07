//! The signature inventory on hostile and ordinary files. Bytes come from the test, so the answers do too.

mod support;

use recto_core::{MAX_FIELD_VISITS, OpenError, SignatureState, inspect, signatures, signatures_within};
use std::time::Duration;
use support::{TempPdf, three_page_pdf};

fn signed_pdf() -> Vec<u8> {
    let extra = vec![
        "<< /Fields [7 0 R 9 0 R] /SigFlags 3 >>".to_string(),
        "<< /FT /Sig /T (Tenant) /V 8 0 R /P 4 0 R >>".to_string(),
        "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /Name (Tor\\nHaugen) /Reason (Agreed) /M (D:20260101120000Z) /ByteRange [0 10 20 30] /Contents <00> >>".to_string(),
        "<< /FT /Sig /T (Landlord) >>".to_string(),
    ];
    three_page_pdf("/AcroForm 6 0 R /Perms << /DocMDP 8 0 R >>", &extra, "")
}

/// Failure: a signature field isn't found, a stated name reaches an agent with a line break in it, or a waiting field reads as signed.
#[test]
fn finds_signed_and_waiting_fields_with_their_stated_facts() {
    let file = TempPdf::new("signed.pdf", &signed_pdf());
    let found = signatures(&file.0).unwrap();
    assert!(found.certified);
    assert!(!found.truncated);
    assert_eq!(found.entries.len(), 2);
    let tenant = &found.entries[0];
    assert_eq!(tenant.field, "Tenant");
    assert_eq!(tenant.state, SignatureState::Signed);
    assert_eq!(tenant.page, Some(1));
    assert_eq!(tenant.stated_name.as_deref(), Some("Tor Haugen"));
    assert_eq!(tenant.sub_filter.as_deref(), Some("adbe.pkcs7.detached"));
    assert_eq!(tenant.byte_range, Some([0, 10, 20, 30]));
    assert!(tenant.bytes_after_range.is_some());
    let landlord = &found.entries[1];
    assert_eq!(landlord.field, "Landlord");
    assert_eq!(landlord.state, SignatureState::Empty);
    assert_eq!(landlord.stated_name, None);
}

/// Failure: a file with no form at all reads as an error, so "this has no signature" can't be said.
#[test]
fn a_file_with_no_form_has_no_entries() {
    let file = TempPdf::new("plain.pdf", &three_page_pdf("", &[], ""));
    let found = signatures(&file.0).unwrap();
    assert!(found.entries.is_empty());
    assert!(found.structure_read, "this file was read, and has no form");
    assert!(!found.certified && !found.truncated);
    assert_eq!(found.unread, 0);
}

/// Failure: a field tree that loops back on itself hangs the walk or reports the same field for ever.
#[test]
fn a_looping_field_tree_ends() {
    let extra = vec![
        "<< /Fields [7 0 R 7 0 R] >>".to_string(),
        "<< /T (Loop) /Kids [7 0 R 8 0 R] >>".to_string(),
        "<< /FT /Sig /T (Inner) /Kids [7 0 R] /V 9 0 R >>".to_string(),
        "<< /ByteRange [-1 2 3] >>".to_string(),
    ];
    let file = TempPdf::new("loop.pdf", &three_page_pdf("/AcroForm 6 0 R", &extra, ""));
    let found = signatures(&file.0).unwrap();
    assert_eq!(found.entries.len(), 1);
    assert_eq!(found.entries[0].field, "Loop.Inner");
    assert_eq!(found.entries[0].byte_range, None, "a range that isn't four whole numbers isn't reported");
}

/// Failure: a field tree thousands deep overflows the stack, or a million fields grows the report without limit. It must stop at its cap and say it did.
#[test]
fn a_deep_or_wide_tree_stops_at_its_caps_and_says_so() {
    let depth = 5_000;
    let mut deep = vec!["<< /Fields [7 0 R] >>".to_string()];
    for i in 0..depth {
        deep.push(format!("<< /T (n) /Kids [{} 0 R] >>", 8 + i));
    }
    deep.push("<< /FT /Sig /T (Bottom) >>".to_string());
    let file = TempPdf::new("deep.pdf", &three_page_pdf("/AcroForm 6 0 R", &deep, ""));
    assert!(signatures(&file.0).unwrap().truncated);

    let width = recto_core::MAX_SIGNATURES + 500;
    let names: Vec<String> = (0..width).map(|i| format!("{} 0 R", 7 + i)).collect();
    let mut wide = vec![format!("<< /Fields [{}] >>", names.join(" "))];
    wide.extend((0..width).map(|i| format!("<< /FT /Sig /T (f{i}) >>")));
    let file = TempPdf::new("wide.pdf", &three_page_pdf("/AcroForm 6 0 R", &wide, ""));
    let found = signatures(&file.0).unwrap();
    assert!(found.truncated);
    assert_eq!(found.entries.len(), recto_core::MAX_SIGNATURES);
}

/// Failure: hidden instructions in a stated name reach an agent, or a huge string comes through whole.
#[test]
fn stated_text_is_cleaned_and_capped() {
    let long = "A".repeat(5_000);
    let extra = vec![
        "<< /Fields [7 0 R] >>".to_string(),
        format!("<< /FT /Sig /T (F) /V << /Name (x\\033y\\nz) /Reason ({long}) >> >>"),
    ];
    let file = TempPdf::new("hostile.pdf", &three_page_pdf("/AcroForm 6 0 R", &extra, ""));
    let entry = &signatures(&file.0).unwrap().entries[0];
    assert_eq!(entry.state, SignatureState::Signed);
    assert_eq!(entry.stated_name.as_deref(), Some("x y z"));
    assert!(entry.stated_reason.as_ref().unwrap().len() <= recto_core::MAX_TITLE_BYTES);
}

/// Failure: a missing file, a damaged one or a pipe reads as "no signatures" or hangs, rather than saying why it couldn't be read.
#[test]
fn files_that_cant_be_read_say_why() {
    assert_eq!(signatures(std::path::Path::new("/no/such/file.pdf")).unwrap_err(), OpenError::NotFound);
    let junk = TempPdf::new("junk.pdf", b"not a pdf");
    assert_eq!(signatures(&junk.0).unwrap_err(), OpenError::NotPdf);
    let fifo = std::env::temp_dir().join(format!("recto-core-fifo-{}", std::process::id()));
    assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    assert_eq!(inspect(&fifo).unwrap_err(), OpenError::NotPdf, "a pipe must be refused, not waited on");
    assert_eq!(signatures(&fifo).unwrap_err(), OpenError::NotPdf);
    std::fs::remove_file(&fifo).unwrap();
}

/// An object number the file points at, whose bytes there aren't an object.
fn with_corrupt_object(extra: &[String], catalog: &str) -> Vec<u8> {
    let bytes = three_page_pdf(catalog, extra, "");
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // Object 6 is damaged in place, keeping every offset in the table: its opening goes, so nothing can be read at that offset.
    text.replace("6 0 obj\n<<", "6 0 xyz\n<<").into_bytes()
}

/// Failure: a form that can't be read reports as "no signatures" (`structure_read`, none found), which someone takes to mean the file isn't signed.
#[test]
fn an_unreadable_form_is_not_reported_as_no_signatures() {
    for (name, catalog) in [("gone", "/AcroForm 99 0 R"), ("corrupt", "/AcroForm 6 0 R")] {
        let extra = vec!["<< /Fields [] >>".to_string()];
        let file = TempPdf::new(&format!("{name}.pdf"), &with_corrupt_object(&extra, catalog));
        let found = signatures(&file.0).unwrap();
        assert!(!found.structure_read, "{name}: {found:?}");
        assert!(found.entries.is_empty());
    }
    let not_a_form = TempPdf::new("form-number.pdf", &three_page_pdf("/AcroForm 12", &[], ""));
    assert!(!signatures(&not_a_form.0).unwrap().structure_read);
    let fields_gone = TempPdf::new("fields-gone.pdf", &three_page_pdf("/AcroForm << /Fields 99 0 R >>", &[], ""));
    assert!(!signatures(&fields_gone.0).unwrap().structure_read);
    // An empty form, and a form with no fields at all, really are forms with nothing in them.
    let empty = TempPdf::new("empty-form.pdf", &three_page_pdf("/AcroForm << /Fields [] >>", &[], ""));
    assert!(signatures(&empty.0).unwrap().structure_read);
    let no_fields = TempPdf::new("no-fields.pdf", &three_page_pdf("/AcroForm << /SigFlags 3 >>", &[], ""));
    assert!(signatures(&no_fields.0).unwrap().structure_read);
}

/// Failure: an unreadable field is skipped without a word, and its value reads as "empty", so a signed field looks unsigned. And a field written directly into the array is dropped.
#[test]
fn what_couldnt_be_read_is_counted_and_never_called_empty() {
    let extra = vec![
        "<< /Fields [99 0 R 7 0 R 42 << /FT /Sig /T (Direct) >>] >>".to_string(),
        "<< /FT /Sig /T (ValueGone) /V 98 0 R >>".to_string(),
    ];
    let file = TempPdf::new("partly.pdf", &three_page_pdf("/AcroForm 6 0 R", &extra, ""));
    let found = signatures(&file.0).unwrap();
    assert!(found.structure_read);
    let rows: Vec<_> = found.entries.iter().map(|entry| (entry.field.as_str(), entry.state)).collect();
    assert_eq!(rows, [("ValueGone", SignatureState::Unread), ("Direct", SignatureState::Empty)]);
    assert_eq!(found.unread, 3, "the missing field, the number in the array, and the value that's gone: {found:?}");
}

/// Failure: one /Kids array of a few megabytes makes a copy of the parent's name per child, about 400 MiB from a 4 MiB array and more from several, in a process with no memory limit. Measured as whole-process growth,
/// so it fails if per-child memory comes back. It also has to stop at the cap and say so.
#[test]
fn a_wide_kids_array_costs_only_itself() {
    let long_name = "N".repeat(500);
    // About 650,000 references in one array, near the reader's 4 MiB limit on one object, under a parent with the longest name there is.
    let refs = "7 0 R ".repeat(650_000);
    let extra = vec!["<< /Fields [8 0 R] >>".to_string(), "<< /T (leaf) >>".to_string(), format!("<< /T ({long_name}) /Kids [{refs}] >>")];
    let file = TempPdf::new("wide-kids.pdf", &three_page_pdf("/AcroForm 6 0 R", &extra, ""));
    let before = recto_platform::isolate::process_memory(std::process::id()).unwrap().peak_footprint_bytes;
    let found = signatures(&file.0).unwrap();
    let after = recto_platform::isolate::process_memory(std::process::id()).unwrap().peak_footprint_bytes;
    let grew = after.saturating_sub(before);
    println!("peak grew {} MiB", grew / (1024 * 1024));
    assert!(found.truncated, "it must say it stopped");
    assert!(found.structure_read);
    // The array itself costs about 70 MiB to parse here. A copy of a name per child adds over 300 more.
    assert!(grew < 250 * 1024 * 1024, "peak grew {} MiB", grew / (1024 * 1024));
}

/// Failure: a walk has no bound on time, so a file whose references are slow to chase holds a job and the session's memory for as long as it likes.
#[test]
fn the_walk_stops_at_its_time_limit_and_says_so() {
    let names: Vec<String> = (0..MAX_FIELD_VISITS / 8).map(|i| format!("{} 0 R", 7 + i)).collect();
    let mut wide = vec![format!("<< /Fields [{}] >>", names.join(" "))];
    wide.extend((0..MAX_FIELD_VISITS / 8).map(|i| format!("<< /T (f{i}) >>")));
    let file = TempPdf::new("timed.pdf", &three_page_pdf("/AcroForm 6 0 R", &wide, ""));
    let stopped = signatures_within(&file.0, Duration::ZERO).unwrap();
    assert!(stopped.truncated);
    let finished = signatures(&file.0).unwrap();
    assert!(!finished.truncated, "{finished:?}");
}
