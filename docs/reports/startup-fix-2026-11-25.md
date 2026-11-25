# Startup fix tried and dropped: warming the sidebar's mark glyphs (2026-11-25)

Outcome: **dropped, nothing merged.** The branch `build/startup-fix` holds only this report.

## What was tried
`docs/reports/startup-stalls-2026-11-25.md` found that in a restored launch the first 14 pt `.AppleSystemUIFontMedium` measure costs 5.2 to 6.6 ms on the main thread. The try: warm CoreText for the cell's mark glyphs ("›" and "⌄", 14 pt and 12 pt Medium) on a background queue at launch (`FontWarm.start()` first in `StartupWatch.launch`), with an entry-point key in `what_appkit_calls_reaches_the_app`. Both seeded bugs (async made synchronous; start call deleted) failed that test as intended.

## Measured (release, interleaved before1, after1, after2, before2; E and R; 1 discarded launch plus 10 runs each; medians of 20)
- The costly string is "⌄" (U+2304) at 14 pt Medium, confirmed by a temporary probe recording short strings.
- R: `text_size` max 5.65 ms to 0.17 ms; `sidebar_cell_layout` max 5.87 ms to 0.24 ms; the "⌄" call 5 to 7 ms to 0.08 ms median (0.445 ms max).
- The cost moved to main: `launch_begin` to `menu_model_ready` went from 0.2 ms to about 2.6 ms (2.4 to 3.7 ms across both after batches, E and R; the before2 batch at similar load stayed at 0.21 ms).
- E settled footprint 13.11 to 13.59 MiB median (+0.48 MiB; the guard was +64 KiB); R 25.63 to 25.84 MiB.
- Load mismatch, said plainly: before1 ran at load 9.7 to 11.5, after1 and after2 at 28 to 38, before2 at about 30. `launch_begin` to `host_ready` (E 58 vs 194 and 96 ms, R 70 vs 132 and 97 ms) is not clean for that reason. The 2.6 ms and the footprint are not explained by load.
- Not analysed: `over_budget` entries after `host_ready`.

## Call-backs
1. NSFont is not declared Sendable in the SDK (`NSFont.h`: no `NS_SWIFT_SENDABLE`), so creating it off main needs a `nonisolated(unsafe)` escape.
2. No single Swift point knows the restored tab count: `.restoreQueued` follows `core.beginRestore()`, which only starts the read.
3. `what_appkit_calls_reaches_the_app` launches empty and then opens a file, so a "warm only with restored tabs" key would be absent there.

## Decision
Dropped. About 5 ms once is under the 16 ms frame budget, so it is not a stall by rule 3's measure; the safe variants cost more than they save (an NSFont escape, a warm that starts right before the first layout and may contend, an empty window paying a thread and footprint).

Reopen if a measured stall over 16 ms appears in the first sidebar layout.

Raw evidence: `/Volumes/EksternalHome/Koding/Rust/rectopdf-evidence/startup-fix-2026-11-25/` (before1/before2/after1/after2, E and R, with load lines).
