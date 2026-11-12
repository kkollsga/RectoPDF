# Fuzz audit, 2026-11-11

Phase 7's release item "Fuzzing on every parser of untrusted input" (`docs/PLAN.md`). Everything in the workspace that parses or interprets bytes or structures that came, directly or indirectly, from outside, which fuzz target reaches it, and whether that coverage is real: does the target get there with arbitrary input, or only with what's well formed. Found from the code-review graph (functions named for decoding, parsing, reading, loading, replaying and unpacking outside tests, and the callers of `recto-svg`, `recto_proto::contents` and `recto_index::chrome`), and from every `serde_json::from_*`, `read_private_file`, `fs::read*` and `read_to_*` call outside tests, each read.

**Real** means the target reaches the code with input it controls, past the checks in front of it: a checksum is put right, a structure is made well formed and then made hostile, or the bytes go in as they are and enough of them get through. **Weak** means it's reached, but rarely or only with a narrow kind of input. **None** means no target reaches it.

## Before: what was covered, and the gaps

| Entry point | Input source | Target | Coverage before |
|---|---|---|---|
| `recto_core::inspect`, outline, `/PageLabels`, title (`read_facts`) | PDF bytes | `pdf_bytes`, `pdf_tree` | Real: mutated real PDFs, and trees built from the bytes with a true xref |
| `recto_core::nav::page_links` | PDF bytes | `pdf_bytes`, `pdf_tree` | Real |
| `recto_core::agreement::structure_view` | PDF bytes | `pdf_bytes`, `pdf_tree` | Real |
| `recto_core::signatures` | PDF bytes | `pdf_bytes`, `pdf_tree` | Real |
| `recto_core::verdict::judge` | the PDF's outline | `pdf_bytes`, `pdf_tree` (through the session's sidebar and `contents`) | Real, indirectly |
| `recto_core::save` planner, copier, rewrite, signature rules, `keep_pages`'s rewrite | PDF bytes, the document's and added ones | `save_copy` | Real |
| `recto_core::page_effects` (what an SVG export asks of a page first: forms, images, patterns, graphics states, layers, inflating a page's drawing to look for inline JPEGs) | PDF bytes | none | **None** |
| `recto_core::copy_restricted` (`/P` of the encryption dictionary) | PDF bytes | none | **None** (one integer, through the same reader) |
| Encryption dictionaries, `/Info` | PDF bytes | `pdf_bytes` only through real files | **Weak**: the tree builder never wrote a trailer with either |
| Every `recto-proto` decoder and validator, `FrameReader` | renderer worker's replies | `proto` | Real |
| `recto_proto::contents::Records` and `StreamCheck` | worker's contents frames | `proto` | **Weak**: arbitrary bytes, almost none of which decode as a record stream that passes the rules |
| `recto_proto::jpeg::JpegScan` (host side, every JPEG an export passes on) | a PDF's JPEG bytes, through the worker | `proto`, only through `StreamCheck` | **Weak**: needs a valid image record first |
| `recto_proto::jpeg::{fix_header, scan_start, frame_components, profile_says_srgb}` (worker side) | a PDF's JPEG and ICC bytes | none | **None** |
| `recto-svg`'s writer, through `recto-agent`'s `write_record` (`export svg`, snapshots as SVG) | worker's contents records | none | **None** (unit tests in `recto-svg/tests/hostile.rs` only) |
| `recto_index::{read_page, normalize, hidden_report}`, `order::repair_line_order` | page text and boxes from the worker | `index` | Real |
| `recto_index::select` (`PageText`, hit-testing, ranges, copies), `recto_reader::selection` | page text and boxes from the worker, pointer input | `select` | Real |
| `recto_index::chrome` (running heads, footers, page numbers left out of a copy) | page text and positions from the worker, and its neighbours' | `select` | **Weak**: only through `PageText::build` on `index`'s page generator, each page compared with itself as its own neighbour, and `chrome_spans`, `page_number`, `framed_number`, `key` never on inputs made for them |
| `recto_index::rank` (search's words, terms, counts, snippets' places, BM25) | page text from the worker; an agent's query | none | **None** (its doc comment says it's pure so it can be fuzzed) |
| `recto_index::corpus` reader, build, agent folder | page-text corpus on disk | `corpus` | Real |
| `recto_index::generated::Generated::decode`, `recto_control::generating::read_answer` | kept generated contents on disk; the headings helper's stdout | `generated` | Real |
| `recto_control::indexing` (the search indexer's one-line report) | the indexer helper's stdout | none | **None**, and a bug: see below |
| `recto_session::record` (record, journal, places log), `sleep::decode_sleep`, `RecordStore::load` and writes | state files on disk | `record` | Real |
| `recto_control::record_store` kept-pages files (`kept-*.keptpages`): stamps, roll-off, carrying over an unconfirmed save, pruning, names in journals | files in the journals' folder; journals that name them | none | **None**: `record` writes no kept file |
| `recto_control::log::recent` (`recto log`) | the activity logs on disk | none | **None** (noted as such in `docs/engineering/crates/recto-fuzz.md`) |
| `recto_session::edit` (edit sequences, undo, redo) | owner's and agents' commands | `edits` | Real |
| `recto_agent::{parse, parse_cli, execute}`, selectors, page specs, `duration_ms`, `Level::parse` | socket lines, CLI words | `agent` | Real |
| `recto_control::levels::parse_stored` | the level file | `agent` | Real |
| `recto_control::server` (line reading, jobs, limits) | socket clients | `control` | Real |
| `recto_update::rules::parse_latest`, `zip::{read_directory, unpack_entries}` | `latest.json` and a release's zip, from the internet | `update` | Real |
| `recto_update::plan::{Plan::read, Outcome::read, read_launch_ok}`, `saved::Saved::read` | files the app and the installer leave for each other | none | **None** |
| `recto_platform::registry::parse_dump` | `lsregister -dump`, which lists what every app on the Mac calls itself | none | **None** |

### Not fuzzed here, by design (unchanged)

| Entry point | Why |
|---|---|
| PDFium (`recto-render`, the render worker) | Runs in a separate, killable, sandboxed worker. What comes back from it is what's fuzzed (`proto`, `index`, `select`, `chrome`, `svg`). |
| distillPDF, in `recto-headings` (Generate contents) | The owner's library, in its own workspace (`crates/recto-headings` is excluded from this one). Runs in a helper process watched from outside: killed past its memory and time, with image decoding off for 0.1.0. What it says is read as hostile and fuzzed (`generated`). Fuzzing distillPDF itself belongs in its repository. |
| `recto-indexer`'s and `recto-headings`' stdin | The job line, written by the app that started them. |
| `recto_control::client` (the CLI reading the app's answers) | The other end is the app's own server, on a socket in the user's private state folder. |
| `recto-svg::verify` | A strict reader of our own output, for tests and QA. The `svg` target uses it as its oracle. |
| `recto-ffi` | Calls from the app's own Swift side. The one file it reads, the old paper setting, is at most 16 bytes matched against a list of names. |
| `recto-agent`'s PNG and JPEG writers (`render`, snapshots) | Encoders of pixels the worker drew, whose size and stride the host has already checked (`proto`). Any bytes are valid pixels. |
| `recto-cli`'s QA scripts and stdin password | Development tools; the password line is capped and checked as UTF-8. |
| The lopdf fork's `/Prev` chain | Slow in proportion rather than a hang, as `docs/engineering/crates/recto-fuzz.md` notes. |

## What changed

| Gap | Now |
|---|---|
| `page_effects`, `copy_restricted` | `page_effects_bytes` and `copy_restricted_bytes`, the same walks on bytes, run by `pdf_bytes` and `pdf_tree` on pages 0, 1, 7 and `u32::MAX`. The tree builder now makes what the walk reads: resources with forms, images, patterns, graphics states and layers; image and form XObjects with JPEG and Flate filters, `/Decode`, `/DecodeParms`, `/SMask`, `/SMaskInData`; and page drawings with inline JPEGs, compressed and not. |
| Encryption, `/Info` | The tree builder now names an `/Info` or a `/Standard` encryption dictionary in the trailer, with versions, revisions, lengths, `/P` and crypt filters the bytes choose, a quarter of the time each. |
| Contents records, `recto-svg`, `JpegScan`, the worker's JPEG fixes | A new target, `svg`: records through `recto_agent::svg_in_memory`, which is `export svg`'s own route (`StreamCheck`, `write_record`, `SvgWriter`) with the worker and the file taken away, as the bytes say and as made by the worker's own writers with hostile numbers, clips, glyphs, groups, text that's markup and JPEGs of every shape, stripped or not and turned. What's written is held to its cap and passes `recto_svg::verify::check_document`. A route of its own runs `JpegScan` in pieces, stripping and not, and the worker's `fix_header`, `scan_start`, `frame_components` and `profile_says_srgb`, on made and raw JPEGs: a scan that doesn't strip writes nothing, stripping never grows a JPEG and is the same done twice, and a fixed header leaves the picture byte for byte. Of 2,000 random made inputs, 1,752 got through the rules to the writer. |
| Chrome detection | A new target, `chrome`: the text rules (`numbers`, `key`, `framed_number`, `page_number`) on arbitrary text and on pieces that look like page numbers, dates and running heads; `chrome_spans` on edge pieces made from the bytes, NaN and infinities among them, up to twice a page's cap, with up to eight neighbours any number of pages away, `i64::MIN` and `MAX` included; and documents of up to five pages laid out with a running head and a footer set on every page with its own number, or now and then thousands of rows in the top band, or rows that are anything, each built by `PageText::build` and compared with the others as a copy compares them, then copied and boxed. |
| `rank` | `index` now runs search's ranking on each page's normalised text, with queries from the page itself and ones with special lower-casing and Han: words cut on characters' edges, counts no more than the page's words, a snippet's place on a character's edge, and a finite score whatever the collection says. |
| Kept-pages files | A new target, `kept`: a real kept file made by `keep_pages_of` and a save's rebase, a journal naming it, open or closed, then the journal's word for it changed (another path inside the folder or out, another stamp, pages, kind, count) and the file damaged (removed, rewritten in place with its time put back, grown, cut, emptied, replaced by a link, a folder or a copy with the same time, made unreadable, made old, with old strays beside it, or the folder made read-only). Then two launches, a restore, the closed history taken up, and the record written and verified. Nothing outside the journals' folder changes, and no page a restored list shows, at any step back or forward, is from a kept file that isn't what it was. |
| `recto log` | `record` now writes its input as the activity logs and reads them back through `log::recent`: no more entries than asked for, and every word of ours one clean line. |
| The plan, the outcome, the saved settings, the launch line, `lsregister` | `update` now reads its input as each of them, and holds each to what its reader says it keeps. |
| The indexer's report | `generated` now reads its input, and its well-formed shape, as the indexer's report through the new `indexing::read_report`. |

### Bugs found

**The indexer's report was believed as it was said** (found by reading, `eda8145`, with `c669862` keeping its memory counts, which `budget index` reports). `recto-indexer` reads a hostile PDF through a worker, and what it printed on stdout went on unchecked: its `state` as any text, its `code` as any JSON up to 64 KiB, into the `indexing_finished` event, the failure a search is refused with for a minute, and `budget index`'s answer. `generating.rs` already read the contents helper this way. Fixed in its own commit, `read_report`: one of five states, a code of at most 48 lower-case letters, digits and `_`, and the counts the helper gives, with a regression test (`a_helpers_report_is_believed_only_as_far_as_it_checks`).

**A journal could insert more pages of an added PDF than it has** (found by `record` after 17 runs, `de85e60`). Replay took `Insert(at, source, count)` with any count, so a journal adding a one-page PDF and inserting two of its pages left a list with a page past the file's end: nothing can draw it, and what the list writes doesn't read back, which every hand-off and compaction checks. Live inserts always use the file's own count. The input is the regression test `a_journal_cant_insert_more_pages_than_a_file_has`.

**A refused insert left its PDF on the list** (found by `edits` in the first combined run, after 3,788 runs, `1949f64`). `insert_pdf` took the file onto the list before committing the step, so an insert refused there, as an agent's that would push the owner's steps out of the history is, left the file listed and journaled with nothing using it: it counted towards the limit on files, and the next file added got the number after it. The refused file and its journal line now go with the step: `a_refused_insert_leaves_no_file_behind`.

**An agent folder's manifest could be a link** (found by `corpus` in the second combined run, after 2,698 runs, `793b6be`). `make_agent_folder` took the folder as finished when `manifest.json` was a file by `is_file`, which follows a link, so a link in its place was what an agent would read as the folder's index, and `agent_folder_bytes` said the folder was made. Each page's file was already held to `symlink_metadata`; the manifest now is too: `a_link_in_the_manifests_place_is_replaced_not_followed`. It only showed with the state folder under `/tmp`, itself a link, as `tools/fuzz.sh` puts it, where the note the link pointed at was taken away by eviction; the seeded smoke run's canonical temp folder never reached it.

**The `kept` target's own damage wrote through a link** (found by `kept` after 1,807 runs, `1f5c2fb`): a kept file replaced by a link to the PDF and then rewritten in place changed the PDF, and the check blamed the store, which had done nothing. Damage to a file's bytes, time or mode now skips a path that isn't a plain file; the input is `damage_to_a_kept_file_never_goes_through_a_link`.

## After: what's left

### Runs

`tools/fuzz.sh --seconds 180`, each target under the build queue's lock in turn, on this Mac (M4, 16 GB), **under load**: two other workers' builds and tests shared the queue's three slots. Peak memory is libFuzzer's own `peak_rss_mb`, the fuzzing process's resident size, not `phys_footprint`.

| Target | Runs in 180 s | Per second | Peak RSS | Result |
|---|---:|---:|---:|---|
| `chrome` (new) | 95,474 | 527 | 69 MB | no failure |
| `svg` (new) | 682,587 | 3,771 | 50 MB | no failure |
| `kept` (new) | 1,807, then 7,507 after the fix | 41 | 46 MB | the target's own bug at 1,807, fixed; no failure after |
| `pdf_bytes` | 41,614 | 229 | 161 MB | no failure |
| `pdf_tree` | 230,807 | 1,275 | 51 MB | no failure |
| `save_copy` | 134,339 | 742 | 56 MB | no failure |
| `index` | 1,708 | 9 | 678 MB | no failure |
| `update` | 68,865 | 380 | 40 MB | no failure |
| `generated` | 1,231,407 | 6,803 | 44 MB | no failure |
| `record` | 17, then 3,793 after the fix | 20 | 45 MB | the replay bug at 17, fixed; no failure after |
| `edits` (after its fix) | 10,336 | 55 | 66 MB | no failure |
| `corpus` (after its fix) | 9,192 | 50 | 40 MB | no failure |

`index` runs slowly because a page can be at the wire's cap of 2,000,000 characters, and each is read four ways; how much of that the new ranking checks add wasn't measured, as there's no run of this target from before to compare with. Its 678 MB is libFuzzer's resident size after many such pages, under the guard's 1 GiB on live bytes.

### The combined run, 2026-11-11

Every target, 60 s each, `tools/fuzz.sh --seconds 60`, going on from the corpora above, under the same load. The first two combined runs each found a bug, in `edits` and then in `corpus` (above); each was fixed and that target run again for 180 s. This is the third, on the code handed back (`793b6be`): **1,811,921 runs, no failure.**

| Target | Runs | Per second | Peak RSS |
|---|---:|---:|---:|
| `pdf_bytes` | 13,528 | 221 | 137 MB |
| `pdf_tree` | 74,704 | 1,224 | 51 MB |
| `save_copy` | 27,866 | 456 | 52 MB |
| `proto` | 657,988 | 10,786 | 40 MB |
| `index` | 478 | 5 | 682 MB |
| `select` | 34,354 | 563 | 633 MB |
| `agent` | 370,093 | 6,067 | 52 MB |
| `control` | 2,639 | 41 | 146 MB |
| `edits` | 4,706 | 77 | 70 MB |
| `record` | 1,386 | 22 | 43 MB |
| `update` | 18,175 | 297 | 41 MB |
| `generated` | 358,481 | 5,876 | 48 MB |
| `corpus` | 3,120 | 51 | 41 MB |
| `chrome` | 19,002 | 311 | 59 MB |
| `svg` | 223,026 | 3,656 | 57 MB |
| `kept` | 2,375 | 38 | 45 MB |

## What's left

Every entry point in this workspace that reads what came from outside is now reached by a target with input it controls. Left:

- **distillPDF, through `recto-headings`** (Generate contents): the heading finder parses a hostile PDF, in the owner's library, and `recto-headings`' own `plain` and `keep_visible` work on what it found. The crate is its own workspace, excluded from this one so distillPDF's lopdf features stay out of the app, so `recto-fuzz` can't depend on it. It runs in a helper watched from outside, killed past its memory and time, with image decoding off, and what it says is read as hostile (`generated`). A fuzz setup of its own in that workspace, or in distillPDF's repository, would close it.
- **PDFium**, by design: in a separate, sandboxed, killable worker, whose every reply is checked and fuzzed here.

