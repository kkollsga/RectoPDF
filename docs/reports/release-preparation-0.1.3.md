# 0.1.3 release preparation, 19 November 2026

This is preparation evidence, not a published-release result. Nothing has been tagged or published yet.

## Completed

- Version and public notes checked. Independent review of the whole delta from 0.1.2 approved the source and the stated limits.
- An initial full-LTO ad hoc release build drew its PDF with a sandboxed worker and passed the release/no-test-hook checks. Its assets were preserved outside the build folder before subsequent debug work. The later installer-launch seam correction means this initial build is not the final-source dress rehearsal.
- The real draft-checker self-test passed with published 0.1.2 and the local 0.1.3 assets, including the intended rejection of the ad hoc bundle's signing/notarization. Wrong-feed seeds now always differ from the positive fixture.
- Carry-checker self-tests passed against published 0.1.0, 0.1.1 and 0.1.2 with that initial local build. These tests cover deliberately lost journals, changed places, lost Find state and the promised retained journal, followed by each real positive pair. They are checker proofs, not the signed final draft gates.
- Two local titlebar y-coordinate differences were observed: 7 in the published apps and 8 in the local app. `vtool` reports linked SDK26.5 on all three published apps and SDK15.0 on the local executable. The checker permits only those two exact y changes for that SDK pair, leaving x/width/height and other SDK/value transitions compared. This is an observed association, not an independently proved AppKit causal mechanism. SDKs and the applied keys are retained in `B-titlebar.json`.
- The original retained-journal fault seed used an obsolete `.not-merged` filename and did not remove the `closed-*.log` that a successful merge now preserves. The corrected seed removes the exact promised original bytes; its focused real run failed only at the missing retained-list assertion. Failed/interrupted checker runs now keep their evidence; injected exit23 proved the old trap removed it and the corrected trap retains it.
- Carry cleanup now confirms private app/helper exit, unregisters at most four exact known bundle URLs, proves their absence and only then removes the private folder. Thirteen mocked guards passed, relevant guards failed first, and independent review approved the implementation. Native runs confirmed complete cleanup. No discovered application PID is signalled.
- Duplicate Dock records were corrected separately: see [single-install report](single-install-2026-11-19.md). The Dock remained at one RectoPDF entry across the draft-checker and all three carry-checker runs.

Evidence root: `/tmp/rectopdf-release-0.1.3-20261119/`. Full logs, the before/after Dock records, SDK reads and failed-first proofs are retained there.

## Installer cleanup findings

A stranded helper from an earlier test was traced to `updater::builds::tests::going_back_lands_on_a_build_only_with_the_switch_on`: its rollback path bypassed the fake host and detached a copied `/usr/bin/true`, after which the fixture was removed. Its loader stall is unexplained; the sample does not show a Rust installer wait. The reviewed correction routes Install and Back through one real launch implementation and the injected host seam. The existing rollback test removes the fixture helper and proves Back is captured exactly when allowed, so even its failed-first run cannot strand a helper. Both named seam tests passed.

The subsequent source review found separate older defects. `tests/installer.rs` teardown signals PIDs discovered by executable path; its replacement uses a private cooperative fixture stop marker and bounded exit observations. In production, `install.rs::stop_new_app` also signals discovered PIDs, and `go_back` restores the record and swaps bundles even after both stop waits expire. This is a release blocker: restoring files while the new app may still write them is unsafe. The owner has been asked to choose between preserving forced automatic rollback with a stable launch-identity implementation, or refusing rollback when exit cannot be established. Neither option permits restoring over a live or unknown writer. No release is published while that decision and correction are outstanding.
