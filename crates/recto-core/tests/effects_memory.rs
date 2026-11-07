//! What reading a page's structure costs, in its own binary so a footprint means this and nothing else. It runs in the session's own process, so whatever a file can make it hold, every tab pays for.

use recto_core::page_effects;
use recto_platform::isolate::process_memory;
use recto_testpdf::{Object, Page, TempPdf, first_extra_object, pages_with, text_lines};
use std::time::{Duration, Instant};

/// A page with `forms` forms, every one naming one large resources dictionary.
fn shared_resources(forms: usize, pad_bytes: usize) -> Vec<u8> {
    let first = first_extra_object(1);
    let shared = first + forms;
    let names: String = (0..forms).map(|i| format!("/F{i} {} 0 R ", first + i)).collect();
    let mut objects: Vec<Object> = (0..forms).map(|_| Object::Stream { dict: format!("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources {shared} 0 R"), data: Vec::new() }).collect();
    objects.push(Object::Dict(format!("<< /Pad ({}) /ExtGState << /G << /BM /Multiply >> >> >>", "x".repeat(pad_bytes))));
    pages_with(&[Page::new(text_lines(&["Words"]))], &format!("/XObject << {names} >>"), objects)
}

/// Failure: a small hostile file takes the session's memory: thousands of forms name one dictionary of megabytes, each visit parses it again and keeps a copy to look at later, and the time limit isn't looked at while that happens. All tabs die with the process. 400 forms and 1 MiB would be 400 MiB of copies; it must be about one.
#[test]
fn forms_sharing_large_resources_cost_one_copy() {
    let file = TempPdf::new("shared.pdf", &shared_resources(400, 1024 * 1024));
    let before = process_memory(std::process::id()).unwrap().peak_footprint_bytes;
    let started = Instant::now();
    let effects = page_effects(&file.0, 0).expect("it's all readable, and small enough to finish");
    let elapsed = started.elapsed();
    let peak = process_memory(std::process::id()).unwrap().peak_footprint_bytes;
    let grew = peak.saturating_sub(before);
    println!("page_effects: grew {grew} bytes in {elapsed:?}");
    assert!(peak > 0, "never measured");
    assert!(effects.blend, "and it did look there");
    assert!(grew < 64 * 1024 * 1024, "the peak grew by {grew} bytes");
    assert!(elapsed < Duration::from_secs(20), "{elapsed:?}");
}

/// Failure: forms past the limit go on being followed, so there's no bound at all, or the answer's taken to be "uses nothing" though much of the file wasn't looked at. Past it there's no answer, which the caller reads as "may use anything".
#[test]
fn past_the_limit_there_is_no_answer() {
    let file = TempPdf::new("many.pdf", &shared_resources(9000, 16));
    assert_eq!(page_effects(&file.0, 0), None);
}
