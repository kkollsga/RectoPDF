//! **Printing** (SPEC section 6, Reading; PLAN phase 3, "Print"). The document's real pages, drawn by PDFium at print resolution through the export path, never the screen's tiles, and written a strip at a time into a new PDF of one picture a page, which the app hands to the system's printing and `recto print --to-pdf` keeps.
//!
//! - **Paper colour never prints** (owner's brief, 2026-11-08): the export path knows nothing of dark or warm paper, so a page is what's in the file.
//! - **Turns print as the edits have them**: a page's unsaved quarter turns go on its page as `/Rotate`, and its picture stays as drawn.
//! - **Streamed and bounded**: one page at a time, each in strips under the export's limits, sharp or refused (owner, 2026-11-07). The host holds one strip, one row and one compressor, whatever the document's length. A page that needs the larger limit waits for the heavy slot, as any export does, so it's for a job thread, never the UI thread.
//! - **Only a new file.** It never writes over anything, least of all the PDF being printed: the lopdf writers alone change a PDF's bytes, and this makes a new one from pictures.

use crate::render::{FileOut, Output, OwnTemp, output_exists, publish_picture};
use crate::{AgentError, exit};
use flate2::{Compression, write::ZlibEncoder};
use recto_proto::BYTES_PER_PIXEL;
use recto_render_service::{DocRenderer, ExportRefusal, OutputCaps, PageEvent, Policy, RenderFailure, ServiceConfig, draw_whole_page};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The resolution a page prints at when nothing says: what a laser printer and the system's own PDF printing use.
pub const DEFAULT_DPI: u32 = 300;
pub const MIN_DPI: u32 = 72;
pub const MAX_DPI: u32 = 600;
/// The most pages one print takes, past which it says so: a document of millions of pages shouldn't become one print job by accident.
pub const MAX_PAGES: u32 = 10_000;
/// The most one print's file may be. A page of text at 300 dpi is about 0.3 MB and a full-page photo a few MB.
pub const MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// The most files a print keeps a worker open on at once: an edited tab can hold pages from several PDFs, and each worker costs memory.
const MAX_OPEN_FILES: usize = 2;

/// One page to print: where it really is, and its unsaved quarter turns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintPage {
    pub path: PathBuf,
    /// From 0, in that file.
    pub page: u32,
    pub quarter_turns: u8,
}

/// How a print went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Printed {
    pub pages: u32,
    pub bytes: u64,
    pub worker_peak_bytes: u64,
    /// Starts over in a larger worker, past a memory limit.
    pub heavy_pages: u32,
}

/// Counts what's written, so offsets for the cross-reference table are known without seeking.
struct Counted<'a, O: Output> {
    out: &'a mut O,
    written: u64,
    cap: u64,
}

impl<O: Output> Write for Counted<'_, O> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.written.saturating_add(data.len() as u64) > self.cap {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        self.out.write_all(data)?;
        self.written += data.len() as u64;
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// A PDF being written: pages of one picture each, as they're drawn. Objects 1 and 2 are the catalogue and the page tree, written last, as only then are the pages known.
struct PdfOut<'a, O: Output> {
    out: Counted<'a, O>,
    offsets: Vec<u64>,
    pages: Vec<u32>,
}

impl<'a, O: Output> PdfOut<'a, O> {
    fn begin(out: &'a mut O, cap: u64) -> io::Result<Self> {
        let mut pdf = Self { out: Counted { out, written: 0, cap }, offsets: vec![0, 0], pages: Vec::new() };
        // The binary comment says to anything reading it that this isn't plain text.
        pdf.out.write_all(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n")?;
        Ok(pdf)
    }

    /// Starts the next object, and says its number.
    fn object(&mut self) -> io::Result<u32> {
        self.offsets.push(self.out.written);
        let number = self.offsets.len() as u32;
        write!(self.out, "{number} 0 obj\n")?;
        Ok(number)
    }

    fn finish(mut self) -> io::Result<u64> {
        for (index, body) in [(0usize, "<< /Type /Catalog /Pages 2 0 R >>".to_string()), (1, format!("<< /Type /Pages /Count {} /Kids [{}] >>", self.pages.len(), self.pages.iter().map(|page| format!("{page} 0 R")).collect::<Vec<_>>().join(" ")))] {
            self.offsets[index] = self.out.written;
            write!(self.out, "{} 0 obj\n{body}\nendobj\n", index + 1)?;
        }
        let xref = self.out.written;
        write!(self.out, "xref\n0 {}\n0000000000 65535 f \n", self.offsets.len() + 1)?;
        for offset in &self.offsets {
            write!(self.out, "{offset:010} 00000 n \n")?;
        }
        write!(self.out, "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", self.offsets.len() + 1)?;
        self.out.flush()?;
        Ok(self.out.written)
    }
}

fn write_failed() -> AgentError {
    AgentError::new("write_failed", "The print couldn't be written.", exit::FAILED)
}

fn from_io(error: &io::Error) -> AgentError {
    if error.kind() == io::ErrorKind::FileTooLarge { AgentError::new("output_too_large", "This print would be larger than RectoPDF will write.", exit::FAILED) } else { write_failed() }
}

/// The scale a resolution is, for the renderer: points are 1/72 inch.
pub fn scale_for(dpi: u32) -> f32 {
    dpi as f32 / 72.0
}

/// Prints `pages` into `out` as a PDF, whole or failing. `config` finds the worker; `progress` hears each page as it's done, and returning false stops the print. Can sleep waiting for the larger limit, so never on a UI thread.
pub(crate) fn print_into<O: Output>(config: &ServiceConfig, pages: &[PrintPage], dpi: u32, out: &mut O, mut progress: impl FnMut(u32, u32) -> bool) -> Result<Printed, AgentError> {
    if !(MIN_DPI..=MAX_DPI).contains(&dpi) {
        return Err(AgentError::new("bad_dpi", format!("A print is from {MIN_DPI} to {MAX_DPI} dots per inch."), exit::USAGE));
    }
    if pages.is_empty() {
        return Err(AgentError::new("no_pages", "There are no pages to print.", exit::FAILED));
    }
    if pages.len() > MAX_PAGES as usize {
        return Err(AgentError::new("too_many_pages", format!("A print is at most {MAX_PAGES} pages."), exit::FAILED).then("Print a range of pages at a time."));
    }
    let scale = scale_for(dpi);
    let caps = OutputCaps::default();
    let limit = Policy::default().heavy_limit_bytes;
    let mut open: Vec<(PathBuf, DocRenderer)> = Vec::new();
    let mut pdf = PdfOut::begin(out, MAX_BYTES).map_err(|error| from_io(&error))?;
    let mut printed = Printed::default();
    let mut surface = None;
    for (index, page) in pages.iter().enumerate() {
        if !progress(index as u32, pages.len() as u32) {
            return Err(AgentError::new("cancelled", "The print was stopped.", exit::FAILED));
        }
        // A worker per file, the most recent kept: pages of one file usually come together.
        let at = match open.iter().position(|(path, _)| *path == page.path) {
            Some(at) => at,
            None => {
                if open.len() >= MAX_OPEN_FILES {
                    open.remove(0);
                }
                open.push((page.path.clone(), DocRenderer::new(&page.path, None, config.clone(), Policy::default())?));
                open.len() - 1
            }
        };
        let doc = &mut open[at].1;
        let (width_points, height_points) = doc.page_size(page.page)?;
        let restarts_before = doc.stats().heavy_retries;
        let image = pdf.object().map_err(|error| from_io(&error))?;
        let length = image + 1;
        let mut size = None;
        // Compressed into a small buffer that's handed to the file after each strip, so nothing of the page piles up.
        let mut encoder = ZlibEncoder::new(Vec::with_capacity(256 * 1024), Compression::fast());
        let mut sink_error = None;
        let mut row = Vec::new();
        let mut rgb = Vec::new();
        let mut stream_start = 0;
        let counted = &mut pdf.out;
        // The image's dictionary can only be written once its size is known, which `Begin` says, so the stream starts there.
        let drawn = draw_whole_page(doc, page.page, scale, 0, &caps, &mut surface, |event| {
            let step = (|| -> io::Result<()> {
                match event {
                    PageEvent::Begin { width, height } => {
                        size = Some((width, height));
                        row = vec![0u8; width as usize * BYTES_PER_PIXEL as usize];
                        rgb = vec![0u8; width as usize * 3];
                        write!(counted, "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {length} 0 R >>\nstream\n")?;
                        stream_start = counted.written;
                    }
                    PageEvent::Rows { memory, rect, stride, .. } => {
                        for y in 0..rect.height as usize {
                            // A copy out, never a look in place: the worker still holds this memory, and it isn't trusted.
                            if !memory.copy_out(y * stride as usize, &mut row) {
                                return Err(io::ErrorKind::InvalidData.into());
                            }
                            for (to, from) in rgb.as_chunks_mut::<3>().0.iter_mut().zip(row.as_chunks::<{ BYTES_PER_PIXEL as usize }>().0) {
                                *to = [from[2], from[1], from[0]];
                            }
                            encoder.write_all(&rgb)?;
                        }
                        counted.write_all(encoder.get_ref())?;
                        encoder.get_mut().clear();
                    }
                }
                Ok(())
            })();
            step.map_err(|error| {
                sink_error = Some(from_io(&error));
                RenderFailure::Unavailable
            })
        });
        let drawn = drawn.map_err(|failure| {
            sink_error.take().unwrap_or_else(|| match (failure, size) {
                (RenderFailure::ExportRefused { why: ExportRefusal::TooHeavy, .. }, Some(_)) => {
                    let mut error = AgentError::from(failure);
                    error.message = format!("Page {} wasn't printed, and nothing was written: drawing it needs more than the {} one page may use.", index + 1, crate::human::bytes(limit));
                    error
                }
                _ => failure.into(),
            })
        })?;
        let rest = encoder.finish().map_err(|error| from_io(&error))?;
        counted.write_all(&rest).map_err(|error| from_io(&error))?;
        let stream_length = counted.written - stream_start;
        counted.write_all(b"\nendstream\nendobj\n").map_err(|error| from_io(&error))?;
        let _ = drawn;
        if pdf.object().map_err(|error| from_io(&error))? != length {
            return Err(write_failed());
        }
        write!(pdf.out, "{stream_length}\nendobj\n").map_err(|error| from_io(&error))?;
        let content = format!("q {width_points} 0 0 {height_points} 0 0 cm /Im Do Q");
        let contents = pdf.object().map_err(|error| from_io(&error))?;
        write!(pdf.out, "<< /Length {} >>\nstream\n{content}\nendstream\nendobj\n", content.len()).map_err(|error| from_io(&error))?;
        let page_object = pdf.object().map_err(|error| from_io(&error))?;
        write!(pdf.out, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width_points} {height_points}] /Rotate {} /Resources << /XObject << /Im {image} 0 R >> >> /Contents {contents} 0 R >>\nendobj\n", u32::from(page.quarter_turns % 4) * 90).map_err(|error| from_io(&error))?;
        pdf.pages.push(page_object);
        printed.pages += 1;
        printed.heavy_pages += u32::from(doc.stats().heavy_retries > restarts_before);
        printed.worker_peak_bytes = printed.worker_peak_bytes.max(doc.stats().peak_footprint_bytes);
    }
    progress(pages.len() as u32, pages.len() as u32);
    printed.bytes = pdf.finish().map_err(|error| from_io(&error))?;
    Ok(printed)
}

/// The app's print: pages to a file of the app's own, which the system's printing then takes, with `progress` hearing each page and able to stop it. The file is made new, and is the caller's to remove. Can wait for the larger limit, so it's for a job thread.
pub fn print_to_path(pages: &[PrintPage], dpi: u32, out: &Path, progress: impl FnMut(u32, u32) -> bool) -> Result<Printed, AgentError> {
    let config = ServiceConfig::discover()?;
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(out).map_err(|_| write_failed())?;
    let mut written = FileOut(file);
    let printed = print_into(&config, pages, dpi, &mut written, progress);
    if printed.is_err() {
        drop(written);
        let _ = std::fs::remove_file(out);
    }
    printed
}

/// `recto print --to-pdf`: pages to a new PDF, through the same drawing the app prints with, whole or not at all.
pub(crate) fn print_to_file(pages: &[PrintPage], dpi: u32, out: &Path) -> Result<Value, AgentError> {
    crate::saving::check_export_path(out, &["pdf"], "PDF")?;
    // Never over anything, least of all the PDF being printed: only the lopdf writers change a PDF's bytes.
    if std::fs::symlink_metadata(out).is_ok() {
        return Err(output_exists());
    }
    let started = Instant::now();
    let config = ServiceConfig::discover()?;
    let (temp, file) = OwnTemp::create(out)?;
    let mut written = FileOut(file);
    let printed = print_into(&config, pages, dpi, &mut written, |_, _| true)?;
    drop(written);
    publish_picture(&temp.path, out)?;
    drop(temp);
    Ok(json!({
        "out": out,
        "pages": printed.pages,
        "dpi": dpi,
        "bytes": printed.bytes,
        "heavy_pages": printed.heavy_pages,
        "worker_peak_bytes": printed.worker_peak_bytes,
        "took_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "turned_pages": pages.iter().filter(|page| page.quarter_turns % 4 != 0).count(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::MemoryOut;

    /// Failure: the PDF a print writes isn't one a reader takes: a wrong offset in its table, a length that isn't its stream's, or a turn that doesn't reach the page. Read back by the lazy reader the app opens files with.
    #[test]
    fn a_print_is_a_pdf_with_a_page_for_each_and_their_turns() {
        let file = recto_testpdf::TempPdf::new("print.pdf", &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["One"])), recto_testpdf::Page::new(recto_testpdf::text_lines(&["Two"]))]));
        let pages = [PrintPage { path: file.0.clone(), page: 1, quarter_turns: 0 }, PrintPage { path: file.0.clone(), page: 0, quarter_turns: 1 }];
        let mut out = MemoryOut::new(64 * 1024 * 1024);
        let mut heard = Vec::new();
        let printed = print_into(&ServiceConfig::discover().unwrap(), &pages, 72, &mut out, |done, of| {
            heard.push((done, of));
            true
        })
        .unwrap();
        assert_eq!((printed.pages, printed.bytes), (2, out.bytes.len() as u64));
        assert_eq!(heard.last(), Some(&(2, 2)), "progress reaches the end");
        let printed_file = file.0.with_file_name("printed-out.pdf");
        std::fs::write(&printed_file, &out.bytes).unwrap();
        assert_eq!(recto_core::inspect(&printed_file).unwrap().page_count, 2);
        let text = String::from_utf8_lossy(&out.bytes);
        assert!(text.contains("/Rotate 90") && text.contains("/Rotate 0"), "the turn reaches its page");
        std::fs::remove_file(&printed_file).unwrap();
    }

    /// Failure: a print can be asked for at a resolution that costs gigabytes a page, or of nothing, or stopped and still look finished.
    #[test]
    fn a_print_refuses_what_it_shouldnt_do() {
        let file = recto_testpdf::TempPdf::new("print.pdf", &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["One"]))]));
        let page = PrintPage { path: file.0.clone(), page: 0, quarter_turns: 0 };
        let config = ServiceConfig::discover().unwrap();
        let mut out = MemoryOut::new(1024 * 1024);
        assert_eq!(print_into(&config, std::slice::from_ref(&page), 2400, &mut out, |_, _| true).unwrap_err().code, "bad_dpi");
        assert_eq!(print_into(&config, &[], 300, &mut out, |_, _| true).unwrap_err().code, "no_pages");
        assert_eq!(print_into(&config, std::slice::from_ref(&page), 300, &mut out, |_, _| false).unwrap_err().code, "cancelled");
        let mut tiny = MemoryOut::new(100);
        assert_eq!(print_into(&config, std::slice::from_ref(&page), 72, &mut tiny, |_, _| true).unwrap_err().code, "output_too_large");
    }
}
