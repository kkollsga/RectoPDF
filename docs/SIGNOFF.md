# RectoPDF: research summary and sign-off

2026-11-06. Follows §1.4 of `rust_app_development.md`. No app code exists. Detail, sources and the 58 [DECIDE] items are in
`docs/research/`. Item names below are the ones those reports use. Concepts are in `docs/concepts/`.

**Product:** a high-performance, low-memory PDF reader in Rust, with tabs in the sidebar, certificate signing, and an agent
that searches a document instead of receiving all of it. CLI: `recto`.

## What the research supports

| Area | Recommendation | Evidence | Status |
|---|---|---|---|
| App | AppKit host, Rust core over UniFFI; no SwiftUI in the main window | Sidebar window 17.6 MiB vs 27.9 SwiftUI; a Metal swapchain adds 105–184 MiB; blank webview 56.7 MiB | Measured off-screen only |
| Page drawing | PDFium without V8 or XFA, in a sandboxed XPC worker, on IOSurface tiles | XPC floor 2.2 MiB, 0.12 ms per tile, restart 15–17 ms; purgeable tiles fell from 271 to 15 MiB | **PDFium's own memory and speed unmeasured** |
| lopdf fork | Owns structure and is the only writer | Opened a 330 MiB file in 1.2 ms using 2.3 MiB (eager: 179 ms, 655 MiB) | Measured warm-cache. It draws no pixels and needs an append writer |
| Agent search | A `claude` run limited to `Read` and `Grep` over a per-document folder of page text | `rg` 7–12 ms per document; about 60–1,400 tokens per search vs about 100,000 for Folio's dump | Search measured; the full `claude` command has never been run |
| distillPDF | **Parked by you, 2026-11-06.** Search text and highlight positions come from PDFium for now | Earlier audit: whole-document calls peak at 200 MiB–1 GiB and can't be cancelled | PDFium text quality and its hidden-text handling are untested |
| Signing | v1 creates PAdES B-B and B-T and reports facts, never "valid" | Fork signed 243 of 244 real PDFs with original bytes intact | PDF mechanics only; no real key store or PAdES conformance test |
| Tabs | One more idle tab ≤ 4 KiB, with no file handle | No competitor source we read frees background tabs | Target. Whether memory actually returns is untested |

**Decided by you, 2026-11-06:** the agent requests page information when it needs it, and the app doesn't send document text up front. "That hands off the decisions to the agent." There is no inline shortcut for short documents (replaces agent D5's recommendation).
The first prompt carries facts about the PDF (size, pages, text status, outline) so the agent can choose between the full text and a subset. Page text is never included.

**Decided by you, 2026-11-06:** "We build everything in one go, but we test as we build." There are no phased releases; build order follows dependencies only.

**Agreed scope:** reader, sidebar tabs, Find, dark and white paper, focus view, agent panel and `recto` CLI, annotations, form filling, a signature placeholder with a labelled image option, page tools, password protection, compare, compress (Low, Medium, High to a fixed image DPI), Tesseract OCR with Save as searchable PDF (modelled on distillPDF's solution), click to copy an image losslessly, and a snapshot tool (text, PNG, SVG, JPG).

**Also in, by your answer:** redaction, remove hidden data, watermarks and Bates numbers, create form fields, measure tools.

**Changed by your answer:** digital signatures are a structured placeholder for now, to be built later to the official standards. `SPEC.md` is now the current record.

**Left out:** full text and image editing, conversion to Word or Excel, creating PDFs from Office files, sending documents for signature, print production, portfolios, and 3D and media.


**Other budgets:** cold start ≤ 300 ms, one document ≤ 60 MiB, ≤ 1 wake-up per second, closed window ≤ 20 MiB.

**Distribution:** Developer ID and notarization, not the Mac App Store. A sandboxed child process can't see `rg` or `claude`.

## Where the reports disagreed

| Conflict | My call |
|---|---|
| Rendering: v1 only reports signatures. Signing: v1 also creates them. Prior art: stamp first | Create them, with a labelled image stamp as a separate option. You asked for signing |
| Platform: the app never starts `claude` in v1. Agent: an in-app panel | **Settled by you, 2026-11-06:** the app starts the existing install |
| Idle tab: 64 KB (prior art) vs 4 KiB (platform) | 4 KiB, because platform measured it. Revisit if the MVP misses it |
| Prior art: nothing floats over the page, system accent. First concepts: floating controls, custom accent | **Settled in part by you, 2026-11-06:** no top bar, nothing over the page, commands in the sidebar. Accent still open, question 4 |
| Highlights: agent report maps hits through block boxes. Engine: those boxes are 59–132× a word's area and need a whole-document run | Positions on demand for the visible page, now from PDFium. The agent report's text cleanup for search stays |

Your preference for ripgrep holds with one change: use Claude Code's `Grep` tool, which is ripgrep, rather than `Bash(rg ...)`.
A test showed `rg --pre` executing a corpus file's text as a script, and that text comes from the PDF.

## Decisions that block the first milestone

| # | Question | Recommendation | Cost to change later |
|---|---|---|---|
| 1 | Platforms? (platform D1) | Mac first, Windows parity later, core cross-checked for Windows from day one | Low |
| 2 | Closed or permissive, or AGPL? (RD2) | Permissive or closed, which rules out MuPDF and poppler | AGPL later is cheap; leaving AGPL is very expensive |
| 3 | PDFium, and may we run its spike? (RD1, RD9) | PDFium. Run the required spike parts: about one agent-day and 5 MB of downloads, including running prebuilt binaries | Low to moderate |
| 4 | Design: custom accent or system accent, and which sidebar material? (prior art D8, platform D8) | Custom accent, native sidebar. Judge a Liquid Glass version before settling. **Decided: no top bar, maximum vertical reading space; a single agent icon at the top right opens the agent panel; dark mode offers dark or white paper** | High |
| 5 | Before public release, does the app's use of a subscription need a written answer from Anthropic? (agent D2) | **Decided: the app uses the existing `claude` install if there is one, as Folio does (owner, 2026-11-06).** Still open: wait for that answer before releasing publicly, and whether to add an API-key route. I recommend waiting, and adding the route | Low in code; user expectations if removed |
| 6 | Is "context cli" Codex CLI? (agent D1) | Claude only for v1, with room for a second CLI | Low |
| 7 | Signing scope? (S1, S2, S5) | Create and report; neutral mark, no green tick; no BankID button, but display BankID files correctly | Very high if a wrong "valid" ever ships |
| 8 | Who owns forms and saving? (RD3) | Rust core plus lopdf. It's the largest item in the rendering report | High |

The other 44 or so items have recommendations, and I'll follow them unless you object. Most are cheap to change. The exceptions are
what Save does (prior art D2) and what `recto --page N` means (prior art D7), so I'll raise those again before building them.

## Risks

1. **Terms:** Anthropic's docs support both readings for an app starting the user's own `claude -p` on a subscription. Folio does this, but that doesn't show it's allowed. `--bare` is also due to become the default for `-p`, and it never reads subscription sign-in.
2. **Renderer memory:** the main goal rests on a number nobody has measured.
3. **Forms:** the appearance generator and append writer are real work. 11 of the 141 benchmark files are hybrid forms, and how Acrobat treats them after saving is unknown.
4. **Fork:** merging upstream 0.47 touches the revision distillPDF pins.
5. **Competition:** SumatraPDF's development branch is adding a macOS port, an AI sidebar, forms and signatures.
6. **Security duty:** PDFium needs an update path that can ship within days.

## Not proven

- Anything on a real display: 120 Hz scrolling (needs the MacBook), Liquid Glass memory, pinch.
- All of Windows.
- The `claude` command, including whether its lockdown holds. Testing it spends your quota, so it needs your approval.
- Acrobat's view of our signed files, and cold-disk timing.
- Competitor numbers, since nothing was installed (prior art D10).
- Whether PDFium's text includes hidden text, which matters for prompt injection. distillPDF was never checked either.

## What only you can provide

- Your Developer ID, if Seedwell's can be reused.
- A few real signed PDFs of your own, plus one YubiKey if you accept it (S10).
- Permission to contact Anthropic, or sending the question yourself. The wording is in agent D2.

## Proposed first milestone (after a go)

1. Run the PDFium spike and the on-screen scroll spike.
2. Create the workspace per guide §14, with `AGENTS.md`, `SPEC.md` and `check.sh`.
3. Build the QA channel and `recto` CLI with the first screen.
4. Open a PDF in the sandboxed worker, scroll it on tiles, and show sidebar tabs with the collapsing contents, in light and dark.

**Accepted when:** the budgets above are measured in tests, an agent can open and screenshot every state through tool calls, and a
hang fails the run. Saving, forms, signing, the agent panel and the other tools follow in the same build, each tested as it lands.
