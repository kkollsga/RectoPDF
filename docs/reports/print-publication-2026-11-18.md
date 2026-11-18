# Asynchronous print completion — 2026-11-18

This follows the UI-wait increment at `fda70495`. It removes filesystem publication from the synchronous native completion call. The historical print stall, short CPU-busy stalls and SPEC's 16 ms target remain open.

## Design and ownership

The existing print command and executor are unchanged. Rust's printing compartment admits a completion only for the exact held path and panel serial, once, with the system's success result. The host call records Pending and wakes the existing core loop. That loop starts one temporary publication worker outside its queue lock; there is no idle worker, thread, handle or per-tab state. The pending list shares the existing four-descriptor cap. A full cap leaves the next file ready and withholds its offer until there is room.

A refused thread start retains the descriptor, output target and WayBack, with retry due after one second on the injected lifecycle clock. There is no synchronous fallback or busy retry. Pending and Running publication keep the normal busy state, settle/handoff ownership and existing bounded quit grace. Merely waiting for the printer does not extend quit. Final reconciliation is queued before the last WayBack is dropped. Target removal, filesystem publication, staging cleanup and descriptor closure occur outside locks taken by UI calls; busy ends only after cleanup and closure.

The native host retains the exact ReadyPrint key and a weak operation identity. A stale or repeated AppKit callback cannot consume the current operation's key. All native aborts report failure for their own key. Filesystem and admission policy remain in Rust; Swift only forwards the OS result and identity. Shared core/session files gain lifecycle registration, not another print implementation.

## Defect found along the way

The former native callback ignored the system's success flag. A cancelled or failed operation could leave a partial staging PDF which Rust then published. Separate commit `44941d0f` fixes this: an unsuccessful completion removes staging and never publishes it, including over an existing destination. The first claimed result wins; a duplicate success cannot upgrade a cancellation. Its regression failed on the old behavior before the fix. Successful publication still uses the existing create-new/no-overwrite helper; source and signed-document bytes are untouched.

## Regression evidence

The exported-FFI regression holds real publication before it can finish, checks that `print_finished` returned while the file remains readable and busy, and then releases it and verifies publication, cleanup and closure. The synchronous implementation failed the held-return test. Wrong and repeated keys, cancellation, failed spawn and clock-bound retry, bounded quit and a full descriptor cap have in-process guards. Small-limit seeded mutations made the cap, retry and quit guards fail. Review found that reading the descriptor immediately after a wrong serial could race asynchronous consumption; the proof was strengthened to observe admission before sending the correct completion.

The existing native entry-point launch also checks stale and repeated operation callbacks. Removing only the operation-identity comparison made it fail on `old_print_callback_keeps_the_current_job`; restoring the comparison passed. The existing print-fidelity and restricted-print launches passed with the real macOS success result. No native app-launch test was added or removed. The export-policy fixture's seed anchor was updated to the new forwarding call, with a unique-anchor assertion; its policy expectations stay intact.

Both independent review rounds are complete. All eight changed/new Rust guards passed20 CI-like rounds each (160/160), including the repaired export-policy seed. All three existing native cases passed20 rounds each (60/60), under the background process clamp. Seven newly added in-process tests had a summed per-test median of1.38s in that deliberately slowed repetition; the adapted export guard's median was0.411s. This sum is test work, not CI critical-path time; native launch count is unchanged. Actual final-gate and CI timings are recorded separately.

Raw logs, seeds and probes are retained in `/tmp/rectopdf-print-publication-20261118/`, outside the disposable worktrees.

## Release measurements

The unchanged input-to-frame harness ran before at08:37 and after at09:12 on the same16GB Mac. The final after run held the local heavy queue alone. All216 quantile comparisons across54 action/tab-count cases (1,100,1,000 tabs) met `after ≤ 1.1 × before + 200 µs` (`input-comparison-quiet.json`). A first after run alongside builds missed61/216 bounds and took10.539s; it is retained as `input-after-under-load.txt`, with its failed comparison. The isolated final run took4.661s versus5.178s before. These are single harness runs with different loads, not a causal speedup claim or visible-window/120Hz measurement.

The shipping `Printing` type is344→368 bytes, measured by `size_of` against the release library with no testkit feature:24 bytes shared per app, no per-tab change. Its handed Vec allocates only while used. Completion has no idle worker; one temporary worker requests a512KiB stack. The measurements below include the running app and bundled helpers through final publication, by whole-process `phys_footprint`, using the same quiet release probes on public RFC9110 pages1–3 and USGS70277338 page1. Both pairs held the local queue alone. This is one pair, not a statistical resource benchmark.

| Print | Before / after elapsed | Before / after app+helpers peak | Before / after app lifetime peak | Output bytes both |
|---|---:|---:|---:|---:|
| RFC |0.279 /0.260s|53.55 /57.02MiB|34.06 /33.86MiB|1,319,373|
| Map |3.662 /3.736s|736.22 /737.39MiB|64.78 /64.97MiB|45,331,075|

The sampler sleeps5ms between scans; observed average sample spacing was about8.3–8.5ms. It reported no overflow, at most3 bundled processes and42 threads for each run; these sampled maxima cannot prove a shorter-lived thread peak absent. The controller CLI and shared OS printing services are outside the selected bundle tree. Final native success was true, settings retained0, and Core Graphics loading was off-main. No memory-saving claim is made. The real publication hold proves the asynchronous boundary; these end-to-end times do not isolate it.

After the measured RFC print, the probe reopened the published PDF and captured inspected real-app light/dark shots (`memory-after-rfc/light.png`, `dark.png`); screenshot work was outside the memory interval. Every probe stopped its owned app and waited for its exit. A visible window, physical printer and120Hz drawing were not tested.

## Delivery validation

The coordinator's final quick check passed at `cd289b2a`: formatting, compilation/clippy/Windows and1,230 selected tests in its last stage. The worker's earlier quick failure was the repaired seeded-export fixture described above; its specific test passed before repetition. The final release build passed. Required CI-style lifecycle and the combined full gate follow; they are recorded only after their results are read.

## Remaining work

Sample the actual short-stall interval, separating UI CPU/layout/drawing from QA reporting and command round trips. This completion fix does not identify the cause of a historical one-second print stall. Horizontal scrollbar reproduction remains a separate open issue.
