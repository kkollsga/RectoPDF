//! Writes small PDFs for tests, so expected values come from the bytes rather than from another parser.

/// Builds a classic-xref PDF from object bodies. Object numbers start at 1, in the given order.
pub fn pdf(objects: &[String], trailer_extra: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

/// Catalog 1, pages 2, three pages 3 to 5, then the given extra objects from 6 up.
pub fn three_page_pdf(catalog_extra: &str, extra: &[String], trailer_extra: &str) -> Vec<u8> {
    let mut objects = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_string(),
    ];
    for _ in 0..3 {
        objects.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_string());
    }
    objects.extend_from_slice(extra);
    pdf(&objects, &format!("/Root 1 0 R {trailer_extra}"))
}

/// A unique temp file holding the bytes, in a folder of its own, removed on drop.
pub use recto_testpdf::TempPdf;
