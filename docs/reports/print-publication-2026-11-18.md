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

Both independent review rounds are complete. All eight changed/new Rust guards passed 20 CI-like rounds each (160/160), including the repaired export-policy seed. All three existing native cases passed 20 rounds each (60/60), under the background process clamp. Seven newly added in-process tests had a summed per-test median of 1.38 s in that deliberately slowed repetition; the adapted export guard's median was 0.411 s. This sum is test work, not CI critical-path time; native launch count is unchanged. Actual final-gate and CI timings are recorded separately.

Raw logs, seeds and probes are retained in `/tmp/rectopdf-print-publication-20261118/`, outside the disposable worktrees.

## Release measurements

The unchanged input-to-frame harness ran before at 08:37 and after at 09:12 on the same 16 GB Mac. The final after run held the local heavy queue alone. All 216 quantile comparisons across 54 action/tab-count cases (1, 100 and 1,000 tabs) met `after ≤ 1.1 × before + 200 µs` (`input-comparison-quiet.json`). A first after run alongside builds missed 61/216 bounds and took 10.539 s; it is retained as `input-after-under-load.txt`, with its failed comparison. The isolated final run took 4.661 s versus 5.178 s before. These are single harness runs with different loads, not a causal speedup claim or visible-window/120 Hz measurement.

The shipping `Printing` type is 344→368 bytes, measured by `size_of` against the release library with no testkit feature: 24 bytes shared per app, no per-tab change. Its handed Vec allocates only while used. Completion has no idle worker; one temporary worker requests a 512 KiB stack. The measurements below include the running app and bundled helpers through final publication, by whole-process `phys_footprint`, using the same quiet release probes on public RFC 9110 pages 1–3 and USGS70277338 page 1. Both pairs held the local queue alone. This is one pair, not a statistical resource benchmark.

| Print | Before / after elapsed | Before / after app+helpers peak | Before / after app lifetime peak | Output bytes both |
|---|---:|---:|---:|---:|
| RFC | 0.279 / 0.260 s | 53.55 / 57.02 MiB | 34.06 / 33.86 MiB | 1,319,373 |
| Map | 3.662 / 3.736 s | 736.22 / 737.39 MiB | 64.78 / 64.97 MiB | 45,331,075 |

The sampler sleeps 5 ms between scans; observed average sample spacing was about 8.3–8.5 ms. It reported no overflow, at most 3 bundled processes and 42 threads for each run; these sampled maxima cannot prove a shorter-lived thread peak absent. The controller CLI and shared OS printing services are outside the selected bundle tree. Final native success was true, settings retained 0, and Core Graphics loading was off-main. No memory-saving claim is made. The real publication hold proves the asynchronous boundary; these end-to-end times do not isolate it.

After the measured RFC print, the probe reopened the published PDF and captured inspected real-app light/dark shots (`memory-after-rfc/light.png`, `dark.png`); screenshot work was outside the memory interval. Every probe stopped its owned app and waited for its exit. A visible window, physical printer and 120 Hz drawing were not tested.

## Delivery validation

The coordinator's final quick check passed at `cd289b2a`: formatting, compilation/clippy/Windows and 1,230 selected tests in its last stage. The worker's earlier quick failure was the repaired seeded-export fixture described above; its specific test passed before repetition. The final release build passed. Required `tools/check.sh --like-ci lifecycle` passed at `c7a8b730`, exit 0: 30/30 lifecycle cases in 178.243 s, then 3/3 registration cases in 14.929 s.

Final `CHECK_ALL=1 tools/check.sh --full` passed once at `abcdf551`, exit 0, 275 ledger seconds: 3,276 Rust tests (1,188 in 21.675 s plus 2,088 in 40.202 s), 203 native tests in 124.548 s, four slow cases in 44.925 s, bundle 2/2 and registration 3/3. Seven new in-process tests totalled 0.454 test-seconds in this ordinary local gate; that is not a CI critical-path estimate. No source changed after the gate.

Main merge `86bb8b61` is identical to the checked staging tree. Main's full command accepted the prior pass without rerunning suites, and the merge was pushed. CI [44266732369](https://github.com/kkollsga/RectoPDF/actions/runs/44266732369) passed all four jobs. The kept `push-main.sh` record says 799 s (13:19) for check, latest five-green median 1,133 s (18:53), budget 1,260 s (21:00), 127 s median headroom. Budget unchanged.

Compared with the preceding responsiveness CI, the decoded logs show compile/lint 82→35 s, combined Rust test/build stages 416→274 s, app build 120→73 s and native stage 315→228 s. Seven new in-process tests totalled 0.391 test-seconds on this CI run; native launches stayed at 203. Several unrelated stages became shorter too. This is a stage accounting comparison, not a causal attribution of the 393 s job reduction to asynchronous publication. Raw heavy/light logs and `ci-added-tests.json` are retained with the other evidence.

Both temporary worktrees, their local branches and the baseline app copy are removed; the graph root is back on main. The baseline was already unregistered, confirmed against Launch Services before removal. The regular `app/build.noindex/RectoPDF.app` development bundle was refreshed successfully from main; the installed release was not replaced. Owned memory-guard/watchdog processes, including the brief dev-build pair, were stopped and their exit observed. The CI watcher exited 0. Raw evidence and inspected light/dark shots remain outside the removed worktrees.

## Remaining work

Sample the actual short-stall interval, separating UI CPU/layout/drawing from QA reporting and command round trips. This completion fix does not identify the cause of a historical one-second print stall. Horizontal scrollbar reproduction remains a separate open issue.
