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

Raw logs, seeds and probes are retained in `/tmp/rectopdf-print-publication-20261118/`, outside the disposable worktrees. Final repetition, release measurements and delivery gate results are recorded below after they are observed.

## Remaining work

Sample the actual short-stall interval, separating UI CPU/layout/drawing from QA reporting and command round trips. This completion fix does not identify the cause of a historical one-second print stall. Horizontal scrollbar reproduction remains a separate open issue.
