//! A short, seeded run of every fuzz target, on stable, in the normal tests. It finds what the targets find on the first few mutations, and it keeps the targets themselves compiling and honest. The long runs are `tools/fuzz.sh`.
//!
//! `RECTO_FUZZ_ROUNDS` runs more rounds, on a different seed with `RECTO_FUZZ_SEED`. On this Mac's debug build the whole file takes a few seconds.

use recto_fuzz::smoke;

fn rounds(default: usize) -> usize {
    std::env::var("RECTO_FUZZ_ROUNDS").ok().and_then(|text| text.parse().ok()).unwrap_or(default)
}

fn seed() -> u64 {
    std::env::var("RECTO_FUZZ_SEED").ok().and_then(|text| text.parse().ok()).unwrap_or(20_261_107)
}

/// Failure: mutating a real or made PDF panics, hangs or bombs `inspect` or the signature walk, or what comes back breaks a cap or isn't clean to show.
#[test]
fn mutated_pdfs_do_no_harm() {
    smoke("pdf_bytes", seed(), rounds(400), 64 * 1024);
}

/// Failure: a file with a true cross-reference table and hostile structure (loops, dangling references, odd types) panics, hangs or bombs a walk.
#[test]
fn hostile_pdf_structure_does_no_harm() {
    smoke("pdf_tree", seed(), rounds(1500), 8 * 1024);
}

/// Failure: a decoder panics or allocates what a claimed length says, or a validator lets through a reply that doesn't match its job.
#[test]
fn what_a_worker_sends_is_refused_or_checked() {
    smoke("proto", seed(), rounds(3000), 64 * 1024);
}

/// Failure: repair loses or invents a character, an offset is out of bounds, or a hostile page takes a long time.
#[test]
fn hostile_pages_keep_their_characters() {
    recto_fuzz::targets::index::LONGEST_PAGE.store(20_000, std::sync::atomic::Ordering::Relaxed);
    smoke("index", seed(), rounds(300), 4 * 1024);
}

/// Failure: a click or a drag on a hostile page lands out of bounds, a highlight isn't a number or is off the page, a copy passes its room, or a page takes long enough to be felt on a mouse move.
#[test]
fn hostile_pages_can_be_selected_on() {
    recto_fuzz::targets::index::LONGEST_PAGE.store(20_000, std::sync::atomic::Ordering::Relaxed);
    smoke("select", seed(), rounds(300), 4 * 1024);
}

/// Failure: the review's ⌘C with the button held, and the drag going on: on an input that reaches it directly, because a seeded run in a smoke test's rounds didn't happen to. The selection moves up past where the copy has got to, and a subtraction underflowed, which killed the thread that draws. Seven bytes for the page's frame, 192 twice for its two pointer checks, and then what the page view does.
#[test]
fn a_drag_under_a_copy_does_no_harm() {
    let mut input = vec![0u8, 0, 0, 2, 3, 0, 0];
    input.extend_from_slice(&[0; 384]);
    input.push(1); // It starts with a copy waiting on a selection dragged up from page 4 to page 2.
    input.extend_from_slice(&[6, 0, 0, 1, 1]); // Drag on to page 6.
    input.extend_from_slice(&[6, 0, 0, 1, 9, 1, 1]); // Page 6 has been read, so the selection now starts at page 4, past the copy.
    input.extend_from_slice(&[0, 0, 0, 1, 8]); // The copy goes on.
    input.resize(1500, 0);
    recto_fuzz::targets::select::check(&input);
}

/// Failure: a JSON line, command line or level file panics, a cap doesn't hold, or a document's name reaches Off.
#[test]
fn commands_from_outside_do_no_harm() {
    smoke("agent", seed(), rounds(3000), 16 * 1024);
}

/// Failure: a client's bytes panic a job, stop the server answering, or keep the next client out.
#[test]
fn a_client_cant_break_the_server() {
    smoke("control", seed(), rounds(150), recto_fuzz::targets::control::MAX_INPUT);
    recto_fuzz::targets::control::shut_down();
}

/// Failure: a sequence of edits leaves the page order wrong, an undo that doesn't invert, `is_edited` out of step, an owner's step lost below Full, or a cap broken.
#[test]
fn edit_sequences_keep_their_invariants() {
    smoke("edits", seed(), rounds(100), 2 * 1024);
}

/// Failure: a damaged or hostile state record or journal panics, bombs, breaks a cap or gets as far as a list that panics when it's undone, on what's read at every launch. Each input also goes in with its checksums made good, or almost nothing would get past them.
#[test]
fn hostile_state_records_do_no_harm() {
    smoke("record", seed(), rounds(400), 8 * 1024);
}
