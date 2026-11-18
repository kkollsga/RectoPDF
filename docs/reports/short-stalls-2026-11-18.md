# Short UI stalls — 2026-11-18

This increment follows `cf6dfa7d`. It identifies a real main-thread print-settings wait and moves that work into the existing background preparation job. It also fixes screenshot encoding/writes on main, concurrent QA diagnostic ownership and an uncovered autorelease-cleanup interval. It does not close the historical one-second print finding or the remaining startup/layout misses against 16 ms.

## Evidence from the actual interval

The first release probe armed Time Profiler before opening the document and driving actions. A matching unprofiled quiet run also recorded over-budget stretches, so the observations were not treated as profiler-only failures. A second temporary build emitted bounded Points of Interest intervals for the actual native callback, layout, report, drawing and printing phases. Each trace lasted eight seconds, targeted only the owned quiet app and used temporary state; no posted events or focus changes. Instrumentation was removed and clean source hashes checked before implementing the fixes.

The first phase trace found a 102.395 ms model-change callback while its reader refresh was much shorter. Narrower instrumentation then isolated these maxima in a separate run:

| Phase | Observed wall time |
|---|---:|
| Model-change callback | 89.819 ms |
| Ready-print handoff | 88.824 ms |
| Paper-size and margin reads | 88.778 ms |
| Print job-title lookup | 20 µs |
| Print-operation creation/start | 136 µs |
| Full QA report construction | 974 µs |
| QA answer encoding/delivery | 426 µs |
| Window recreation | 7.751 ms |

These are individual maxima, not durations to add together. The narrower trace's sidebar refresh also reached 21.353 ms, with one cell layout at 18.603 ms. Those remaining layout costs are separate from the print fix. A last `scroll` or `state` label was not used as proof of which phase was slow.

A further bounded capture caught this main-thread stack during the original paper read: `NSPrintInfo.paperSize` → page-format reconciliation → `PMSessionGetCurrentPrinter` → default-printer lookup → `xpc_connection_send_message_with_reply_sync` → `mach_msg`. This proves that the read enters a synchronous print-service call; the enclosing measured read lasted about 89 ms. It does not assign the entire duration to one stack sample or distinguish every scheduler wait. The profiler's sampling weights are observations, not exact CPU durations.

## Changes and regressions

- **Print settings (`a6c0dee8`, `Print.swift`).** After checking the ready key and original window, main makes one checked, distinct private `NSPrintInfo` copy. The existing single background job adjusts the already-consumed page range and reads paper size once plus four margins, then loads the PDF. It returns immutable geometry with its owned settings/document to main for view/operation creation. The preview panel retains its original settings, so removing our dictionary entry alone was not sufficient ownership. There is no shared-object fallback, global Sendable conformance, extra job or idle state. Failed copy, load or replaced window completes only its exact key unsuccessfully. Scalar copy/paper diagnostics retain no settings or document. The existing fidelity launch failed first on a real main-thread paper read: 94,600 µs, `main_thread=true`; it passed after the move, preserving output dimensions, page range and the existing off-main document-load guard.
- **Screenshots (`9e298f73`, `QAChannel+Shots.swift`).** Window, menu and Settings captures share one background encoding/atomic-write helper. AppKit drawing stays on main, and the existing bitmap transfers without a second bitmap/Data copy. Existing errors and Rust's new-file/file-wall policy remain. A temporary back-to-main seed failed the existing screenshot launch; its expanded guard reads real PNGs on all four routes and checks the actual encoding/write thread. No app-launch test was added.
- **Concurrent QA labels (`36c67c8f`, `core/qa/pending.rs`).** Each existing answer slot owns a fixed label until its ask returns, including decoding. Completion removes only its own slot and restores the latest admitted surviving label. A consumed reply cannot be refilled by a duplicate. Both real concurrent-request completion orders failed first on the former unconditional clear. Error, zero-wait timeout and held-decode/duplicate cases passed. Payload destruction and decoding remain outside the answer-map lock. Compiler `QaBridge` size stays 200 bytes; its ten fields are unchanged, with 19→20 methods across its whole implementation. Bounded answer slots gain a 25-byte label; this is not a zero-heap-growth claim.
- **Watchdog cleanup (`e2f81c48`, `MainWork.swift`).** The watch now spans the entire synchronous autorelease pool, including its drain. If the run loop already owns the interval, the block does not end it. The existing entry-point launch exercises the same helper with an observable pool tail; the original ordering failed only the intended new key. The final helper retains no state or closure. Swift required an escaping input annotation when passing the existing escaping work through an arbitrary synchronous pool callback; it does not introduce another queued job.

Apple's local SDK `NSPrintPanel.h` documents the preview panel's retained settings; `NSPrintInfo.h` supports copying but provides no blanket concurrency guarantee. The [threading guide](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Multithreading/ThreadSafetySummary/ThreadSafetySummary.html) describes serialized mutable-object use generally. The private-copy ownership sequence is our application invariant, not a claim that all print-info operations are thread-safe.

## Validation and resources

Independent source reviews approved all four fixes. Screenshot and print-fidelity guards each passed 20 CI-like rounds; the QA ownership/decode group passed 100 cases over 20 rounds, and the pool/entry-point guard passed all 20 rounds. The combined quick check passed 1,227 selected cases in its last stage; the final release build passed. No native launch test was added or removed.

The release input harness ran before and after within the same hour at 1, 100 and 1,000 tabs. All 216 comparisons met `after ≤ 1.1 × before + 200 µs`. The 1,000-tab p99 increases above 10 µs were Rotate 126.9→137.3 µs and Open New File 441.3→451.5 µs; several other quantiles decreased. Ordinary action implementations did not change. Different binary layout, scheduling and load are uncontrolled in this single pair, so these small tail changes are not attributed to the QA fix or presented as a speedup. Raw comparison: `input-comparison.json`.

The resource scenario prints RFC 9110 pages 1–3 or USGS70277338 page 1, then captures the window, one menu, all menus and Settings. Each before/after pair used a fresh quiet app, the same release controller/corpus and the local queue held alone. App-plus-bundled-helper peaks use whole-process `phys_footprint`; the controller and shared OS print services are excluded. This is one pair per document, not a statistical speedup or memory-saving claim.

| Scenario | Elapsed before→after | App+helpers peak before→after | App lifetime peak before→after |
|---|---:|---:|---:|
| RFC print | 0.299→0.241 s | 57.06→50.13 MiB | 33.69→33.83 MiB |
| RFC four captures | 0.110→0.104 s | 77.10→74.94 MiB | 68.30→68.33 MiB |
| Map print | 3.748→3.769 s | 737.24→738.28 MiB | 65.03→64.69 MiB |
| Map four captures | 0.126→0.125 s | 62.66→61.39 MiB | 67.34→67.17 MiB |

Final settings copies took **23 µs and 19 µs on main** in these two release probes. Paper preparation took 62,279 µs and 2,935 µs **off main**; document opening/page counting also remained off main. The original panel object never crosses the handoff. These measured copy costs do not guarantee all printer configurations behave identically. All four final capture responses reported off-main encoding/write. PDF output sizes matched within each pair: 1,319,373 and 45,331,075 bytes. Native result was successful; settings retained zero entries.

The sampler sleeps 5 ms between scans; observed spacing was roughly 8–9 ms. It reported no overflow, at most three bundled processes and 43 threads across these intervals. Shorter peaks can be missed; the observed thread maximum is not a claim that the implementation removed a thread. The print move reuses its existing job, screenshot work is bounded by the existing QA-job cap, and neither adds idle per-tab work. The RFC output was reopened and inspected in real light/dark shots after the measured intervals (`resources-after-rfc/light.png`, `dark.png`). Every probe stopped its app and waited for exit.

Required `tools/check.sh --like-ci lifecycle` passed on the final source at `8edb98cc`, exit 0: 30/30 lifecycle cases in 213.376 s, then 3/3 registration cases in 15.172 s. Load was 1.97/2.31/3.80 before and 7.10/5.59/4.90 after. The combined full gate and CI are recorded only after their results are observed. Raw traces, seeded failures, process sampling and scripts remain under `/tmp/rectopdf-short-stalls-20261118/`, outside disposable worktrees. Traces are local diagnostic artifacts; only bounded summaries are committed.

## Remaining work

Startup and first sidebar/text layout still exceed 16 ms in some captured passes. Keep investigating those specific phases with an unprofiled control; do not reinterpret a whole run-loop pass as one scroll call, or use an idle stack sample to explain earlier work. The horizontal-scrollbar report still needs its original document/zoom/window reproduction. Physical printing and a visible-window frame rate were not tested by these quiet runs.
