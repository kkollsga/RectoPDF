//! The signature inventory on hostile and ordinary files. Bytes come from the test, so the answers do too.

mod support;

use recto_core::{OpenError, inspect, signatures};
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
    assert!(tenant.signed);
    assert_eq!(tenant.page, Some(1));
    assert_eq!(tenant.stated_name.as_deref(), Some("Tor Haugen"));
    assert_eq!(tenant.sub_filter.as_deref(), Some("adbe.pkcs7.detached"));
    assert_eq!(tenant.byte_range, Some([0, 10, 20, 30]));
    assert!(tenant.bytes_after_range.is_some());
    let landlord = &found.entries[1];
    assert_eq!(landlord.field, "Landlord");
    assert!(!landlord.signed);
    assert_eq!(landlord.stated_name, None);
}

/// Failure: a file with no form at all reads as an error, so "this has no signature" can't be said.
#[test]
fn a_file_with_no_form_has_no_entries() {
    let file = TempPdf::new("plain.pdf", &three_page_pdf("", &[], ""));
    let found = signatures(&file.0).unwrap();
    assert!(found.entries.is_empty());
    assert!(!found.certified && !found.truncated);
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
    assert!(entry.signed);
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
