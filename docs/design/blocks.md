# 8.3 Blocks, and asking for more: design report (2026-11-27) (Opus, 2026-11-27, read-only, nothing built)

**Where things stand.** 8.2's slices are merged: refs on every hit, `ref`, `resolve`, `--ref` on 10 commands, and page identities stored in the corpus. The 8.2 box isn't ticked, so 8.3 starts once it is. distillPDF is a git dependency of `crates/recto-headings` only, pinned at 0.1.7 `fbf5b22`. That crate is a workspace of its own, so rayon stays out of the app. Its checkout is `/Volumes/EksternalHome/Koding/Rust/distillPDF`, which has an `inbox/unread/` like lopdf's. The helper `recto-headings` keeps headings only. It is **not sandboxed**: it opens the file by path (SPEC decision 3, and the owner's rule of 2026-11-08 "Turn its image decoding off"). `PageOutput.blocks` already gives kind, level, a box and reading order a page at a time (`distillpdf/src/pages.rs:498`).

**1. What a block is, and where it comes from.**
- **What a block is.** One of distillPDF's para, heading, list_item, table, figure, caption, footnote or code blocks, in its reading order. It is stored only after being mapped onto `recto text`'s characters of that page: its spelled letters must be one unbroken run in the page's text, found after the previous block's end. That is the research's cheap check (§6): 91–100% of blocks pass on prose, 20–64% on interleaved pages.
- **What a stored block holds.** Kind, level, page, `[start,end)` in characters, its box in whole points, and a "continues onto the next page" bit. **No text.** Text always comes from `recto text`, with hidden spans flagged and running heads marked, so distillPDF's own text never reaches an agent. That matters because it includes hidden text: the World Bank "For Official Use Only" case.
- **Headings above** come from the tab's contents, not from distillPDF's heading stack, which misfiles legal articles. Use the file's own outline where its verdict is fine, otherwise the generated contents if they have been made. The entries whose (page, place) come before the block form the stack; a section ends at the next entry of the same or a shallower depth; a chapter is a top-level entry.
- **Where it comes from.** `recto-headings` gets a job kind `blocks`, in its own module `src/blocks.rs`. It writes nothing and sends one JSON line per page, `{"page":N,"blocks":[{"kind","level","box":[4],"text","continues"}]}` (`text` is plain text: only its letters are used, and the session never shows it), so nothing piles up. `page` is zero-based; a line over 512 KiB, a page out of range or one that doesn't come after the last is refused. The session maps each line against the corpus text and drops a block that fails.
- **Caps.** At most 512 blocks a page, 64 Ki letters a block and 512 KiB a line. Comparisons are capped as `seen_at`'s are. The blocks job needs no render worker, because mapping against the stored text already covers the ink question.
- **Recommendation: sandbox the blocks job** in a helper app with the renderer's walls (`recto_platform::wall`) before it ships, with the file passed as a descriptor. Why: blocks would parse every document an agent widens a hit in, not just files someone clicked Generate contents on. distillPDF opens by path today, and `from_bytes` copies the whole file (330 MiB for the USGS map). So either `/dev/fd/N` is tried under the sandbox, or one inbox ask goes to distillPDF for an open from a `File` without a copy. This is owner question A.

**2. Where blocks are kept, and when they're made.**
- **Kept** in the tab's corpus folder, sealed as the identities are: `app/blocks.bin` (one sealed record per page) and `app/blocks.sealed`. They are keyed by `CorpusKey`, counted in `max_doc_bytes`, and the 2 GiB, 90-day sweep, `index clear` and a new file version all take them away. Nothing is in memory per tab (a cold tab stays ≤ 4 KiB, 0 threads). A `get` reads only the records of the pages it answers, and holds nothing after.
- **Never carried over.** Old blocks are never mapped onto a changed file: a new version has a new key, so its blocks are simply "not made".
- **Made** when someone first asks for a grain that needs blocks, never on open (decision 22). They go through **one distillPDF queue** shared with generated contents: at most one helper, newest ask first, the thread only while a job runs, utility priority, 512 MiB and a time limit, watched from outside.
- **Nothing waits (rule 3).** `get` never waits for the build. It answers at once with the best grain it can (`widened: {to: "page", why: "blocks_building"}`), and `recto wait blocks-ready` exists for an agent that wants to wait. No action by the person is involved.
- **Without the app** there is no key, so no corpus and no blocks. The answer widens to the page with `no_find_key`, as search does.

**3. `recto get <ref> --grain hit|paragraph|section|page|chapter --as text|markdown`.**
- **Level and route.** Read-only, Route::Session, reading pages under the server's draw cap (`draws`/`reads_pages` true). It resolves through the one `locating::resolved`. Only `found` and `moved` give an answer; every other status is `reference_unresolved` in `text --ref`'s words.
- **Rules carried over unchanged.** A protected document is refused (exit 8) before anything is resolved. Copy rules are `text`'s. There is no path: `untrusted.link` is the name with `#page=N`, as on hits. Titles and text sit under `untrusted`. The policy walk shows an agent below Full gains nothing.
- **The ref is the anchor; the grain is the question.** No new token part: the research's `.b7-…` is dropped. Why: block edges change with distillPDF's version, while character refs are anchored to `recto text`, and the existing matcher and character check already cover a changed file. So a block's reference is the character ref of its range, and a section is "the entry whose range holds the anchor", worked out on the version found.
- **Text of record.** When the file's `CorpusKey` still matches, `get` reads the corpus text and checks the ref's characters against it, with the key and stamp checked again at the end, as `HitRefs::finish` does. Otherwise it reads live.
- **Caps.**
  - `--max-chars`: default 16,000, at most 65,536.
  - At most 64 pages read from the corpus per answer, and live reads within 5 s.
  - A section is cut at a block boundary, otherwise at a line.
- **Markdown is decided in Rust.**
  - A heading's `#` count comes from its contents depth. Lists and code are fenced longer than any run of backticks inside them.
  - Markdown characters at the start of a line in document text are escaped, so a file can't forge a heading or a fence.
  - Hidden text is marked `[hidden: …]` as `text` marks it.
  - Running heads are left out and counted.
  - Tables come out as plain lines (owner question D).
- **Answer.** `{tab, grain, as, ref:{status,…}, unit:{kind, pages, chars}, widened, headings_above:[{depth, source, untrusted:{title}}], refs:{self, next, prev, up, more}, truncated, counts, untrusted:{text|markdown, link}}`.

**4. Asking for more.** The plan's `--next`, `--prev` and `--up` flags become refs carried in every answer. The agent calls `get <refs.next> --grain paragraph`; going up is the same ref at a coarser grain. A long section ends with `refs.more`, a character ref at the cut, used as `get <more> --grain section --from-here`. That reads the same section starting at the ref. It is stateless: no cursor, token store or session field (rule 1). It is bounded by the caps above, and a more-ref after the file changed resolves or refuses like any ref. Recommendation: this design, because it is one implementation and nothing is kept.

**5. The window in 8.3: nothing.** It gets nothing until 8.4's opening half or the sidebar. So there are no screenshots: say "no UI" in the hand-back, as `design/locator-core.md` did.

**6. Slices, in order.** Each is a branch for a Sonnet worker.
- **S0, measurements first** (`recto-headings/examples/blocks_sample.rs` and a report). The numbers in §7. Decides Full against Typography scan and the caps.
- **S1, the pure core** (`recto-index/src/blocks/`: the record, the sealed codec, the mapping, section ranges from contents entries, markdown and its escaping). Unit tests at the limit plus one, and fuzz targets `blocks` and `distill_line`. No I/O.
- **S2, `get` for hit and page** (`recto-agent/src/getting.rs`, `commands/getting.rs`; one-line registrations only). Answer shape, refs next/prev/up, caps, widening, the levels walk, the protected refusal, goldens and `requests.txt`. Can run alongside S3.
- **S3, the helper's `blocks` job** (`recto-headings/src/blocks.rs`). Streams per page. Tested with `recto_testpdf` documents (two columns, a list, a table, a hidden paragraph, `HIDDEN_HEADINGS`' 15 techniques, `form_bomb`): no panic, a bounded peak.
- **S4, the session's build** (`recto-control/src/blocking.rs` on the generator's one queue; the corpus aux files). Tests: killed partway, the file changed, eviction, `index clear`, `wait blocks-ready`.
- **S5, paragraph, section and chapter, `--from-here` and markdown.** Headless tests on a made outline document and a generated-contents document, `text_changed` after a rewrite, and a forged heading or fence escaped.
- **S6, the sandboxed distill helper** (if A is yes), proven as `recto-render-service/tests/sandbox.rs` proves the renderer.
- **Later (8.3b), PDF, PNG and SVG** delegating to `snapshot --ref --rect` and `export svg --ref` at Full. A PDF of pages needs a new page-extract writer path, which doesn't exist yet.

**7. Risks, and what to measure first** (release builds, with the load stated).
- **distillPDF per file:** scan plus stream, Full against Typography, the helper's peak with `MallocLargeCache=0`, wall time at utility, and CPU. On: NASA `gov_nasa_ntrs20240002628` (383 pages, 44–49 s on the page path), `ntrs20220005969` (picture-heavy), USGS `gov_usgs_usgs70277338` (330 MiB), the remade 10,000-page PDF (against the 30-minute limit), `rfc9110`, `un_sdg2024` (worst quality), the GDPR and the AI Act, `math_AG_2607.28411`, `forms_irs_f990`, `form_bomb`.
- **Mapping pass rate** by kind and file over bench100. This sets what "paragraph" honestly promises.
- **Disk size** per block and per page, against `corpus_bytes`. An estimate is about 0.4–0.5 KiB a page, so about 190 KiB for NASA and about 5 MB for 10,000 pages.
- **`get` timing:** p50 and p95 for a paragraph and a 30-page section on NASA.
- **Sandbox:** whether distillPDF opens through `/dev/fd` under the App Sandbox, and what `from_bytes` copies.
- **Risks:** Typography's quality hasn't been measured; a scan costs the whole document even for one page; distillPDF's silent text caps.

**8. Questions for the owner, in plain words.**
- **A.** Should the program that reads document structure run walled off, as the page drawer does, before agents use it widely? *Recommend yes* (about 2–3 days, plus one request to distillPDF).
- **B.** Make blocks only when an agent first asks for a paragraph or section, or after every search index? *Recommend on first ask.* Indexing every searched document costs minutes of processor time on big books.
- **C.** "Paragraph" is promised only where it checks against the page's own text; otherwise the answer is the page, and it says so. *Recommend yes* (about 9 in 10 on prose; magazines and law fall back).
- **D.** Tables come out as plain lines for now. *Recommend yes*; proper tables once each cell can be checked.
- **E.** Text and Markdown in 8.3; PDF, PNG and SVG pieces later, through the existing commands at Full. *Recommend yes.*
- **F.** A default answer size of about 16,000 characters (about 4,000 tokens), with "more" refs to go on. *Recommend yes.*


## The owner's answers (2026-11-27)

A: yes, wall the helper off first. B: blocks made when an agent first asks. C: a paragraph only where it checks against the page's text, else the page, said. F: about 16,000 characters an answer. D and E settled on the recommendation (tables as plain lines; text and Markdown now, PDF/PNG/SVG later). See `docs/OWNER-RULES.md`.

## After S0's measurements (2026-11-27, coordinator, from `docs/reports/blocks-s0-2026-11-27.md`)

- **Typography scan is the default** (maps as well as Full or better, 2-10x faster to scan).
- **The mapping rule (settled for S1):** a block's letters are looked for first after the previous mapped block's end; if not found there, the block is accepted at its one occurrence anywhere on the page that overlaps no block already mapped; if the letters occur more than once in the unmapped text, or not at all, the block isn't stored. Reading order stays distillPDF's. Why: strict "after the previous" costs most on multi-column pages (NASA 42% strict vs 73% anywhere), and "exactly one unmapped occurrence" keeps a repeated running head or a short phrase from mapping to the wrong place.
- **Tables and figures** have no text to map, so they aren't stored in 8.3 (a page that is a table answers as the page); footnotes and map pages mostly fail and fall back to the page, as the owner chose.
- **A page whose text the renderer refuses** (`MemoryLimit`, as `form_bomb`) maps nothing: it's a page with no blocks, never a failure of the job.
- **Record size** measured at about 15 bytes a mapped block plus 8 a page (76-259 bytes a page), well under the estimate.

## For S6b, from S6a's security review (2026-11-27)

- Spawn only `structure_helper(canonicalize(current_exe).parent())`; remove `Contents/MacOS/recto-headings` from build.sh and the bare fallback in `helper_exe`, and add it to `--check`'s stray list. The structure helper is required in `--check <app>`.
- `in_structure_helper` gains `RELEASE_BUILD ⇒ walled`, as the renderer's rule has.
- Don't trust the hello: read it, then check from outside before writing the job — `sandbox_check(pid, NULL) == 1` and the document path not readable by name (`pid_may_read == 1`, denied); a release build refuses `walled:false` or a failed check. The child is unreaped, so its pid can't be reused. (The Mach drop can't be seen from outside; it rests on the helper's own exit 79.)
- Keep the helper's environment explicit; never pass `APP_SANDBOX_CONTAINER_ID` (`sandbox_in_force` trusts it).
- A fresh read-only `File` per spawn (the fd shares its offset with the host's open file).
- The headings job by descriptor stays off paths (fonts, CMaps); its render-worker ink check moves to the session (the walled helper can't spawn).
- Sign the app after both helpers.
