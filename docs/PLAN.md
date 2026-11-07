# RectoPDF: plan to first release

Last updated 2026-11-07. Behaviour is in `SPEC.md`, and the reasons behind it are in `docs/research/`.

## Where we are

| Phase | What you get | Status | Size (agent-days) |
|---|---|---|---:|
| **0. Research and foundations** | Spec, concepts, benchmarks, first core | **Done** | |
| **1. CLI and renderer** | An agent can open, read and draw any PDF, and test everything without a window | **Done** | 5–8 |
| **2. First real app** | A native window you can read in, with real screenshots | **In progress** | 8–12 |
| **3. Complete reader** | Find, menus, ⌘K, session restore, image copy, snapshot and SVG | **In progress** | 10–15 |
| **4. Saving and editing** | Native Save, edit mode, annotations, forms, signature image | **Started:** the fork's writer is done | 25–40 |
| **5. Tools** | Compress, OCR, passwords, compare, redaction, watermarks and more | Not started | 20–35 |
| **6. Agent sidebar** | Ask a question about a PDF, with your own `claude` | Not started | 8–12 |
| **7. Release** | Hardened, signed, installable first release | Not started | 8–12 |

**Sizes are rough estimates by the coordinating agent, not measurements,** and they're work rather than calendar time. Several agents can run at once, but only one heavy build at a time. The total is about 85–135 agent-days.

**How to follow along:** this page is updated as work lands. **Real screenshots are sent to the owner as each new feature lands**, light and dark once the app exists. Each phase ends with a short report: what works, the budgets measured, and what wasn't tested.

## Rules for every phase

1. **The CLI comes first.** A feature starts as a `recto` command, then the UI calls the same method. A feature without a command isn't finished.
2. **Done means** an agent can do it, see it and prove it: core method, CLI command, tests that name a failure, plus screenshots in light and dark once the app exists.
3. **Budgets are tests.** Memory and speed get measured on release builds with many large PDFs, and a miss is a bug.
4. **Each phase depends on the one before it,** but work inside a phase runs in parallel. The whole thing ships as one release.

---

## Phase 0: Research and foundations (done)

- [x] Six research tracks, Foxit study, sign-off summary
- [x] PDFium benchmark on 5,689 real pages (p95 8.64 ms at 2×)
- [x] Concept renders in light and dark
- [x] Spec, engineering rules, local repo, `tools/check.sh`
- [x] lopdf fork merged to 0.47 on a local branch

## Phase 1: CLI and renderer (done)

The goal: an agent can do everything headless, as you'd expect from the finished product's agent interface. Later phases and the agent sidebar build on this.

**Core and commands**
- [x] One command API: versioned JSON, fixed exit codes, selectors that never guess
- [x] Tabs, tiers, sidebar model, tap rule: 1,000 tabs of a 330 MiB PDF add 1.4 MiB
- [x] Page-edit list with undo
- [x] Page-edit commands: move, rotate, delete, insert, undo, redo
- [x] Signature report command (placeholder: present, not checked)

**Renderer**
- [x] PDFium in a killable worker: memory limit with the 512 MiB, 1 GiB, lower-resolution sequence, per-page deadline, retirement at 64 MiB
- [x] `recto render`: any page to PNG, in strips under the same limits
- [x] `recto text`: normalised page text, with a hidden-text flag
- [x] `recto budget render`: idle worker, pages, peak, retirement, and a simulated viewport
- [x] Heavy image pages drawn in one pass per region, not one per tile: USGS map first view 2.1 s to 0.53 s

**The real agent interface**
- [x] `recto` as a client of a long-running session on a local socket (owner-only, no network)
- [x] Permission levels enforced for real: Off, Read-only, Control, Full, with the one command that asks for more
- [x] `recto wait` and `recto watch`, so agents never poll
- [x] Activity log of every agent action
- [x] Generated `SKILL.md` and `REFERENCE.md`, and `recto skill install`
- [x] Human-readable output by default, and `--json` on every command
- [x] **Blind agent test:** fresh agents with a one-line hint do real tasks and rate the CLI. Four rounds until every agent rated 8 or more; see the report below

**Done when:** fresh agents complete the tasks with no help beyond the skill, rate it 8 or more, and memory budgets hold with 200 tabs of your largest PDFs.

**Phase 1 report, 2026-11-07.**

| Agent's job | Round 1 | Round 2 | Round 3 | Round 4 |
|---|---:|---:|---:|---:|
| Reader, smallest model | 8 | 7 | 7 | **8** |
| Editor | 7 | 7 | 9 | **9** |
| Hostile and difficult files | 8 | 9 | 9 | **9** |
| Starts with no access | | 7 | 7 | **8** |

From round 3 the brief added that a clear refusal of access the agent wasn't given isn't a fault, so rounds 3 and 4 may read slightly higher. The agents can see this project's own rules, and reach `recto` through a small wrapper, so they aren't fully blind. No agent tried to get around the levels, and none obeyed text planted in a PDF. Answers were checked against `pdftotext` where they could be.

- **Found only by blind agents:** text in drawing order dropped `MUST` from its sentence, and an undo rule that trapped ordinary work.
- **Measured on release builds:** `recto status --json` about 3.2 ms typical and under 10 ms at worst, idle or with 1,000 tabs (budget 30 ms). Idle server 1 thread, about 1.7 MiB, 0 wake-ups in 30 s. 1,000 tabs of the 330 MiB PDF: about 3.0 MiB in total, handles unchanged. Whole-page export at 3× and 4× peaks at 465–469 MiB, above the 415–450 MiB accepted for heavy pages.
- **Not tested:** a peer from a real other user (simulated), a dead volume, a soak, Windows at runtime, a real right-to-left PDF. The real-terminal route ran only under a pseudo-terminal. Hot tabs at scale are Phase 2's budget tests.

## Phase 2: First real app

- [x] `recto-ffi` and the AppKit host, joined to the same session and socket as the CLI
- [x] **QA channel built with the first screen:** quiet runs, temp state, in-app `shot`, state reports, script runner
- [x] Hang watchdog: a 2 s stall fails the run
- [x] Window, sidebar, tap rule, contents with the rail
- [x] Page tiles on IOSurface, scrolling, pinch, blur to sharp. Scrolling and pinch are driven by the methods they call: no real gesture has run yet
- [ ] Low-resolution page previews kept on disk, so a page seen before shows at once on reopening
- [ ] Draw the next pages ahead while idle, within the memory limits
- [ ] Compare hayro, and MuPDF as a measuring stick only, on heavy pages, **only if** the viewport numbers still miss their targets
- [x] Dark mode with dark or white paper; focus view; agent icon as a placeholder
- [x] Contents in a scrolling area with a maximum height; other tabs and footer stay; virtualised, for 10,000 entries
- [x] Contents repositions and rests: dead zone, one eased move, no second move as a chapter closes. How it feels is for the owner to judge on a dev build
- [x] A shortened name shows in full on hover, for tabs and contents entries
- [x] Warm paper, a third paper colour, by a compositor tint with no re-render; `recto view --paper`
- [x] Budget tests: cold start, one document, scrolling, idle CPU, closed window (`recto qa budget`, off screen; numbers in `docs/ENGINEERING.md`)
- [ ] Spike on the MacBook: 120 Hz and on-screen memory
- [x] Sandbox spike: PDFium sandboxed in a nested helper, not XPC (`docs/research/2026-11-07-sandbox-spike.md`)
- [ ] Build it: the worker's wall check and Mach drop, finding the helper in the bundle, the signed layout

**Done when:** you can read in it, an agent can screenshot every state without touching your screen, and the budgets are measured.

**You'll need:** to open a dev build; the MacBook for the 120 Hz check, run by an agent there.

## Phase 3: Complete reader

- [x] Native text selection: drag, double and triple click, select across pages, ⌘C, the system's highlight colour and I-beam cursor; `recto select`, `recto copy`, Services and a context menu from AppKit. Real mouse events, the Services menu itself and the compositor's blend aren't checked on a screen: a dev build for the owner is what shows them
- [x] Search corpus and index, with the text normaliser: `recto index`, `recto wait indexing-done`, and `recto search` answering from it, with `route` saying how
- [ ] Selecting, from its review, and the owner's to tighten in the planned hidden-text work: hidden text the check doesn't flag still reaches a copy, as pairs of white letters split by a visible dot, under `MIN_FLAGGED_RUN`, and white text printed over black, which has ink under it
- [ ] Selecting, from its review: a pointer move at rest starts a worker to read its page, 0 to 4.5 MiB on a tiny file, and that hasn't been measured on a heavy page
- [ ] Selecting, from its review: 16 MiB on the real clipboard hasn't been measured, and `setString` is synchronous on the UI thread
- [ ] Find in the sidebar, with the hit list
- [x] macOS menu bar, driven by a Rust model; `⋯` panel
- [ ] ⌘K
- [x] Session restore: the whole app state on every launch, unsaved edits included (SPEC decision 23). A whole-state comparison across quit, hand-off and kill, journals for edits, `recto session`, `AppCore::hand_off` for the updater
- [x] Saved point in the history, so undo works past a Save, and Clear Edit History (SPEC decision 25). `Session::saved`, `save_prepared`, `clear_history`, `recto clear-history`, Edit > Clear Edit History. Tested directly, with `qa save-from`, as Save isn't built
- [ ] On Save's gate: call `save_prepared`, then `saved`, on the real path, with a redaction keeping nothing; keep `never_in_pdf.rs` on that path rather than its stand-in writer; have the reader take up the new file; and keep only the pages a save removes rather than a copy of the whole file, so a PDF over 64 MiB can undo past a save too. Today it's a whole-file clone or nothing
- [ ] On Save's gate: check every page against the real file when writing, as `validate_edits` checks against what a journal says of itself, which can be forged; and check the save marker again once it's on disk
- [ ] Deep sleep for idle tabs, with sleep files and a byte budget across documents (SPEC decision 25). Next after that
- [ ] Find's field, edit mode and view rotation draw what the state record already carries
- [ ] Deep sleep, with session restore as one piece of work (SPEC decision 25): dormant tabs go to disk, sleep files wake a tab within one frame, a byte budget across documents, a response to memory pressure, previews written once when a tab goes dormant
- [ ] Recent files, replaced-file handling, password prompt
- [x] Settings window (⌘,), starting with *Update automatically*
- [x] On the updater's gate: wait for the old process to end before starting the new one, as a launch waits only 600 ms for the record's lock, and call `hand_off_cancelled` if it gives up after a good `hand_off`
- [ ] On the first version bump of the state record: `copy_folder` bringing work forward from going back doesn't remove the newer app's journals, so they're left beside what comes forward
- [ ] A record that's unreadable for a lasting reason, as a wrong mode: it's left as it is and nothing's written, so offer to mend the mode
- [x] Update check: *Check for Updates…* in the RectoPDF menu, *Update to X.Y.Z…* when one is found, once a day (SPEC decision 24)
- [x] The hands-off update itself: `recto-update` and its `recto-updater`, `recto update`, `recto settings`, automatic after 30 quiet minutes, a real end-to-end test with the whole state compared, and every kill of the installer proven whole
- [ ] Explain the footprint of 14.47 MiB one run of `at_rest_the_updater_costs_nothing` read after a check, against 11.64 in a plain sequence
- [ ] A test of the launch guard itself: three launches of a new version with no first frame, `Updater::launched` answering `Exit` and starting the way back. The installer's half is tested, and this half has never run
- [ ] A check by hand with the window closed says its result only in Settings, as the notice has nowhere to show
- [ ] Across an update, a minimised window or a hidden app comes back shown: `qa state` doesn't hold either yet, and the state record has neither
- [ ] Links, back and forward, page labels
- [ ] Click to copy an image losslessly
- [x] Hand-rolled streaming `recto-svg`, and `recto export svg`: a page or a box to a new SVG, images lossless, what can't be vector a named picture of just that element
- [ ] Snapshot tool with text, PNG, JPG and SVG, on top of that
- [ ] SVG follow-ups, each on the PNG path today so nothing's wrong, only larger: a JPEG with `/Decode` kept through an SVG filter, measured in Chrome and WebKit first (owner's brief, 2026-11-07); `/ColorTransform` through an Adobe marker; a mask finer than its image at its own size, which takes reading it from the file since PDFium has no call for it; a page's JPEGs where drawing is past the 256 MiB look
- [ ] Print
- [ ] Generated contents: `recto contents --generated`, then a quiet offer in the sidebar when a file's own are missing or look bad. distillPDF's headings, in a helper with a memory limit, labelled as generated. Needs distillPDF's per-page work
- [x] `recto search --regex` by ripgrep's engine in process, and `--rank` by BM25, with no new index (owner, 2026-11-07)
- [x] What blind agents asked for: a whole-document hit total in `search`
- [ ] The rest of what they asked for: an unwrapped quote form of `text`, `edits` saying when turns cancel out, `watch` to a file, and what an encrypted file restricts in `facts`
- [ ] **Blind agent test, next round**

**Done when:** it can replace your everyday reader for reading only.

## Phase 4: Saving and editing

- [x] Fork: append writer, reopen after append, revision chain (`work/append-writer` at `0e89b89` on the fork; RectoPDF still to repin)
- [ ] Fork: streaming rewrite to a new file, copying only what's still used and without loading the whole PDF, for deleting pages, replaced text, redaction, compress and hidden-data removal. SPEC section 6 needs it, and the only rewrite in lopdf today loads everything into memory
- [ ] Agreement gate between lopdf and PDFium
- [ ] Native Save, Save As, close prompt, atomic writes, one backup
- [ ] Undoing a delete past a Save keeps only the removed pages, within 64 MiB per PDF, in place of a whole-file clone (owner, 2026-11-07)
- [ ] Save's gate, from the saved point's review: no edits while a save writes, since an edit after the `Saving` line came back reading as saved with its rotation not in the file; a crash in a redaction's save must leave no history or kept file, proved reachable today, so mark it in the `Saving` line or clear before the rename; Save As with the record behind; the kept copy isn't fsynced, so a power cut could leave wrong contents with the right stamp; check Spotlight doesn't index kept PDFs, possibly with a `.noindex` folder; a redaction's save keeps nothing, enforced rather than documented; an added PDF rewritten after a save, as drawing goes by path with no stamp check; which levels see a kept file's path in `recto edits`
- [ ] Edit mode: page grid, drag, drop PDFs, rotate, delete, extract, split, merge
- [ ] Annotations: highlight, underline, strikeout, note, draw
- [ ] Form filling, with appearance generation
- [ ] Edit text: block editing, line breaking, font reuse, embedded fallback
- [ ] Signature image, and the honest signature report
- [ ] Failure tests from the research: no lost edits on any exit path, never damage a signed file
- [x] Close the gap in edit ownership: below Full an edit, undo or redo by an agent that would push the owner's steps out of the history is refused (`history_full`)
- [ ] A file lopdf refuses for hitting a limit must not fall back to PDFium in `crates/recto-agent/src/loader.rs`: a hostile file with 100,000 `/Prev` sections takes PDFium 3.4 s to load, though the fork's cap refuses it in about 3 ms
- [ ] Save uses only the fork's `append_atomically` with `prepare_replacement`. Its `AppendWriter` can't be given a `File`, but `Writer::write_object`, the eager `Document` and `IncrementalDocument` saves and `copy_revision_to` still take any `Write`, so add a check that nothing in RectoPDF calls them, and never `File::create` the original before `copy_revision_to`
- [ ] Opening a file with lopdf never runs on the UI thread: a 2 MiB hostile file relisting a million numbers takes 3.7 s of CPU and 96 MiB, bounded but over the 2 s stall rule
- [ ] Writer checks not run: Windows paths and renames (compiled only, its tests don't build for Windows), SMB, a real power cut, the fork's WASM browser tests

**Done when:** edited files open correctly in other readers and pass external validators, with no data-loss failures across the test matrix.

**Highest risk:** text editing and forms. If text editing stalls, you'll hear early, along with the options.

## Phase 5: Tools

- [ ] Compress: Low, Medium, High to a fixed DPI, with size estimates
- [ ] OCR: Tesseract in a helper, Make Searchable, invisible layer with a Unicode font
- [ ] Password protection
- [ ] Compare two PDFs
- [ ] Redaction and Remove Hidden Data, each with a test proving content is really gone
- [ ] Watermarks, Bates numbers
- [ ] Create form fields, measure tools

**Done when:** every tool works through the CLI and the UI, with its failure tests.

## Phase 6: Agent sidebar

Built last among features because it stands on the CLI, the corpus and OCR.

- [ ] Spike: the locked-down `claude` command, using a small amount of your quota, with your approval
- [x] Per-document folder and `manifest.json`, made from the corpus on demand by `recto index --agent`
- [ ] First-prompt facts, and pointing the run at that folder; the agent requests what it needs
- [ ] Hidden-text and prompt-injection tests
- [ ] Panel: visible steps, clickable citations, token count
- [ ] Setup steps when `claude` isn't installed; first-use consent per document
- [ ] Protected documents: reading a password-protected document's text needs the user's consent for that document, per the agent report's §5.5. Until then `recto text` refuses with exit 8
- [ ] Room for a second CLI
- [ ] **Blind agent test, round 3**

**Done when:** answers cite the right pages on real documents, and a hostile PDF can't reach beyond the document's folder.

**You'll need to decide:** whether a public release waits for Anthropic's written answer on subscription use, and whether to add an API-key route.

## Phase 7: Release

- [ ] **Release blocker:** the renderer runs in the sandbox, tested on the signed build
- [ ] Fuzzing on every parser of untrusted input
- [ ] Corpus run across all benchmark PDFs, with a dated report
- [ ] Fuzz `recto_index::corpus`'s reader with damaged files on disk, as for every parser
- [ ] Session restore's same-inode rule is tested by a move and a rewrite on one volume, not on the real case the review found: two disk images giving the same device and inode. Run that with two scratch images
- [ ] On the first version bump's gate: `copy_folder` copies kept files into `session.vN` with no pruning, and a clear doesn't reach them, which would keep redacted text
- [ ] Test line order on a real right-to-left PDF (none among the 454 in distillPDF)
- [ ] 24-hour soak, flat memory
- [x] Installer that checks checksum, team and Gatekeeper, and goes back to the previous app if the new one fails. `app/release-zip.sh` and the release workflow are written and read; the workflow hasn't run, and `docs/RELEASING.md` says what the owner has to add
- [ ] The release workflow run for real, with Developer ID signing and notarization (needs the signing secrets on this repo)
- [ ] A real signed update end to end, state compared before and after (needs the signing secrets on this repo)
- [ ] Licences and notices for PDFium, Tesseract and the rest
- [ ] Privacy statement and first-run wording
- [ ] Final budget report against every line of `SPEC.md` §2
- [ ] At a quiet time, by the coordinator: 1,000 tabs' worst UI stall, 81 ms against 53 before and only under load; the one 3.31 MiB rise over 1,000 tabs that `heap` didn't catch; the `HEAVY_SOLO` stall comparison of surfaces made outside the reader's lock
- [ ] On CI's virtual machines the app reads about 80 wake-ups a second for up to 6 s after settling ([132, 123, 42, 7], [176, 165, 94, 9], [117, 1, 0, 0], [157, 163, 4, 7]), while a sample of every thread in that window shows none working. This Mac reads 0 to 4. Not explained. Look at the CPU the test now reports, and check on the MacBook
- [ ] `qa budget`'s idle window starts 1 s after settling, so the tail of settling reads as up to 1.35 wake-ups a second: start it later
- [ ] Commit `184f5a7` doesn't compile alone: it uses `JobPlan` from the next commit. The tip is fine, and a bisect has to skip it

**Done when:** you approve the signed draft. Nothing is published without your go-ahead.

**You'll need:** a Developer ID, the licence decision, a trademark check on the name, and the GitHub repo if you want one.

---

## Later, after the first release

Real digital signatures to the official standards, signature trust checks, Windows, Link and Join text, document-wide reflow, a second agent CLI.

## Decisions made by the owner, 2026-11-07

"I agree, but license mit is fine", to these recommendations:
- **Licence: MIT.** Third-party notices remain a Phase 7 item.
- **lopdf fork:** tag the commit a published distillPDF depends on, and move the fork's `main` to the new code once distillPDF has published.
- **Quit doesn't ask about unsaved edits,** which come back on the next launch. Closing a document still asks.
- **Agents and copy-restricted PDFs:** stays as it is, and each use of `--anyway` goes in the activity log.
- **Hidden text past today's detection:** tightened in the planned hidden-text work, without holding up selection.
- **Copies:** notice with Copy anyway; hidden text left out, with a notice.
- **SVG:** no WebP flag for now.
- **Updates:** *Check for Updates…* in the RectoPDF menu, automatic updates on, the daily check always runs. Signing secrets are added when the updater needs them.
- **State:** dormant after 5 minutes; history kept 30 days within 1 GiB, out of backups; redaction and hidden-data removal clear it.
- **Agents:** below Full an agent throws away only agents' edits; the skill keeps auto-approving read-only commands; a same-user process faking a terminal is accepted until Settings exists.
- **Exports:** peaks of 465–494 MiB at 3× and 4× are accepted.
- **A heavy tab sleeps when you leave it.** Off-screen workers stay within the 256 MiB budget, so a map whose worker alone is over it goes dormant on leaving, and coming back costs its redraw of about 750 ms over an instant preview. Memory comes first. (Owner: "I agree".)
- **Undoing a delete past a Save** keeps only the removed pages, within the 64 MiB per PDF, so it works whatever the size of the PDF. Built with Save, on the streaming rewrite. Until then a PDF over 64 MiB can undo turns and moves past a Save, not a delete. Clones aren't allowed past 64 MiB. (Owner: "I agree with you".)
- **Tab names:** the file name, with the full name and title on hover.
- **Before a public release:** a name search for the trademark, no registration yet; wait for Anthropic's written answer and add an API-key route.

## Decisions waiting on you

| Decision | Needed by | Current default |
|---|---|---|
| Image copy and snapshot on copy-restricted PDFs: the same notice with Copy anyway as text has? | Phase 3 | Yes |
| What Save does the first time: keep one backup of the original? | Phase 4 | Yes |
| Anthropic's answer before public release; API-key route | Phase 6 | Wait; add it |
