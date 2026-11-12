# recto-testpdf and recto-fuzz

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-testpdf`, `recto-fuzz` | **Started** | Test-only: PDFs written for tests, and one check on bytes for each thing that takes hostile input. Nothing that ships depends on these | Be depended on by anything that ships |

## Dependencies

| Crate | Why |
|---|---|
| `libfuzzer-sys` | `fuzz/` only, which is outside the workspace: nothing in the normal build sees it |

## Fuzzing

**One check per thing that takes hostile input,** in `crates/recto-fuzz/src/targets/`, a plain function on bytes. `tests/smoke.rs` runs each on stable with a fixed seed and a mutator that needs no crate, so every `tools/check.sh` runs them, and a failure repeats on any machine. `fuzz/`, outside the workspace, runs the same functions under libFuzzer, on nightly. `tools/fuzz.sh --seconds 300 [target ...]` builds and runs them, each under the heavy lock and releasing it between targets, keeping its corpus and any crash in `target/fuzz-work`. A longer campaign is the same with `--seconds 3600` overnight, eight hours in all, since a run picks up from its corpus. Each target's whole log is in `target/fuzz-work/<target>.log`, and the build queue won't run a command again on the same code after it fails, so a rerun after a crash takes `HEAVY_RETRY=1`. `RECTO_FUZZ_ROUNDS` and `RECTO_FUZZ_SEED` run the stable loop for longer, with no coverage feedback, if nightly isn't to hand.

| Target | What it runs | What must hold |
|---|---|---|
| `pdf_bytes`, `pdf_tree` | `inspect`, `signatures`, the sidebar and `contents` on mutated real PDFs, and on PDFs built from the bytes with a true cross-reference table | Returns, within 30 s and 512 MiB. Caps hold, text is clean, "none" only said of what was read |
| `proto` | Every decoder and validator, and `FrameReader` on a real channel | No allocation before the bytes arrive, decode and encode agree, a reply that differs from its job is refused |
| `index` | `repair_line_order`, `read_page`, `normalize`, `hidden_report`, up to the wire's 2,000,000 characters, with NaN, infinities and huge boxes | Only whitespace is added or lost, offsets in bounds, within 60 s |
| `agent` | JSON lines and command lines through `parse`, `parse_cli` and `execute`, selectors, page specs, level files | Caps hold, what the CLI builds the socket takes, a tab's name doesn't reach Off |
| `control` | Bytes from a client to the real server on a real socket | Every answer is a line of JSON, no job panics, `status` is answered, and the next client gets in |
| `record` | Bytes as a main record, a journal and a log of places, as they are and with their checksums made good | No panic or bomb, caps hold, and what's accepted is restored, undone and redone all the way, and written back the same |
| `edits` | Random steps by the owner and by agents through `execute` at Control, against a plain model | The order is the model's, undo inverts, `is_edited` means it differs, no owner's step is lost below Full, caps hold |
| `chrome` | Chrome detection's text rules, `chrome_spans` on edge pieces made from the bytes with neighbours any distance away, and documents of up to five pages with running heads and footers, or thousands of rows at an edge, compared as a copy compares them | What's left out is the page's own pieces, inside its text; caps hold; the rules on text give what they say |
| `svg` | Contents records through `recto_agent::svg_in_memory`, `export svg`'s own route, as the bytes say and made by the worker's writers; `JpegScan` and the worker's JPEG header fixes | Within its cap, and what's written passes `recto_svg::verify`; a header fix leaves the picture as it was |
| `kept` | A kept-pages file and the journal naming it, both damaged, through two launches, a restore and a closed history taken up | Nothing outside the journals' folder changes; no page shown at any step is from a kept file that isn't what it was |

**What a hostile page costs in the normaliser.** At the wire's 2,000,000 characters, each with the hyphen flag, a read took 5.2 s and takes 64 ms (release, without LTO, this Mac, one run on the page the `index` target found): only the first 4,096 markers on a page have their words looked at, and the search for a page's own spelling reads at most 32 MiB. Neither is near a real page: all of bench100 has 13,924 markers across 5,872 pages. Past the first cap a hyphen stays a hyphen, which on such a page is no loss.

**A hang or a bomb is a failure.** `recto_fuzz::guard` counts every allocation in the process, ends it past 1 GiB, and holds each check to its time and memory. A fuzz build has debug assertions and overflow checks on, and the default sanitizer is none, for speed: everything here is safe Rust but the OS seam, and `FUZZ_SANITIZER=address tools/fuzz.sh control` covers that. **An open question:** under that sanitizer with libFuzzer's leak check on, two runs ended with the server not answering a new client, on a different input each time and not on replay. With the check off, which is what `tools/fuzz.sh` now does, two runs didn't, and no run without the sanitizer has. So the leak check is the likely cause, which isn't proved. `select` holds hit-testing, ranges and copies to the same on hostile characters, boxes, frames and pointer sequences, and goes through the page view's `Selecting` with text arriving or failing at any moment and a copy under way. `docs/reports/fuzz-audit-2026-11-11.md` lists every entry point and the target that reaches it. **Not covered:** PDFium, which runs in a worker that can be killed. distillPDF, in the headings helper, which is watched from outside and whose answer is fuzzed. The fork's `/Prev` chain, noted above, which is slow in proportion rather than a hang.
