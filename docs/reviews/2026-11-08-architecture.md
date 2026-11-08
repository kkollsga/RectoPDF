# RectoPDF architecture review: scalability, verbosity, compartmentalization

Reviewed 2026-11-08 at `main` = `74cf8ec`, read-only. Sources: the code-review graph (6,279 nodes: 4,351 functions, 536 structs, 177 enums, 34 traits, 131 `src/` files; `tests/` folders aren't in the graph, so they were measured with `wc`/`grep`), `git log`, and direct reads. Nothing was built, run or measured at run time.

---

## 1. Verdict

The **crate layer is good**: 19 crates, strictly layered as `docs/ENGINEERING.md` says, no cycles, the session crate free of rendering and toolkits, and one command table that feeds the CLI, help, levels and skill files. The problem sits **inside four hubs that every feature has to thread through**: `recto-session/src/lib.rs` (one `Session` struct with 34 fields and 180 methods), `recto-agent/src/lib.rs` (a 59-variant `Command` enum that each new command extends in 7 to 9 places across 3 files), `recto-ffi/src/lib.rs` (a 34-variant `CoreCall`, a 267-line `session_thread` match, a 191-line `reconcile` that builds the UI's view models, and 108 `with_session` calls), and the Swift `MainWindowController` and `QAChannel`. Each of the last 8 feature merges touched **7 to 11 of the 11 hub files** the coordinator listed. Two cheap, mechanical causes make the conflicts worse than they need to be. There is no `rustfmt`, so 2,097 lines run past 200 characters, and the struct literals and destructurings that list every field sit on one line each. And the restore test's `KNOWN_PATHS`, 297 entries, is unsorted, with new entries added at the top. The biggest testability gap: the FFI crate holds about 2,100 lines of real logic, including view-model rules, menu rules and the hand-off freeze, that are reachable only through the built Swift app (10 unit tests against 58 app tests). The target is per-concern components inside the session, per-feature command modules behind one macro list, and a toolkit-free `recto-app` crate between agent and FFI. The order below goes from cheapest to largest, each step mergeable alone.

---

## 2. Key numbers

### Crates (lines; `src` includes inline `#[cfg(test)]` modules)

| Crate | `src` lines | `tests/` lines | Depends on (recto-*) | Rebuilt when it changes (reverse deps) |
|---|---:|---:|---|---|
| recto-agent | 10,268 | 0 | core, platform, session, sign, proto, index, render-service, svg | control, ffi, cli, fuzz |
| recto-session | 9,541 | 166 | core, platform | agent, control, ffi, cli, fuzz |
| recto-reader | 7,922 | 754 | core, index, platform, proto, render-service | ffi, fuzz |
| recto-index | 7,262 | 0 | proto, core, platform | indexer, headings, reader, agent, control, ffi, cli, fuzz |
| recto-ffi | 6,474 | 0 | agent, control, index, core, platform, proto, reader, render-service, session, update | (Swift app) |
| recto-control | 6,134 | 772 | agent, core, index, platform, session | ffi, cli, fuzz |
| recto-update | 4,979 | 639 | platform | ffi, fuzz |
| recto-render | 4,086 | 535 | platform, proto | render-service and everything above it |
| recto-core | 3,865 | 1,247 | platform | 10 crates |
| recto-platform | 3,833 | 333 | none | all but proto, svg, sign, testpdf |
| recto-render-service | 3,409 | 3,204 | platform, proto, render | reader, agent, indexer, headings, control, ffi, cli, fuzz |
| recto-proto | 3,344 | 0 | none | most |
| recto-fuzz | 2,503 | 138 | 11 crates | none |
| recto-svg | 1,572 | 315 | none | agent and above |
| recto-cli | 1,349 | **12,247** | agent, control, core, index, platform, session | none |
| recto-headings (own workspace) | 601 | 157 | core, index, platform, proto, render-service | none |
| recto-testpdf / indexer / sign | 334 / 266 / 149 | | | |
| **Rust total** | **~99,200** including tests | | | |
| Swift, hand-written (`app/Sources/RectoPDF`) | 4,441 | | | `Generated/recto_ffi.swift` 7,591, untracked (good) |

**Dependency graph:** layered, **0 cycles**, and every edge goes down the list in ENGINEERING.md's "A crate may depend only on crates below it", checked crate by crate. The highest-coupling edges are `agent → session`, `control → agent`, and `ffi → everything`. `recto-agent` also pulls in `render-service`, `svg` and `index`, so the socket server (`recto-control`) recompiles on any renderer-host change.

### The 15 largest files (Rust; prod/test split where tests are inline)

| # | File | Lines | Of which inline tests |
|---:|---|---:|---:|
| 1 | `crates/recto-session/src/lib.rs` | 5,564 | 2,486 (from l.3079), 108 tests |
| 2 | `crates/recto-reader/src/tiles.rs` | 3,798 | 1,305 (from l.2494), 50 tests |
| 3 | `crates/recto-ffi/src/lib.rs` | 3,400 | 155 (from l.3246), **10 tests** |
| 4 | `crates/recto-agent/src/lib.rs` | 2,780 | 905 (from l.1876), 40 tests |
| 5 | `crates/recto-render/src/pdfium/contents.rs` | 2,564 | |
| 6 | `crates/recto-cli/tests/restore.rs` | 2,243 | (all test) |
| 7 | `crates/recto-control/src/record_store.rs` | 2,213 | 908 |
| 8 | `crates/recto-render-service/src/host.rs` | 1,994 | |
| 9 | `crates/recto-cli/tests/app.rs` | 1,959 | (all test) |
| 10 | `crates/recto-agent/src/table.rs` | 1,918 | 507 |
| 11 | `crates/recto-update/src/updater.rs` | 1,761 | |
| 12 | `crates/recto-session/src/record.rs` | 1,708 | 331 |
| 13 | `crates/recto-index/src/select.rs` | 1,543 | |
| 14 | `crates/recto-index/src/corpus.rs` | 1,542 | |
| 15 | `crates/recto-session/src/edit.rs` | 1,413 | 475 |
| Swift | `Sidebar.swift` 1,180, `MainWindow.swift` 1,077, `PageArea.swift` 752, `QAChannel.swift` 726, `App.swift` 483 | | |

### God objects and central switches (graph: `Struct.fields`, `HAS_METHOD`, `Enum.variants`)

| Thing | Where | Size |
|---|---|---|
| `Session` | `recto-session/src/lib.rs:702` | **34 fields, 180 methods** (1,761 lines of methods), one `impl` block, l.870–2972, about 10 concerns |
| `Reader` (page view) | `recto-reader/src/tiles.rs:504` | **40 fields, 141 methods** |
| `Inner` (app core) | `recto-ffi/src/lib.rs:915` | **34 fields**; `impl Inner` blocks in 4 files (`lib.rs`, `saver.rs`, `sleeper.rs`, `updates.rs`) and 21 more `fn(inner: &Inner)` |
| `ReaderView` (UniFFI record) | `recto-ffi/src/lib.rs:347` | **32 fields**, built in one 1-line literal at l.3215 |
| `TabRecord` | `recto-session/src/lib.rs:61` | 23 fields (held per tab, ≤256 B asserted) |
| `Command` enum | `recto-agent/src/lib.rs:132–542` | **59 variants, 410 lines** of definition |
| `execute_held` | `recto-agent/src/lib.rs:1012` | 262 lines, **120 branches** |
| `CoreCall` enum | `recto-ffi/src/lib.rs:693–755` | **34 variants** |
| `session_thread` | `recto-ffi/src/lib.rs:2377` | 267 lines, **88 branches** (the longest function in the code) |
| `reconcile` | `recto-ffi/src/lib.rs:3054` | 191 lines, 35 branches, 6 separate session locks |
| `QAChannel.handle` | `QAChannel.swift:31–280` | ~50 string verbs in one `switch` |
| `MainWindowController` | `MainWindow.swift:218–1077` | 56 stored properties and 77 functions in the file, about 9 responsibilities |

### Files every feature touches (git, last 16 merges on `main`)

| Merge | Hub files touched (of the 11 listed by the coordinator) |
|---|---:|
| links (`433f88f`) | 10 |
| Save stage 1 (`745a12e`) | 10 |
| comparison view (`709352a`) | 10 |
| generated contents (`c1f8151`) | 10 |
| reader layout round (`e817c0c`) | 11 |
| hands-off updates (`6e6b414`) | 9 |
| deep sleep (`4125671`) | 7 |
| session restore (`7033bcc`) | 11 |
| small fixes (`d27a0e7`, `223a24e`, `8125488`, ...) | 0–7 |

Over all history: `agent/table.rs` 85 commits, `agent/lib.rs` 60, `session/lib.rs` 56, `ffi/lib.rs` 51, `cli/tests/app.rs` 48, `ffi/qa.rs` 36, `QAChannel.swift` 35, `MainWindow.swift` 29, `restore.rs` 29, out of 455 non-merge commits. `docs/PROCESS.md:94` and `:131` already record the conflicts ("conflicts in session restore and the app bridge"; "worth splitting `recto-ffi/src/lib.rs`?").

---

## 3. Findings

Severity: **High** = blocks the owner's goal on every feature; **Medium** = a real, recurring cost; **Low** = local.

### 3.1 Compartmentalization

**C1. `Session` is a god struct holding about 10 concerns behind one `impl`. High.**
- *Evidence:* `recto-session/src/lib.rs:702–776`: 34 fields. They are tab registry (`tabs`, `dirs`, `dir_ids`, `next_id`, `active`), warm tier (`warm`, `left_warm`, `woken`, `just_read`, `woken_from_file`), edits and journals (`edits`, `journals`, `fresh_journals`, `retired_journals`, `departing`, `history`, `journaling`, `saving`, `saved_over`, `wake_failed`), sleep (`asleep`, `held`, `started`, `sleep_files`), contents (`generated`), navigation (`jumps`), comparison, view, find, edit mode, draws, `place_lost`, and a per-command flag `agent_at_full` (l.766: "Set for each command", which is a hidden argument passed through shared state). The graph lists 180 methods in one `impl<L: DocLoader> Session<L>` block (l.870–2972). The modules `compare.rs`, `trail.rs` and `sleep.rs` exist, but they hold data types; the behaviour stays on `Session`.
- *Why it matters:* every feature (links +321 lines, comparison, sleep, save) adds fields and methods to the same struct and file. The struct can't be tested one concern at a time, and touching it rebuilds agent, control, ffi, cli and fuzz.

**C2. The `Command` enum is spread across 7 to 9 places per command, in 3 files. High.**
- *Evidence:* adding one command means editing the variant (`agent/lib.rs:132–542`), `tag()` (l.545, exhaustive), `access()` (l.610, only for exceptions), `draws()` (l.628, a non-exhaustive `matches!`), `log_details()` (l.633, exhaustive), `reads_pages()` (l.666, non-exhaustive), `tab_selector()` (l.672, `_ => None`), the `execute_held` arm (l.1012), a `table.rs` row, and a `human.rs::render` arm (l.100, string tags). A command answered by the server, the windowless host or the app also needs `control/server.rs:482`, `control/host.rs:171` or `ffi/lib.rs:541/583/644`.
- *Fail-open lists:* three of these are allow-lists with a default. A new command that forgets `draws()` skips the server's `max_draw_jobs` cap (`server.rs:839`), which is the memory rule. One that forgets `tab_selector()` isn't `hold`/`touch`ed in `execute` (l.990–998), so its tab can go dormant mid-command.

**C3. `recto-ffi` holds rules, contrary to its contract. High.**
- *Evidence:* ENGINEERING.md says ffi "must not: hold rules". Yet `reconcile` (`ffi/lib.rs:3054–3244`) decides the notice precedence (`open_failure > save_failed > problem > session_notice > compare_note > engine note`, l.3208), when the Save icon shows (`save_button: shown_tab != 0 && (edited || saving_now)`, l.3215), the wording "Copy anyway" (l.3212) and "Couldn't rotate: …" (l.2563), which panes get labels, and when contents follow. `menus.rs` holds the whole menu bar, the enable rules (l.117) and the zoom label wording. `session_thread` sequences multi-step session transactions (for example `CloseAfterSave`: "An edit that came in between the save and now isn't in the file: the tab stays"), and there are **108 `with_session` calls in ffi** (71 in `lib.rs`).
- *Why it matters:* this is "Rust decides", but in the wrong crate. These rules sit behind UniFFI types and an `AppListener`, so they can't be unit-tested or reached from the CLI without the app.

**C4. `Inner` is the FFI's shared state object, with every module reaching all 34 fields. Medium.**
- *Evidence:* there are `impl Inner` blocks in `lib.rs:1011`, `saver.rs:300`, `sleeper.rs:142` and `updates.rs:123`, plus 21 more functions taking `inner: &Inner`. QA-only fields live in the production struct (`qa_hold_reads`, the update test clock).
- *Why it matters:* the file split is cosmetic. Saver, sleeper and updates are coupled through `Inner`'s fields, not through interfaces.

**C5. Two implementations of the record commands, and the level check in four places. Medium.**
- *Evidence:* `session show/save/restore/sleep` exist twice, for the windowless host (`control/host.rs:171–233`) and for the app (`ffi/lib.rs:541–580`). Both repeat `level < Full && holds_owner_edits()`, and the reply shapes differ: the app's `session save` has `total_us`, `write_us`, `force_us` and `verify_us`, while the windowless one has `total_ms` only. The level is checked in `server.rs:464`, `agent::execute` (l.987), `host.rs:176` ("Checked here too, as `execute` would") and `ffi run_counted` (l.669).
- *Why it matters:* one command can behave two ways depending on the host, and a fix lands in one copy only.

**C6. Events are strings with ad-hoc JSON, emitted by callers comparing state before and after. Medium.**
- *Evidence:* 37 `Event::new("…")` sites in 7 files: `page_changed` ×10, `tab_activated` ×7, `view_changed` ×6, `tab_closed` ×4. "If the active tab changed, emit `tab_activated`" is re-derived in `agent` Close (l.1063), `ffi` `CloseAfterSave` (l.2534) and four more ffi sites (l.2593, 2792, 2835, 3048). `table::event_group` (l.735) ends in `_ => "access"`, so a new event name lands in the wrong `watch --events` group without any error.

**C7. QA verbs have no single list. Medium.**
- *Evidence:* verb names appear as string literals in `QAChannel.swift:33–276` (~50 cases), in the `ffi/qa.rs::drive` validation list (l.246–330, the main list at l.303), in `ffi/lib.rs::qa_session_verb` (l.1102) and in `ffi/qa.rs::run` (l.214). This is unlike agent commands, which have `TABLE`. A verb added to Swift but not to the Rust list fails with "That isn't a QA command".

**C8. Other fail-open allow-lists in central code. Medium (latent bugs).**
- `CoreQueue::push` (`ffi/lib.rs:801`) freezes user actions during a hand-off with a `matches!` list. `CloseTab`, `CloseAnswered`, `ContentsAction` and `ToggleSection` are person-originated but not on it, so they are taken while a hand-off is writing. **Needs checking**; it may be intended.
- `menus::enabled` (l.117) ends in `_ => true`, so a new item that needs a document is enabled without one.
- Per AGENTS.md ("No bugs left behind"), each of these deserves a look and a test.

### 3.2 Testability

**T1. The app core is testable only through the built Swift app. High.**
- *Evidence:* `ffi/lib.rs` has 2,143 lines of function bodies and 10 unit tests. `session_thread`, `reconcile`, `CoreQueue` freezing, hand-off, reads and pane sync are exercised by `cli/tests/app.rs` (58 tests), `restore.rs` (45), `compare.rs` (7) and `long_session.rs` (1). All of these need `app/build/RectoPDF.app` and skip under `--quick` (`app.rs:14–25`; `RECTO_REQUIRE_APP` makes them fail under `--full`).
- *Why it matters:* the fast loop (`check.sh --quick`) never touches the code that changes in every feature merge. That matches the owner's note that the test program slows the team down: the only check is the slowest one.

**T2. `reconcile` mixes a pure projection with side effects. High, and the cheapest high-value fix.**
- *Evidence:* the same 191-line function builds `SidebarView`/`ReaderView` from session state (pure), drives page-view engines (`set_paper`, `set_count`, `lay_out`, `even_out_zoom`), and schedules the saver and sleeper. The projection is about 120 of those lines and needs nothing but `&Session` plus a pane snapshot.

**T3. `KNOWN_PATHS` is a valuable guard and the worst merge hotspot among the tests. Medium.**
- *Evidence:* `cli/tests/restore.rs:382–681` holds 297 entries plus 27 `EXCEPTIONS` (l.190). It is **not sorted**: `sort -c` fails at entry 3, because the links merge put `app.links.*` and `session.tabs[].back*` at the top, ahead of `app.activation`. Every feature that adds a `qa state` field edits this list. Being unsorted, branches collide at the head of it. The guard itself is right ("state is complete by construction", l.340) and should stay.

**T4. The app test harness is duplicated across test files. Medium.**
- *Evidence:* `struct Run` is defined in `app.rs`, `restore.rs` and `compare.rs`. `fn app_path` is in 4 files, `fn settled` in 3, `fn wait_for` in 3, and `long_document`/`chaptered` in 2. `tests/common/mod.rs` (241 lines) has none of them. A change to how a quiet run starts has to be made 3 or 4 times.

**T5. What's already testable in isolation (good).** `Session<L: DocLoader>` is generic over the disk, so its 108 inline tests run without files. `Reader` is toolkit-free with 50 inline tests plus `tests/engine.rs`. `table.rs` tests walk the whole command table. `recto-render-service` has 3,204 lines of its own tests, including hostile workers. The pattern exists; FFI just doesn't follow it.

### 3.3 Cost of a feature: Links traced end to end (`433f88f`, 38 files, +3,178 lines, 10 crates)

| Layer | Own new module | Threaded through a hub |
|---|---|---|
| core | `nav.rs` +938, `tests/nav.rs` +100 | `core/lib.rs` +37 |
| reader | `links.rs` +181 | `tiles.rs` +114, `engine.rs` +57 |
| session | `trail.rs` +201 | **`lib.rs` +321**, `sleep.rs` +32, `record.rs` +10 |
| agent | `linking.rs` +131 | **`lib.rs` +148** (variants `Links`, `Follow`, `Back`, `Forward`, `Goto{label}` in the enum plus `tag`, `log_details`, `tab_selector`, `reads_pages`, `execute_held`), `table.rs` +30, `human.rs` +26 |
| ffi | `linking.rs` +153 | **`lib.rs` +189** (`CoreCall::Jump/JumpTyped/Step`, `ReaderView` fields, exported methods), `menus.rs` +22, `qa.rs` +8 |
| Swift | none | **`MainWindow.swift` +136**, `PageArea.swift` +42, `QAChannel.swift` +22, `App.swift` +14 |
| tests and docs | `app/qa/links-shots.sh` | `cli/tests/app.rs` +81, `restore.rs` +17 (`KNOWN_PATHS`), fuzz +52, `ENGINEERING.md`, `PLAN.md`, `SPEC.md` |

Roughly **half the lines (≈1,600) went into new feature modules**, and the other half (≈1,500) into about **20 shared files**, 10 of them hubs. That is the merge-conflict surface. The per-feature modules already exist one per layer (`agent/linking.rs`, `ffi/linking.rs`, `session/trail.rs`, `reader/links.rs`). What's missing is a way for those modules to *register* commands, core calls, view fields and QA verbs without editing the hub.

### 3.4 Scalability

**S1. Compile-time coupling. Medium (structural; not measured, since no builds were allowed).**
- *Evidence:* a change to `recto-session` rebuilds session, agent, control, ffi, cli and fuzz: about 36k `src` lines plus 18 `recto-cli` integration-test binaries (12,247 lines), each linking the full tree. A change to `recto-core` rebuilds 10 crates. A change to `recto-render-service` reaches `recto-control` through `recto-agent`.
- *Why it matters:* the biggest crates are also the most-edited ones. Splitting *files* doesn't cut rebuilds. Moving app-core logic out of the UniFFI crate does cut regeneration of the Swift bindings and the Swift rebuild, since those follow from `recto-ffi` changes.

**S2. One `Mutex<Session>` for every client. Low to medium.**
- *Evidence:* `control/host.rs:43`. The session thread, socket job threads, `recto-look`, saver and sleeper all lock it. `reconcile` takes it **6 separate times** per pass (l.3057, 3068, 3155, 3214, 3236, 3239), so one model can mix state from before and after another thread's edit.
- *Mitigations already in place:* the UI thread reads a published snapshot (`Published`, l.905) and never takes the lock, and agents read files outside it (for example `PageInsert`, l.1200).
- *At scale:* with up to `MAX_TABS` 10,000 tabs, `to_record` builds every `TabEntry` with a `PathBuf` under that lock, and `Session::tab(id)` is a linear scan (`lib.rs:896`, about 58 callers). Neither was measured.

**S3. Per-tab layout is good.** A cold `TabRecord` is ≤256 B and asserted, and side maps (`edits`, `journals`, `asleep`) cost nothing for untouched tabs. This matches the "memory first" rule. Splitting `Session` into components (below) must keep `TabRecord` as one compact struct and not scatter per-tab state across components.

**S4. Growth.** `agent/table.rs` has the most commits (85) and grows by a row per command, which is fine because it's data. `Command`, `CoreCall`, `ReaderView` and `KNOWN_PATHS` grow by a variant, field or line in a shared file for every feature, which is the problem. At the current pace (8 large features in 16 merges), `Session` and `ffi/lib.rs` will pass 4k and 4.5k production lines within a phase or two.

### 3.5 Verbosity

**V1. No `rustfmt`, and one-line exhaustive literals guarantee textual conflicts. High (cheap fix).**
- *Evidence:* there's no `rustfmt.toml` and no `cargo fmt --check` in `tools/check.sh`. 2,097 lines run past 200 characters, 678 past 300, and the longest is 2,373. `Session::to_record` (`session/lib.rs:2197`) destructures all 34 `Session` fields and all 23 `TabRecord` fields **on one line each**. That's deliberate ("Every field is named here, with no `..`, so one added … doesn't compile until it's carried"), and it's a good guard. But two branches that each add a field both edit that one line, which always conflicts. `ReaderView { … 32 fields … }` (`ffi/lib.rs:3215`), `SidebarView` (l.3134), `SidebarRow` (×6 sites) and `AppState` (l.1360) are the same.
- *Why it matters:* this is the cheapest lever on the conflicts the owner sees. Multi-line, one field per line, git merges additions at different positions without conflict.

**V2. Doc comments mostly help, but they drift. Medium.**
- *Good:* failure-named test docs ("Failure: an update goes ahead while a document is being saved …", `ffi/lib.rs:3250`) and field docs that state a cost or cap (`TabRecord::trail`, `Session::generated`). Character share of comments in the hubs is 20–31% (session 30%, ffi 27%, agent 20%, `table.rs` 8%), with average doc lines around 128 characters. That's not burying the code by itself.
- *Bad, misattached after inserts or merges (5 found):*
  - `agent/lib.rs:933`: "Runs one command, noting what happened in `events`…" sits on `paper_name`.
  - `agent/lib.rs:666`: "The selector this command acts on…" sits on `reads_pages`.
  - `agent/lib.rs:944`: the QA doc sits on `only_the_user_quits`, and `qa_needs_app` has none.
  - `ffi/qa.rs:155`: two docs are stacked on `walled_workers`.
  - `control/host.rs:147`: the `with_session` doc sits on `try_with_session`.
- *Bad, stale:* `session/lib.rs:89` "Nothing sets it yet" on `selection`, but `note_selection`/`set_selection` (l.2085–2092) set it and restore carries it. `lib.rs:735` "Edit mode. Nothing draws it yet", but `ReaderView.edit_mode` drives the panel's Rotate.
- *Bad, duplicated by a merge:* `ffi/lib.rs:2573–2574`, two versions of the same comment in `CompareClick`.
- *History in docs ages fastest:* "(review)", "the first build …", owner quotes. Rule of thumb: the *what and why* stays at the item, and *history and measurements* go to ENGINEERING.md (it's already 886 lines and does this well).

**V3. Function length. Medium.** 14 non-test functions run over 120 lines: `session_thread` 267, `execute_held` 262, `cli qa::budget` 236, `reconcile` 191, `update zip::read_directory` 166, `core save::write` 151, … (graph: `end_line - line_number`). The first, second and fourth are the dispatch hubs above. The rest are parsers and writers, where length follows the format and is acceptable.

**V4. Swift. Medium.**
- *Size and policy:* Swift is small (4.4k lines) and mostly policy-free. The numeric constants are drawing metrics (`cornerMargin`, `rowHeight`).
- *Concentration:* `MainWindowController` holds the zoom level, the go-to field, the link hint and prompt sheet, the Save button and problem sheet, the copy notice, home, panes, placement and focus.
- *Duplication:* the zoom field and go-to field editing are parallel code (`startPageEditing`/`endPageEditing` at l.689–714 against the zoom pair at l.933–950, `zoom_type`/`page_type` and `zoom_escape`/`page_escape` QA verbs).
- *Stringly typed:* focus names `"sidebar"`/`"page"` cross the bridge as strings (`applyFocus`, l.429).
- *Dead:* `Theme.sidebarWidth = 272` (`Theme.swift:20`) is unused and duplicates Rust's `record::SIDEBAR_WIDTH`.

---

## 4. Target structure

Principle: **a feature lives in one module per layer and registers itself in one line**. The hubs become thin lists and facades. Exhaustiveness checks stay (they're the project's good habit), but each lives next to its own data, not in a shared one-liner.

### 4.1 Session: a facade over components

```
recto-session/src/
  lib.rs          Session facade (fields = components), TabId, SessionError, re-exports   (~400 lines)
  tabs.rs         TabRecord, registry, dir interning, tiers/warm cache, open/close/tap
  editing.rs      PageEdits per tab, undo/redo, saving set, saved_over        (wraps edit.rs)
  journal.rs      journals, fresh/retired/departing, history source
  sleep.rs        dormancy, held/touched, asleep lists                        (exists: grows)
  contents.rs     verdict, generated views, folds, sidebar rows
  trail.rs        jumps, back/forward                                         (exists: grows)
  compare.rs      comparison                                                  (exists: grows)
  view.rs         ViewSettings, find, edit mode, draws, place notes
  record.rs       to_record/restore, each component contributing its own part
  tests/<concern>.rs   the 2,486 inline test lines, split the same way
```

- Each component destructures only *its own* fields in its own `to_record` part. That keeps the "can't compile until carried" guard without one shared line.
- `agent_at_full` becomes a parameter (an `Actor { author, at_full }` passed to edit methods), not session state.
- `TabRecord` stays one compact struct. Components keep side maps keyed by `TabId` as now, so cold-tab cost doesn't change.

### 4.2 Commands: one macro list plus a module per feature

```rust
// recto-agent/src/commands/mod.rs: the only shared edit per new command is one line here
commands! {
    Open(open::Open)          => "open",
    Links(linking::Links)     => "links",
    Follow(linking::Follow)   => "follow",
    // ...
}
// each payload implements a trait with NO defaults for the risky parts:
pub trait CommandImpl {
    const SPEC: &'static [Spec];          // the table rows move next to the handler
    fn tab(&self) -> Option<&str>;        // required: no silent `_ => None`
    fn draws(&self) -> bool;              // required: memory cap can't be skipped
    fn reads_pages(&self) -> bool;
    fn log_details(&self) -> Value;
    fn run<A: SessionAccess>(&self, cx: &mut Cx<A>) -> Result<Value, AgentError>;
}
```

- The macro generates the serde enum (`tag = "cmd"`), `tag()`, the dispatch, and `TABLE` as the concatenation of each `SPEC`.
- Server-only and app-only commands implement `run` as "needs server" or "needs app". The host layer routes by a `Route` column, not by `matches!` lists in three crates.
- `human.rs` formatters move into the same module, as an optional `fn human(data: &Value) -> Option<String>`.
- Events become a typed `enum Event { PageChanged { tab, page }, TabActivated { tab }, … }`. Name and group come from one exhaustive `match`, and session methods return what changed (`Closed { id, now_active }`), so callers stop re-deriving events.

### 4.3 The app core: a new toolkit-free crate between agent/control and FFI

```
recto-app/         (no uniffi; depends on agent, control, reader, session, update)
  core.rs          AppCore logic: threads, queue, hand-off
  calls/           CoreCall split per feature: nav.rs, compare.rs, saving.rs, contents.rs, view.rs ...
                   enum CoreCall { Nav(NavCall), Compare(CompareCall), Save(SaveCall), ... }
                   each sub-enum: fn handle(&self, cx) and fn is_user_action(&self) -> bool (exhaustive)
  model.rs         pure: fn build(session: &Session, panes: &PaneState, ui: &UiState) -> (ReaderModel, SidebarModel)
  menus.rs, qa_verbs.rs (a table like agent's: name, value check, route Session|UiThread|Special, help)
  saver.rs, sleeper.rs, updates.rs   with narrow handles (Arc<SaverState>), not &Inner
recto-ffi/         UniFFI records and enums, From<model::X> conversions, #[uniffi::export] methods
                   that each delegate in one line. Target: < 1,500 lines.
```

- `session_thread` becomes `match call { CoreCall::Nav(c) => c.handle(cx), … }`, one arm per feature.
- `CoreQueue::push` asks `call.is_user_action()`, an exhaustive `match` with no `_`, so every new variant must decide.
- `model::build` gets unit tests with a fake `DocLoader` session, covering notice precedence, Save icon visibility and pane labels. It can also be printed by a `recto qa model` command, which serves CLI-first.

### 4.4 Restore's whole-state test

- **Now:** sort `KNOWN_PATHS` and split it by top-level area into `tests/restore/known/{app,session,reader,panes,update}.txt`, one path per line, sorted, loaded with `include_str!`. Additions at different places merge cleanly, and the failure message stays the same.
- **Later:** the `session.*` part can be checked from the record types instead of listed. Every `session.*` path in `qa state` must map to a serde field of `SessionRecord`/`TabEntry`/`AppRecord`, or to an `EXCEPTIONS` entry. That's a mechanical comparison over the types, and these types are already serde.

### 4.5 Swift

- `MainWindowController` keeps the window, layout and pane management. It gets child controllers:
  - `InlineEditField`, shared by the zoom level and go-to field, which removes the duplicated start/end/typed/escape code;
  - `SaveController` (icon, problem sheet, Save As panel);
  - `LinkController` (hint, prompt sheet);
  - `NoticeController`;
  - `HomeView`.
- `SidebarPane` is split into tabs list, contents list (with `FrameRunner` following) and panel.
- `QAChannel` verbs move into `extension QAChannel` files by area (`+Pointer`, `+Sidebar`, `+Window`, `+Shots`). Each Swift verb set is checked against Rust's `qa_verbs` table by a test, for example through the existing `entry_points` verb, so the lists can't drift.

### 4.6 Order of refactors (smallest first; each its own branch, own tests, mergeable alone)

Effort is one worker's days, including review. Every step is behaviour-preserving; its test is "the same suites pass" plus the named new test.

| # | Step | New test it brings | Effort |
|---:|---|---|---|
| 1 | Fix the drifted docs (V2), delete `Theme.sidebarWidth`, add an exhaustive `is_user_action` for `CoreCall` and decide `CloseTab`/`ToggleSection`/`ContentsAction`/`CloseAnswered` (C8). Make `menus::enabled` and `event_group` exhaustive over known ids, with tests that every menu id and every emitted event name is listed | "every person-originated call is frozen during a hand-off"; "every menu id has an enable rule" | 0.5–1 d |
| 2 | Sort and split `KNOWN_PATHS` into per-area files; move `Run`/`app_path`/`settled`/`wait_for`/document makers into `tests/common/app.rs` | a test that each known-paths file is sorted and has no duplicates | 0.5–1 d |
| 3 | Adopt `rustfmt` (`max_width` about 160, so prose comments aren't rewrapped: `wrap_comments = false`), one mechanical commit at a merge point with **no open branches**, add `.git-blame-ignore-revs`, add `cargo fmt --check` to `check.sh --quick` | the fmt check itself | 0.5 d, plus a quiet window |
| 4 | Extract the pure part of `reconcile` into `model.rs` (still in ffi), returning `ReaderView`/`SidebarView` from plain inputs | 10–20 unit tests: notice precedence, Save icon, pane labels, contents-version deltas | 2 d |
| 5 | Typed `Event` enum, with session methods returning change sets for close/tap/jump; delete the re-derived `tab_activated` logic | "every event has a group"; existing watch tests | 1–2 d |
| 6 | Split `Session`'s `impl` into per-concern files (`impl Session` blocks move with no struct change), and move its inline tests into `src/tests/<concern>.rs`. Pure moves, so reviews are easy | none needed (moves) | 1–2 d |
| 7 | Turn `Session`'s fields into component structs one concern at a time (comparison, trail, view first; journals and sleep last). Each component destructures its own fields in `to_record`. `agent_at_full` becomes a parameter | per-component unit tests, moved from the facade tests | 4–6 d over several merges |
| 8 | `commands!` macro and `CommandImpl` trait. Migrate one table group per branch (Start, Read, Move, Edit, Session, App, QA); the old `match` keeps the unmigrated rest. Then delete `tab()`, `log_details()` and the old matches | "unknown fields are still refused" for every variant (check serde's internally tagged and newtype behaviour here); "every command declares `draws`/`tab`"; the existing table walk | 4–6 d |
| 9 | Merge the two record-command implementations (C5) behind a `RecordHost` trait that windowless and app hosts implement; one level check at the server, with the others asserted by a test, not re-checked | the same reply shape from both hosts, by test | 1–2 d |
| 10 | Split `CoreCall` into per-feature sub-enums with `handle`, still inside ffi | the hand-off freeze test from step 1, now exhaustive per sub-enum | 2–3 d |
| 11 | Create `recto-app`: move steps 4 and 10's modules, saver, sleeper, updates and menus; narrow `Inner` into handles; `recto-ffi` becomes adapters only | `recto-app` unit tests with a fake listener; these run in `--quick` without the app | 4–6 d |
| 12 | QA verb table in `recto-app`, with Swift's verbs checked against it; split `QAChannel` into extension files | "Swift answers exactly the table's UI verbs" | 1–2 d |
| 13 | Swift child controllers and `InlineEditField` | existing app tests and shots, light and dark | 2–4 d |

**Total:** about 24–40 worker-days. Steps 1–3 (about 2–3 days) remove most of the *textual* conflicts. Steps 4, 6, 8 and 11 remove most of the *structural* hub-threading. Do the restore list and `rustfmt` before the next feature track starts, as both conflict with every open branch.

---

## 5. What's good and should stay

- **The crate graph:** 19 crates, layered, no cycles, each with a written "must not" in ENGINEERING.md that the Cargo manifests obey. `recto-session` depends only on core and platform; the renderer is a separate sandboxed process; `recto-headings` sits in its own workspace to keep distillPDF's features out of the app.
- **`agent/table.rs` as the one list of commands:** CLI grammar, help, levels, `SKILL.md` and `REFERENCE.md` come from it, with tests that walk every row. It's the model for the QA verb table and for the per-command `SPEC` in 4.2.
- **Seams for testing:** `DocLoader` on `Session`, `SessionAccess` on agent, the `Host` trait on control and `RenderBackend` on render. `Session`'s 108 tests and `Reader`'s 50 run without files or a UI.
- **The UI thread never takes the session lock:** `Published` is swapped whole under a short lock, and slow work goes to the session thread or a reader thread. That keeps rule 3.
- **Compile-time guards for state completeness** (`to_record`'s exhaustive destructure) and the whole-`qa state` restore test. Keep both; change only their layout (4.1, 4.4).
- **Per-item size asserts, caps on everything from outside, one fuzz target per hostile input,** and the clippy `disallowed-methods` wall around the one writer.
- **Swift is thin:** wording, ordering and enable rules come from Rust, the numeric constants are drawing metrics, and the generated bindings aren't committed.
- **Per-feature modules exist on every layer** (`nav.rs`/`links.rs`/`trail.rs`/`linking.rs` ×2, `compare.rs`/`panes.rs`, `saving.rs` ×2). The target structure extends this pattern; it doesn't replace it.

---

## 6. What wasn't checked

- **Build and test times:** nothing was built or run, so the compile-coupling and rebuild-set claims are structural, from Cargo manifests, not measured. Neither were runtime lock contention, `to_record` cost at 10,000 tabs, or linear `tab()` lookups.
- **Crates not read in depth:** render, render-service, index, update, proto, svg, platform and headings, beyond their sizes and longest functions. Their internal structure may hold findings of its own; `record_store.rs` (2,213 lines) and `render-service/host.rs` (1,994) are the next candidates.
- **Swift:** `Sidebar.swift` and `PageArea.swift` were sized and skimmed, not read line by line. Swift compile time wasn't looked at.
- **Correctness:** C8 is a possible bug; whether `CloseTab` during a hand-off is really reachable from the Swift side wasn't traced. No other correctness review was done.
- **Graph limits:** `tests/` directories aren't in the graph, and call edges to common names (`take`, `lock`, `find`) are mis-resolved, so fan-in was taken from type use and git co-change, not raw `CALLS` counts.
- **Out of scope:** `docs/PLAN.md` priorities, CI configuration, the C# or Windows future, and the lopdf fork.
- **Effort estimates** are judgement, not measured against this team's past velocity.
