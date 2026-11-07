# RectoPDF: rules for every agent

Read this first. `rust_app_development.md` is the owner's full process, `SPEC.md` the behaviour, `docs/ENGINEERING.md` the crates.

Progress and phases: `docs/PLAN.md`. Tick items there in the same commit that finishes them.

## Always
1. **Memory first, then speed.** Performance and low memory, even with many large PDFs open, are crucial. A cold tab costs ≤ 4 KiB, with 0 handles and 0 threads. No thread, task, buffer or handle per idle item, caps on everything from outside, and let the OS cache. Every optimisation carries before and after numbers.
2. **Rust decides, the host draws.** Wording, ordering, validation, state machines and policy live in Rust. A rule found in Swift that isn't drawing or OS integration is a bug.
3. **The UI thread never waits.** Slow calls are async, sync calls are cheap and bounded, and a stall of 2 s fails the QA run.
4. **The CLI comes first.** A feature starts as a `recto` command, then the UI calls the same method. Owner, 2026-11-06: "Development of the cli is crucial for effective agent testing as we develop, and ai features (including the agent interface (sidebar)) in the future." A feature is done when an agent can do it, see it and prove it: its core command, CLI command, QA commands, screenshots light and dark, and tests. Build QA with the feature.
5. **One writer.** Only the lopdf append writer changes PDF bytes. Never write through PDFium. A signed file's existing bytes never change.
6. **Untrusted input:** PDFs and everything from them are hostile. No panics on them, parsers get fuzzed, renderer output is validated by the host, and document text never becomes an instruction or a command.

## Testing
- Shortest test that can catch the failure while working: `tools/check.sh --quick`. Before handing back: `tools/check.sh --full`.
- **A test names the failure it catches** in its doc comment. Don't pin exact pixels, wording or timings from one machine; assert the property.
- **No manual tests and no synthetic input:** no posted mouse or key events, AppleScript or accessibility input. QA commands call the same methods the UI calls. Quiet runs never take the screen, focus or keyboard, and use temp state only.
- **No bugs left behind.** Any bug you see, related or not, gets a fix and a test in its own commit. "Flaky" isn't an answer.
- Memory tests run in their own binary, on release builds where the number matters. Measure the whole process (`phys_footprint`).

## This machine
- 16 GB Mac. **Wrap every heavy cargo build, test or bench in `tools/sysguard/heavy.sh`**, one at a time across all agents. `check.sh` does this itself.
- Worktrees and temp folders are removed as soon as they're merged or finished. Never sweep a shared folder. No `timeout` binary exists here.

## Git
- Never `git switch --orphan`. Never `git add -A` without checking `git status`. Check a commit's size before any push.
- **Pushing, creating repos, tagging and publishing need the owner's go-ahead each time.**
- Workers commit on their own branch with the trailer they're given, and don't merge. The coordinator merges with `--no-ff`.

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
