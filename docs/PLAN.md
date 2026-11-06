# RectoPDF: plan to first release

Last updated 2026-11-06. Behaviour is in `SPEC.md`, and the reasons behind it are in `docs/research/`.

## Where we are

| Phase | What you get | Status | Size (agent-days) |
|---|---|---|---:|
| **0. Research and foundations** | Spec, concepts, benchmarks, first core | **Done** | |
| **1. CLI and renderer** | An agent can open, read and draw any PDF, and test everything without a window | **In progress** | 5–8 |
| **2. First real app** | A native window you can read in, with real screenshots | Not started | 8–12 |
| **3. Complete reader** | Find, menus, ⌘K, session restore, image copy, snapshot and SVG | Not started | 10–15 |
| **4. Saving and editing** | Native Save, edit mode, annotations, forms, signature image | Not started | 25–40 |
| **5. Tools** | Compress, OCR, passwords, compare, redaction, watermarks and more | Not started | 20–35 |
| **6. Agent sidebar** | Ask a question about a PDF, with your own `claude` | Not started | 8–12 |
| **7. Release** | Hardened, signed, installable first release | Not started | 8–12 |

**Sizes are rough estimates by the coordinating agent, not measurements,** and they're work rather than calendar time. Several agents can run at once, but only one heavy build at a time. The total is about 85–135 agent-days.

**How to follow along:** this page is updated as work lands. Each phase ends with a short report to you: what works, the budgets measured, renders in light and dark, and what wasn't tested.

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

## Phase 1: CLI and renderer (in progress)

The goal: an agent can do everything headless, as you'd expect from the finished product's agent interface. Later phases and the agent sidebar build on this.

**Core and commands**
- [x] One command API: versioned JSON, fixed exit codes, selectors that never guess
- [x] Tabs, tiers, sidebar model, tap rule: 1,000 tabs of a 330 MiB PDF add 1.4 MiB
- [x] Page-edit list with undo
- [ ] Page-edit commands: move, rotate, delete, insert, undo, redo
- [ ] Signature report command (placeholder: present, not checked)

**Renderer**
- [ ] PDFium in a killable worker: memory limit, per-page deadline, retirement at 64 MiB *(worker running)*
- [ ] `recto render`: any page to PNG
- [ ] `recto text`: normalised page text, with a hidden-text flag

**The real agent interface**
- [ ] `recto` as a client of a long-running session on a local socket (owner-only, no network)
- [ ] Permission levels enforced for real: Off, Read-only, Control, Full, with the one command that asks for more
- [ ] `recto wait` and `recto watch`, so agents never poll
- [ ] Activity log of every agent action
- [ ] Generated `SKILL.md` and `REFERENCE.md`, and `recto skill install`
- [ ] Human-readable output by default, and `--json` on every command
- [ ] **Blind agent test, round 1:** fresh agents with a one-line hint do real tasks and rate the CLI

**Done when:** fresh agents complete the tasks with no help beyond the skill, rate it 8 or more, and memory budgets hold with 200 tabs of your largest PDFs.

## Phase 2: First real app

- [ ] `recto-ffi` and the AppKit host, joined to the same session and socket as the CLI
- [ ] **QA channel built with the first screen:** quiet runs, temp state, in-app `shot`, state reports, script runner
- [ ] Hang watchdog: a 2 s stall fails the run
- [ ] Window, sidebar, tap rule, contents with the rail
- [ ] Page tiles on IOSurface, scrolling, pinch, blur to sharp
- [ ] Dark mode with dark or white paper; focus view; agent icon as a placeholder
- [ ] Budget tests: cold start, one document, scrolling, idle CPU, closed window
- [ ] Spike on the MacBook: 120 Hz and on-screen memory
- [ ] Sandbox spike: PDFium in an XPC service (its library links AppKit)

**Done when:** you can read in it, an agent can screenshot every state without touching your screen, and the budgets are measured.

**You'll need:** to open a dev build; the MacBook for the 120 Hz check, run by an agent there.

## Phase 3: Complete reader

- [ ] Search corpus and index, with the text normaliser
- [ ] Find in the sidebar, with the hit list
- [ ] macOS menu bar, driven by a Rust model; `⋯` panel; ⌘K
- [ ] Session restore, recent files, replaced-file handling, password prompt
- [ ] Links, back and forward, page labels
- [ ] Click to copy an image losslessly
- [ ] Snapshot tool with text, PNG, JPG and the hand-rolled streaming `recto-svg`
- [ ] Print
- [ ] **Blind agent test, round 2**

**Done when:** it can replace your everyday reader for reading only.

## Phase 4: Saving and editing

- [ ] Fork: append writer, reopen after append, revision chain
- [ ] Agreement gate between lopdf and PDFium
- [ ] Native Save, Save As, close prompt, atomic writes, one backup
- [ ] Edit mode: page grid, drag, drop PDFs, rotate, delete, extract, split, merge
- [ ] Annotations: highlight, underline, strikeout, note, draw
- [ ] Form filling, with appearance generation
- [ ] Edit text: block editing, line breaking, font reuse, embedded fallback
- [ ] Signature image, and the honest signature report
- [ ] Failure tests from the research: no lost edits on any exit path, never damage a signed file

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
- [ ] Per-document folder, `manifest.json`, first-prompt facts; the agent requests what it needs
- [ ] Hidden-text and prompt-injection tests
- [ ] Panel: visible steps, clickable citations, token count
- [ ] Setup steps when `claude` isn't installed; first-use consent per document
- [ ] Room for a second CLI
- [ ] **Blind agent test, round 3**

**Done when:** answers cite the right pages on real documents, and a hostile PDF can't reach beyond the document's folder.

**You'll need to decide:** whether a public release waits for Anthropic's written answer on subscription use, and whether to add an API-key route.

## Phase 7: Release

- [ ] Fuzzing on every parser of untrusted input
- [ ] Corpus run across all benchmark PDFs, with a dated report
- [ ] 24-hour soak, flat memory
- [ ] Developer ID signing and notarization, installer, update check
- [ ] Licences and notices for PDFium, Tesseract and the rest
- [ ] Privacy statement and first-run wording
- [ ] Final budget report against every line of `SPEC.md` §2

**Done when:** you approve the signed draft. Nothing is published without your go-ahead.

**You'll need:** a Developer ID, the licence decision, a trademark check on the name, and the GitHub repo if you want one.

---

## Later, after the first release

Real digital signatures to the official standards, signature trust checks, Windows, Link and Join text, document-wide reflow, a second agent CLI.

## Decisions waiting on you

| Decision | Needed by | Current default |
|---|---|---|
| Copy-restricted PDFs: notice with Copy anyway? | Phase 3 | Yes |
| What Save does the first time: keep one backup of the original? | Phase 4 | Yes |
| Anthropic's answer before public release; API-key route | Phase 6 | Wait; add it |
| Licence: permissive or closed | Phase 7 | Permissive or closed, undecided which |
| GitHub repo for RectoPDF; pushing the lopdf branch | When you want CI or a second machine | Local for now |
