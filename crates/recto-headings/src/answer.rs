//! What the helper says, as lines: pure, so a test reads exactly what the helper writes.
//!
//! One line a page that has headings, in page order, and one end line (`docs/engineering/generated-contents.md`). The session reads every line as hostile (`recto_control::structure::headings`),
//! and refuses a line over [`MAX_LINE_BYTES`], so a page's line is cut to it here.

use recto_index::generated::visible::{Found, Headings, MAX_LINE_BYTES};
use serde_json::json;

/// One page's line: `{"page":N,"found":[{"level":L,"bbox":[x0,y0,x1,y1]|null,"title":T}]}`, `page` zero-based. While it's over [`MAX_LINE_BYTES`] entries go from the end; the number
/// kept is returned, and a caller that sees fewer than it gave says the answer is cut.
pub fn page_line(page: u32, found: &[Found]) -> (String, usize) {
    let mut line = format!("{{\"page\":{page},\"found\":[");
    let mut kept = 0;
    for entry in found {
        let bbox = entry
            .bbox
            .map_or_else(|| "null".to_string(), |[x0, y0, x1, y1]| format!("[{x0},{y0},{x1},{y1}]"));
        let title = serde_json::to_string(&entry.title).unwrap_or_else(|_| "\"\"".into());
        let one = format!("{{\"level\":{},\"bbox\":{bbox},\"title\":{title}}}", entry.level);
        // The closing `]}` and a comma are still to come.
        if line.len() + one.len() + 3 > MAX_LINE_BYTES {
            break;
        }
        if kept > 0 {
            line.push(',');
        }
        line.push_str(&one);
        kept += 1;
    }
    line.push_str("]}");
    (line, kept)
}

/// The last line: how the read ended. `sent` is how many entries went out in page lines, which the session checks against what it received.
pub fn end_line(headings: &Headings, sent: usize, truncated: bool, distilled_ms: u64, peak: Option<u64>) -> String {
    json!({
        "state": "ready",
        "pages": headings.pages,
        "pages_failed": headings.pages_failed,
        "truncated": truncated,
        "found": sent,
        "distilled_ms": distilled_ms,
        "helper_peak_bytes": peak,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn found(page: u32, title: &str, bbox: Option<[f32; 4]>) -> Found {
        Found {
            level: 2,
            page,
            bbox,
            title: title.into(),
        }
    }

    /// Failure: a page of long titles full of quotes and backslashes (each costs two bytes in JSON) makes a line longer than the session will read, so the whole answer is refused and a
    /// document with one dense page gets no contents at all. The line is cut from its end instead, and says how many it kept.
    #[test]
    fn a_page_line_is_cut_to_the_cap_and_says_how_many_it_kept() {
        let title = "\"\\".repeat(120);
        assert_eq!(title.len(), 240);
        let many: Vec<Found> = (0..300).map(|_| found(4, &title, Some([1.0, 2.0, 3.0, 4.0]))).collect();
        let (line, kept) = page_line(4, &many);
        assert!(line.len() <= MAX_LINE_BYTES, "{}", line.len());
        assert!(kept > 0 && kept < 300, "{kept}");
        let read: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(read["found"].as_array().unwrap().len(), kept, "and what it says is what is in it");
        assert_eq!(read["found"][0]["title"], title.as_str(), "the title reads back as it went out");
    }

    /// Failure: a box or a title doesn't read back as it went out (a float with an exponent, an unescaped quote), so the session sees another place or another word than distillPDF found.
    #[test]
    fn an_ordinary_page_reads_back_as_it_went_out() {
        let page = [found(2, "1.5 Results & \"more\"", Some([70.5, 700.25, 190.0, 712.0])), found(2, "No box", None)];
        let (line, kept) = page_line(2, &page);
        assert_eq!(kept, 2);
        let read: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(read["page"], 2);
        assert_eq!(read["found"][0]["level"], 2);
        assert_eq!(read["found"][0]["title"], "1.5 Results & \"more\"");
        let bbox: Vec<f32> = read["found"][0]["bbox"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_f64().unwrap() as f32)
            .collect();
        assert_eq!(bbox, [70.5, 700.25, 190.0, 712.0]);
        assert!(read["found"][1]["bbox"].is_null());
        assert_eq!(read.as_object().unwrap().len(), 2, "exactly a page and what was found");
    }

    /// Failure: the end line's numbers go under other names than the session reads, so every answer is refused as not adding up.
    #[test]
    fn the_end_line_has_the_names_the_session_reads() {
        let headings = Headings {
            pages: 9,
            pages_failed: 1,
            ..Headings::default()
        };
        let read: Value = serde_json::from_str(&end_line(&headings, 6, true, 120, Some(5))).unwrap();
        assert_eq!(read["state"], "ready");
        assert_eq!(
            (read["pages"].as_u64(), read["pages_failed"].as_u64(), read["found"].as_u64()),
            (Some(9), Some(1), Some(6))
        );
        assert_eq!(read["truncated"], true);
        assert_eq!(read["distilled_ms"], 120);
    }
}
