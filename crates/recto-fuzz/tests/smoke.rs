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

/// Failure: Save's planner, copier or remapper panics, hangs or bombs on a hostile file added or saved, or writes something that doesn't read back with the pages it said.
#[test]
fn hostile_files_through_saves_copier_do_no_harm() {
    smoke("save_copy", seed(), rounds(400), 8 * 1024);
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

/// Failure: a forged kept file or a hostile helper's answer panics, bombs, or gets believed with a page past the end, a title on two lines or with invisible characters, or a depth or count past its cap.
#[test]
fn generated_contents_are_read_as_hostile() {
    smoke("generated", seed(), rounds(1500), 16 * 1024);
}

/// Failure: a page-text corpus damaged on disk, or forged with its checksums right, panics, bombs, makes an agent's folder past its cap, or leaves something a cleared rebuild doesn't read back as written. Each round builds and lays out a corpus, so this is short.
#[test]
fn damaged_corpora_do_no_harm() {
    smoke("corpus", seed(), rounds(12), 4 * 1024);
}

/// Failure: positions and text from hostile pages, NaN and huge values, thousands of rows at the edges, or neighbours nothing like their page, panic chrome detection, take long, or leave out what isn't one of the page's own pieces.
#[test]
fn hostile_pages_chrome_does_no_harm() {
    recto_fuzz::targets::chrome::MOST_CHARS.store(20_000, std::sync::atomic::Ordering::Relaxed);
    smoke("chrome", seed(), rounds(300), 4 * 1024);
}

/// Failure: records a compromised worker sends panic the stream's rules, the SVG writer or the JPEG scan, write past the cap, or reach the SVG as markup, a script or a way out of it.
#[test]
fn hostile_records_write_a_safe_svg() {
    smoke("svg", seed(), rounds(1500), 8 * 1024);
}

/// Failure: a kept file damaged on disk, or a journal naming it by another path or stamp, gets a file outside the journals' folder removed or changed, is undone from when it isn't what it was, or leaves a history that can't be undone and redone all the way. Each round makes a kept file with its syncs, so this is short.
#[test]
fn damaged_kept_files_do_no_harm() {
    smoke("kept", seed(), rounds(30), 1024);
}

/// Failure: the `kept` target's own damage, a kept file replaced by a link to the PDF and then rewritten in place, writes through the link into the PDF, and the check blames the store for it. The input libFuzzer found: a closed history whose journal names the canary, then made old, made a link, and rewritten.
#[test]
fn damage_to_a_kept_file_never_goes_through_a_link() {
    recto_fuzz::targets::kept::check(&[0x01, 0x01, 0x01, 0x00, 0x01, 0xf1, 0xff, 0x01, 0x82, 0x00, 0xb0, 0xff, 0xf7, 0xa0, 0x01, 0x00]);
}

/// Failure: a hostile `latest.json` or zip panics, or writes past what it said. The seeded run of what `tools/fuzz.sh` runs at length.
#[test]
fn update_holds() {
    smoke("update", seed(), rounds(2000), 16 * 1024);
}

/// Failure: the fuzz run only ever exercises the first few bytes of the zip reader, as its seed isn't one it takes.
#[test]
fn the_update_seed_is_a_zip_the_reader_takes() {
    let seeds = recto_fuzz::seeds::for_target("update");
    let entries = recto_update::zip::read_directory(&mut std::io::Cursor::new(&seeds[1]), "RectoPDF.app").unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(entries.len(), 1);
}

/// Every `.rs` file under a crate's `src`, as one text: what's written in the code, constant.
fn source_of(krate: &str) -> String {
    let mut text = String::new();
    let mut folders = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(krate).join("src")];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).expect("a source folder").flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|ending| ending == "rs") {
                text.push_str(&std::fs::read_to_string(&path).expect("a source file"));
            }
        }
    }
    text
}

/// What a request holds that an agent typed, as a refusal may echo it: each string cleaned, and each number.
fn agents_words(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => out.push(recto_core::clean_line(text)),
        serde_json::Value::Number(number) => out.push(number.to_string()),
        serde_json::Value::Array(items) => items.iter().for_each(|item| agents_words(item, out)),
        serde_json::Value::Object(fields) => fields.values().for_each(|item| agents_words(item, out)),
        _ => {}
    }
}

/// Failure (review of R2.5, F2, and its second review, S2): a refusal made as a request is read, before any level or tab is looked at, says something,
/// in its message or in what to do next, that's neither constant words nor the agent's own words cleaned: raw text an agent typed (a control character, a
/// bidi override), or anything else. Walks every refusal by a command's type that the malformed requests, the registry's golden requests and the agent
/// target's seeds reach, and mutations of them as the fuzz target makes: with the agent's cleaned words taken out, every word left in either is written in
/// the code of `recto-agent` or `recto-session`, and nothing else rides along.
#[test]
fn a_refusal_as_a_request_is_read_says_only_constant_words_and_the_agents_own() {
    let code = [source_of("recto-agent"), source_of("recto-session")].concat();
    let mut lines: Vec<String> = include_str!("../../recto-agent/tests/golden/malformed.txt")
        .lines()
        .map(|entry| entry.split('\t').next().unwrap_or(entry).to_string())
        .chain(include_str!("../../recto-agent/tests/golden/requests.txt").lines().map(str::to_string))
        .chain(recto_fuzz::seeds::json_lines())
        .collect();
    let mut rng = recto_fuzz::input::Rng::new(seed());
    for _ in 0..rounds(3000) {
        let mut bytes = lines[rng.below(lines.len())].clone().into_bytes();
        recto_fuzz::mutate::bytes(&mut bytes, &mut rng, 16 * 1024);
        lines.push(String::from_utf8_lossy(&bytes).into_owned());
    }
    let mut walked = 0;
    for line in &lines {
        let Err(refused) = recto_agent::read(line) else { continue };
        if refused.tag == recto_agent::UNKNOWN {
            continue;
        }
        walked += 1;
        let error = &refused.error;
        assert!(
            error.candidates.is_empty() && error.untrusted.is_none() && error.ask.is_none(),
            "{line}: {error:?}"
        );
        let mut words = Vec::new();
        agents_words(&serde_json::from_str(line).expect("a line read as a command is JSON"), &mut words);
        words.sort_by_key(|word| std::cmp::Reverse(word.len()));
        for told in [&error.message, &error.next] {
            let mut said = told.clone();
            for word in words.iter().filter(|word| !word.is_empty()) {
                said = said.replace(word.as_str(), " ");
            }
            // A list the code joins (`single, two-up, two-up-cover`) has its commas between words, not in them.
            for piece in said
                .split_whitespace()
                .map(|piece| piece.trim_end_matches([',', '.']))
                .filter(|piece| !piece.is_empty())
            {
                assert!(
                    code.contains(piece),
                    "{line}: `{piece}` in {told:?} is neither constant nor the agent's own words"
                );
            }
        }
    }
    assert!(walked >= 25, "the walk reached the types' refusals: {walked}");
}
