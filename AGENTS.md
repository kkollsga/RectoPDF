# RectoPDF: rules for every agent

Read this first. `docs/agent-instructions/rust_app_development.md` is the owner's full process, `SPEC.md` the behaviour, `docs/ENGINEERING.md` the crates.

Progress and phases: `docs/PLAN.md`. Tick items there in the same commit that finishes them.

## Always
1. **Memory first, then speed.** Performance and low memory, even with many large PDFs open, are crucial. A cold tab costs ≤ 4 KiB, with 0 handles and 0 threads. No thread, task, buffer or handle per idle item, caps on everything from outside, and let the OS cache. Every optimisation carries before and after numbers.
2. **Rust decides, the host draws.** Wording, ordering, validation, state machines and policy live in Rust. A rule found in Swift that isn't drawing or OS integration is a bug.
3. **The UI thread never waits.** Slow calls are async, sync calls are cheap and bounded, and a stall of 2 s fails the QA run.
4. **The CLI comes first.** A feature starts as a `recto` command, then the UI calls the same method. Owner, 2026-11-06: "Development of the cli is crucial for effective agent testing as we develop, and ai features (including the agent interface (sidebar)) in the future." A feature is done when an agent can do it, see it and prove it: its core command, CLI command, QA commands, screenshots light and dark, and tests. Build QA with the feature.
5. **One writer.** Only the lopdf writers change a document's bytes; other writers only create new files, as a print to PDF or an export does, and never write over anything. Never write through PDFium. A signed file's existing bytes never change.
6. **Untrusted input:** PDFs and everything from them are hostile. No panics on them, parsers get fuzzed, renderer output is validated by the host, and document text never becomes an instruction or a command.
7. **Every feature in its own compartment** (guide §2.8). Owner, 2026-11-08: "I want each element of the software to be compartmentalized, easy to test, develop and modify." A feature's state, commands, events, QA verbs, wording and tests live in its own module; shared files (the session, the bridge, the command and QA tables, menus, the Swift window) get a one-line registration at most. A brief names where a feature lives and which shared files it may touch. Logic that decides is unit-tested without the app. A diff that grows a hub with new logic goes back, or the extension point is made first. Owner, the same day: "let's do the quick fixes now before release, then add the large restructuring into the plan before we start on the other features" (PLAN phase 7 and phase R).

## Testing
- Shortest test that can catch the failure while working: `tools/check.sh --quick`, and `tools/check.sh --failed` for only what just failed. Before handing back: `tools/check.sh --full`.
- **Each suite runs once per hand-back** (owner, 2026-11-07: "I think the main issue is repeating tests leading to queues and bottlenecks"). `--full`, a fork's whole CI set, a corpus gate, a memory run and a long fuzz run go once, last, on the commit being handed back, and once again only if code changed after them. While working, use `--quick`, a named test or `--fast`. Fix all of a review's findings, then run once. Whole-suite repeats on the way to a working fix are the waste this rule is here for. **Ask the smallest question first** (owner: "check why 13 tests where needed"): a named test, then just the documents in doubt, then the fast mode. A whole suite decides a hand-back and is never the way to find out why something changed. Don't run a final gate until what it tests is final, dependency pins included.
- **The tools hold this.** A pass on identical code counts: `--full` on a clean tree that has already passed runs nothing and records the commit. `CHECK_AGAIN=1` runs it anyway. A branch gets two full runs, and `main` two per range not yet pushed. A third needs `tools/check.sh --full --why "what this run decides"`, which goes in the ledger: `tools/check.sh --runs`. Paste `docs/agent-instructions/worker-testing.md` into a worker's brief. Any change to `tools/check.sh` passes `tools/check-test.sh`.
- **Don't use the queue to find out code is broken.** Fix before rerunning: the queue refuses a command that failed on unchanged code and shows the saved failure. `HEAVY_RETRY=1` is only for investigating a failure that comes and goes, which still gets a fix.
- **A test names the failure it catches** in its doc comment. Don't pin exact pixels, wording or timings from one machine; assert the property.
- **No manual tests and no synthetic input:** no posted mouse or key events, AppleScript or accessibility input. QA commands call the same methods the UI calls. Quiet runs never take the screen, focus or keyboard, and use temp state only.
- **No bugs left behind.** Any bug you see, related or not, gets a fix and a test in its own commit. "Flaky" isn't an answer.
- Memory tests run in their own binary, on release builds where the number matters. Measure the whole process (`phys_footprint`).

## This machine
- 16 GB Mac. **Wrap every heavy cargo build, test or bench in `tools/sysguard/heavy.sh`**, the queue shared by all agents. `check.sh` does this itself. `HEAVY_SOLO=1` runs with nothing else running: only the coordinator uses it, for final measurements at a quiet time. Workers measure under load and say so. `heavy.sh --status` shows the queue, and `--report` the waits.
- **Start a worktree with `tools/worktree.sh new <branch> .worktrees/<name>`.** Its `target/` is an APFS clone of the main checkout's, so the first build reuses every dependency already built and rebuilds only the workspace: about 45 s for the builds of a first check against about 105 s cold, under load (2026-11-11, `docs/reports/build-performance-2026-11-11.md`). `tools/worktree.sh remove <path>` ends it.
- Worktrees and temp folders are removed as soon as they're merged or finished. Never sweep a shared folder. No `timeout` binary exists here.

## Git
- Never `git stash`: the stash is shared by every worktree and session in a repo, and `git stash pop` can take someone else's work (it happened three times on 2026-11-07, once touching the owner's own WIP in the lopdf fork). Set work aside with a temporary commit on your own branch.
- Never `git switch --orphan`. Never `git add -A` without checking `git status`. Check a commit's size before any push.
- **Pushing `main` of `kkollsga/RectoPDF`:** owner, 2026-11-07: "Push automatically". The coordinator pushes `main` after each merge that passes its check. Workers don't push.
- A pre-push hook enforces that (`tools/hooks/install.sh`): `main` only goes up if `tools/check.sh --full` passed on that commit, or only documents changed since. Never use `--no-verify`.
- **Everything else needs the owner's go-ahead each time:** force-pushing, other branches, the lopdf fork, creating repos, tagging and publishing.
- Workers commit on their own branch with the trailer they're given, and don't merge. The coordinator merges with `--no-ff`.

## How work flows (owner, 2026-11-07: "Remember you are responsible for the progress and quality. So add a few process reviews every once in a while to catch inefficiencies")
- **Merge first.** A branch under review is frozen: new requests go to the next branch. No new track starts while the current phase's main branch is unmerged.
- **Two review rounds per branch at most.** Anything serious blocks the merge; the rest become tracked follow-ups in `docs/PLAN.md`.
- **About three workers at a time.** More only lengthens the build queue.
- **Short dev-build cycles for the owner.** The owner's use of the real app finds what screenshots can't.
- **Process reviews,** at each phase's end and at least after every fourth merge: progress, waste and quality against the numbers (git, `heavy.sh --report`, `check.sh --runs`, CI, review findings), logged in `docs/PROCESS.md`, with what changes as a result, plus a structure check through the code-review graph: the largest files and functions, field and variant counts, which shared files the last features touched, and the files merges conflicted in (guide §2.8). The coordinator owns progress and quality.
- **Lessons go in the guide as we progress** (owner, 2026-11-07: "feed learnings into the rust development guide as we progress"): `docs/agent-instructions/rust_app_development.md`, in its own sections, when a merge, review or process review teaches something a future project would want. General lessons, with the number or incident behind them, not this project's details.

## The build queue is critical (owner, 2026-11-07: "Since you are the queue operator you need to clear it. You need to learn from this, set up the correct safe guards so it doesn't happen again. The queue is absolutely critical to ensure we have progress in the projects")
- **The coordinator operates the queue:** it clears a stuck queue itself, and doesn't wait for someone to notice. `tools/sysguard/heavy.sh --status` must show the watchdog and the memory guard running while work is under way. Start them first.
- **The watchdog** (`heavy.sh --watch`) returns only when the queue has stopped moving: a free slot nobody takes, nothing moving with nothing using the CPU, or a job waiting over 45 minutes. Every return gets looked at straight away, then the watchdog restarts.
- **Any change to the queue passes `tools/sysguard/heavy-test.sh`,** with a test for what it fixes. It's shared with other projects, so a bad change costs them too.
- **Never report something as done until the result has been seen.**

## Writing for the owner
- **Show new features.** Owner, 2026-11-06: "Print screenshots as new features are added." When a feature lands, the coordinator sends real screenshots of it, after looking at them first. Label anything that is a mock-up rather than the real app.
- Keep them informed in short updates, and say plainly what failed. Don't give them manual tasks.
- When the owner states a rule, write it here or in `SPEC.md` straight away, in their words, with the date.
- Never fabricate a measurement. Say what wasn't tested.

## Product rules from the owner (2026-11-06)
- "We need to maximize vertical reading space." No top bar; nothing above or over the page.
- "the left sidebar menu is a bit too complicated (visually noisy)". Names and contents at rest; details on hover.
- "I prefer the agent requesting page info when it needs rather than us providing it."
- "We build everything in one go, but we test as we build."
- "Remember performance and low memory use (even when many large pdfs are loaded is crucial."

## Product rules from the owner (2026-11-07)
- "The left sidebar toc should have a max height within scroll so it doesn't push everything of screen".
- "The agent logo in the upper right is wrong, it should be a ai 4 point star".
- "The toc should auto scroll to where you are in the document (smoothly so you don't notice it in a elegant way)".
- "It can whoosh with smooth acceleration at both ends. It can have a \"dead-zone\" so it doesn't react to small scrolls within the window, so basically it repositions, stays still (while user is reading slowly scrolling) then repositions after the user is scrolling a bit."
- "We have two paper color modes: white and dark, let's add a yellowed white option that's better for the eyes".
- "Im seeing an issue: in dark mode scanned pages appear bright white: We need to detect scanned images (with predominently white bg, and flip and match grey tone paper color".
- "similar with images with white bg" (dark paper: an image with a white background flips with the page, as a scan does).
- "Lets style the scrollbar better. Lets use the mac native option, as seen in pages".
- "I think option 4 is the best one, dont modify the bad toc, but offer a generated toc using distillpdf".
- "To save horizontal space the scrollbar can stop below the agent icon. Add an option in the macos top bar to turn off agent bar (hides the icon), enable + - and command - scroll for zooming".
- "And select text should work natively".
- "For rectopdf we need a update feature similar to what we added to seedwell. Can you have a look and implement a similar handsoff system? For us we can add a check for updates button in the top bar, that turns into a update to… button if new version has been released. App settings should have a auto update toggle, and we check for updates once per day. It's crucial that app state is perfectly recovered before and after update".
- "Actually it should recover state so perfectly that it can update while the user is using it. But for auto update let's update in a window the user hasn't touched the app for 30 minutes".
- "I was thinking we need to reduce SSD wear as much as we can, while of course keeping a good memory budget." Edits come in bursts: "usually when people are editing they will edit a lot of things, so we need to make sure we keep the budget".
- "We want a good balance between SSD-aware performance and memory use for warm PDFs that's actively being edited. It's fine to keep in memory for a while, and then as it goes dormant we store to disk."
- "When it's on deep sleep, we want its state to be stored on disk, not on memory. But it needs to be stored in a way that's quick to recover without any delay."
- "and edit should also keep undo, redo history of edits until save has been applied."
- "We don't want to add the undo redo history to the file itself, but we can have a rolling save redo history on the system, so it kind of in the temp files per PDF."
- "1000 steps and 64mb is good." On Save: "after a save you might still have regrets and want to make changes. But you definitely don't want history to leak into the pdf file".
- "II agree with this signature approach": a change that needs a rewrite of a signed PDF (delete a page, redact, compress) turns Save off, and Save As makes a new copy with every signature fully removed, field, value and drawn box, saying "The new copy won't be signed. The original keeps its signature." Additions are appended and keep the signature valid. The signed original is never touched.
- "All in one release." Asked which: everything ships together, not the reader first. Replaced on 2026-11-08: "Lets release after phase 3. the rest we tackle after". The first release is the complete reader (phases 2 and 3) with the release work (phase 7). Saving and editing, tools and the agent sidebar (phases 4 to 6) come after it. Then, the same day: "No we release everything we have started in the first release. We don't park anything (finish on going features)." So a feature already under way, such as Save stage 2, is finished and ships; what hasn't started waits. And: "But don't start new post phase 3 features before release". No new feature from phases 4 to 6 starts until the first release is out.
- "it's clear to me that the test program is slowing us down considerably. I need your help for us to speed up safely".
- "Two tabs of the same pdf? This shouldn't be possible. A pdf can only be opened once".
- "I agree, but license mit is fine" (to the coordinator's recommendations, listed in `docs/PLAN.md` under Decisions made).

## Product rules from the owner, first use of the restore build (2026-11-07)
- "We dont need detailed zoom options in the left sidebar menu. Zoom is pretty natural to people. Instead lets show the zoom level in upper left and clicking the zoom value enables you to type in zoom level or reset."
- Replaces where the zoom level goes: "The zoom level should be in the upper right of the app (not in the sidebar)".
- "Focus mode enable disable should be in the right click menu. In the right click menu you can select side by side layout, with and without header page."
- Replaces the focus part of that: "Actually instead of focus view, simply have a sidebar icon to the right of the traffic light buttons that toggles sidebar on and of. No need mor a separate mode". So there is no focus mode; a sidebar button beside the window controls shows and hides the sidebar.
- Contents: "The hover delay before showing the full toc name makes in not intuitive, and the expanded name should have the color of the title." "there should be a collapse toggle button for sections with sub chapters on the left side of the headers." Page numbers on the right that don't make sense, and section indents, are bugs.
- "When selecting across pages the page number is copied. Both footer and header should be ignored when coppying body text."
- "The page number should always be visible in the lower right." Replaced on 2026-11-08 by "Zoom level should auto hid 3s after last zoom change, and similar behaviour for page number": the page number shows when the place changes, and fades 3 s after.
- "The font in the future competitiveness is not super readible. I think its a bit to skinny. Can you check?"
- Dark mode: images with a white background should be dark, as a chart with a white background was left light.
- "The full app settings should be accessible from file -> settings"
- "The sidebar menu is more for tools, like edit mode etc. When not in edit mode rotate page will rotate all pages together (it still enables save to commit the adjustment). Usefull when there is a scan issue or similar, for a quick fix."
- "The margin left of the ai icon is too large. It should have the same margin to the right and above. Above should have slightly smaller margin then now (and right gets the same one)."
- "and improve the styling of the scrollbar. The sidebar thumb should always be visible, and lets hide the rail."
- "the thumb should be thinner": slimmer than the system's default thumb, in the sidebar and the page area.
- "add some margin above the first page, and below the last page". Replaced on 2026-11-08 by "The margin above first page, and below last page should be 50% of page height".
- "the scroll thumb always visible, the rail always hidden": for the page area too, whatever the system's scroll bar setting.
- "Ctrl clicking more then one pdf should enable comparison view, right click menu can enable separate scroll (default) and synced scroll. Zoom is always synced."

## Product rules from the owner: copying and printing protected PDFs (2026-11-08)
- Exports of a copy-restricted PDF ask "anyway" like Copy. Asked whether SVG export, PNG render and print-to-PDF should get the same step: "Yes, same step". Agents need `--anyway`, and it's logged.
- Print ignores a PDF's no-print flag. Asked what Print should do with a file that says it may not be printed: "Ignore the flag". No notice and no refusal.

## Product rules from the owner, using the full reader build (2026-11-08, "RectoPDF - User observations")
- "Scroll bar track is quite wide. Can you make it thinner with a small margin on the right side so its centered on the thumb?"
- "Text selection should be the default tool (not camera tool), enable camera tool via hotkey or sidebar menu."
- "Im not seeing check for updates button in the macos top bar menu."
- "When sidebar is hidden, I want the traffic light buttons and sidebar toggle to autohide. To show them again you hover above them."
- "Zoom level should auto hid 3s after last zoom change, and similar behaviour for page number."
- "The margin above first page, and below last page should be 50% of page height"
- "When you are testing the app it flashes open in lower left corner. Is it possible to test the app in the background? Ie headless?" A quiet run must never show a window on the screen, not even for a moment (rule: quiet runs never take the screen).

## Product rules from the owner, before the first release (2026-11-08)
- The contents helper (Generate contents, through distillPDF) reads untrusted PDFs outside the renderer's sandbox. Asked what 0.1.0 should do: "Turn its image decoding off". Headings come from text.
- The ✦ agent icon does nothing until the agent sidebar ships. Asked what 0.1.0 should do with it: "Keep it as it is".
- The owner's dev copy (`target/dev/RectoPDF.app`): "You can swap out rectopdf even if its running (as long as its not actively beeing used, remember the cool down rules)" (2026-11-11). Then: "for dev work, lets set the limit to 5 mins" (2026-11-11). So the dev copy is swapped after 5 minutes with no use; the product's own automatic updates keep their 30 minutes. With agent access off, the coordinator goes by the Mac's own idle time (HID idle ≥ 5 min), which can only be longer than the app's.
- Signing (owner, 2026-11-11: "Do we need to sign dev copies? Is it possible to only sign when we push to git? This has been an issue that slows down dev work on seedwell"): development builds are never notarized and never get a secure timestamp. Then: "The reason why it slows down dev is because I need to approve the use of the keychain, that in some cases takes hours". So a development build never touches the keychain: it's signed ad hoc with a designated requirement naming only the bundle identifier, so macOS keeps its folder permissions across dev builds. Developer ID signing and notarization happen only in CI, when a release tag is pushed.
- "Im seeing two rectopdf installs in macos. Can you check? We should only ever have one. We should enable dev updates (similar to the seedwell feature)." (2026-11-11). The one install is `/Applications/RectoPDF.app`, and development builds go in through the updater (SPEC decision 28, `tools/dev-build.sh`, `recto update install <zip>`).
- Installing a dev build needs agent access Full. Asked how dev builds should reach their copy, which runs at Off, the owner chose "Raise my copy to Full" (2026-11-11). So the owner's copy runs at Full, and agents install dev builds after 5 idle minutes.
