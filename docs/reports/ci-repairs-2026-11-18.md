# CI repairs, 2026-11-18

Status: the repairs below have focused evidence; remaining investigations and combined final verification are pending. This is a follow-up to the earlier short-stalls delivery, whose full check passed in 277 s at `3f852859`. That previous pass does not verify these changes. CI [44311551530](https://github.com/kkollsga/RectoPDF/actions/runs/44311551530), attempt 2, acquired runners and exposed real failures in both check and macOS 26 sandbox; it is not a green CI result.

Retained logs below are in `/tmp/rectopdf-ci-sandbox-20261118/`. Production sandbox restrictions, deadlines, retry policy, wakeup thresholds and the 2 s stall failure bound have not been relaxed.

## Sandbox control and retirement dependency

The macOS 26 sandbox job's LaunchServices positive control failed: the launch request was accepted but its marker never arrived. This does not establish the original cause. The workflow then ran a retirement diagnostic although the failure had skipped creation of its bundled worker, producing a secondary missing-worker failure.

`.github/workflows/ci.yml` now gives worker layout/signature validation its own `sandbox-worker` step. Retirement requires successful preparation and a run that was not cancelled. It still runs after a later renderer-suite failure, and its own failures still propagate. The existing `tools/check-test.sh` K2 guard failed on the old condition, then passed setup-skipped, setup-failed, suite-passed, suite-failed and cancelled scenarios. Actionlint passed. Evidence: `workflow-failed-first.txt`, `workflow-final-reviewed-segment.txt`, `workflow-actionlint-reviewed.txt`; repair `2c72e033`.

`crates/recto-platform/tests/walls.rs` now uses a small native probe app instead of a shell that forks under inherited process limits. Its bounded atomic receipt must match the private executable, PID, fresh nonce and exact canary bytes; a launch acknowledgement alone cannot pass. Cleanup identifies only this fixture's executable, waits for exit and preserves a still-live bundle for later cleanup. Registration checks query the one bundle identifier with bounded URL/path counts. Seeded denied-canary and stale-nonce receipts failed; the native positive control and full wall-denial test each passed 20 CI-like rounds. Evidence: `walls-denied-exact-guard.txt`, `walls-native-stale-receipt.txt`, `walls-20.txt`; reviewed source `59a67a53`. These strengthen the fixture and evidence without proving why the original hosted control failed.

## One bounded command collector

`crates/recto-platform/src/test_command.rs`, available only through existing test seams on macOS, is the single collector used by the walls fixtures and CLI diagnostic callers. The former duplicate source was removed in `2a391ae3`. Default capture is 64 KiB per stream through pipes, preserving `RLIMIT_FSIZE=0`; reads are fair and bounded while the producer runs. Deadline or byte-limit exhaustion is reported separately from ordinary exit/signal status.

The collector retains an unreaped owned leader until its process group is retired, including inherited writers. Lost ownership (`ECHILD`) forbids signals. Darwin's zombie-only group `EPERM` is accepted only after checked leader exit and a bounded group-only scan proves no live member; cleanup has its own bound. Five handshaken OS-process guards passed 20 CI-like rounds on reviewed `fc3816e6`. A dropped temporary old-behaviour seed failed four guards: zero-file-limit capture, stream overflow, inherited writer cleanup and signalling after ownership loss. The fifth deadline/error guard already passed that seed. Evidence: `capture-old-behavior-proof.txt`, `capture-reviewed-named.txt`, `capture-fc3816e6-final20.txt`. Intermediate failed logs remain retained and are not final-pass evidence.

## Renderer CPU accounting

`host/job_cpu.rs` records the matching worker's CPU counter at each exchange boundary, including hidden-set frames. Deadline classification subtracts that reading rather than counting CPU from earlier jobs; missing, decreasing or mismatched counters remain unknown. Both new guards failed first, then passed 20 CI-like rounds alongside the existing bounded unreadable-footprint retirement guard (`job-cpu-failed-first.txt`, `job-cpu-20.txt`; `8af4ac78`).

A local before/after check rendered four pages from RFC 9110 and the USGS map. The map's render measurements were 538,097/521,439 µs and footprint readings 302,482,272/302,711,624 bytes; memory queries took 0–2 µs across the four pages. This ordinary-load pair checks the added observation cost; it is not a demonstrated CI root cause, speedup or memory reduction (`job-cpu-before.txt`, `job-cpu-after.txt`, load logs).

## Long-session workload

`long_session.rs` and its feature-local `client.rs` retain 30 tabs, 40 pages each, three rounds, 30 edits, real app launches, and all memory, wake, frame, dormancy, worker, disk and held-document assertions. Start, a representative open/wait/state exchange and quit still run real `recto` processes. Repeated commands use a fresh public client connection, the CLI parser and the same server executor, actor and wait budgets; no edit is retried.

One same-app ordinary-load pair measured 639 CLI processes and 90 sysctl processes in 32.416 s before, versus five CLI processes, 634 public-client exchanges and three sysctl processes in 26.771 s after. Loads differed; this is one workload pair, not a causal speed estimate or an explanation of the original 300 s CI timeout. Load is sampled per round and on failure, with last-command progress retained (`long-client-before-cli.txt`, `long-client-after-public.txt` and load/hash logs; `4cef0c40`).

The first temporary seed omitted both page-layer removal and tile-content clearing in `PageArea.clear`. The unchanged guard **passed**: this omission was not caught. A second temporary seed omitted actual dormant `DocView` removal in `Reader::keep_only`; the guard failed with 30 documents held against its unchanged limit of three. Both seeds were dropped and source/good app restored. Final source passed 20 CI-like rounds (`long-client-leak-proof.txt`, `long-client-docview-leak-proof.txt`, `long-client-4cef0c40-final20.txt`). Neither seed is a product bug claim.

## Diagnostics and remaining failures

Reviewed app-evidence branch `0275bf7e` uses the same collector for native sampling, with a 1 MiB sample cap and the existing 16 MiB evidence-folder limit. Failed assertions keep a bounded excerpt; native-tool failure is valid evidence. Successful idle assertions invoke no report/sampler callback. Two in-process guards passed 20 CI-like rounds (`app-idle-final-named.txt`, `app-idle-0275bf7e-final20.txt`). This branch remains separate from integrated `89f4a921` at this report's checkpoint.

The idle rule remains settling ≤8 wakeups per two-second window and subsequent resting windows ≤80. A current real run failed with settling windows of 15–21 wakeups, even though resting windows were 18–20. Its cause remains open (`app-five-native-first.txt`). The hosted startup stall also remains open: the record reports 2,804 ms wall time; its last `doing=scroll` label does not establish the expensive operation (`source-ci-panics.txt`). Neither is fixed by bounded or lazy diagnostics.

Other native-test repairs remain in progress. A missing real zoom-change delivery was caught by the revised counter guard (`marks-missing-delivery-seed.txt`), but its action-response callback race is still being repaired. A temporary pressure-test seed removed the attached-surface filter while leaving previews intact; the test passed, so it is not evidence that the guard catches loss of visible content (`pressure-visible-discard-seed.txt`). Those temporary seeds were dropped. Diagnostic probes are not shipping fixes.

The same-hour core input comparison contains 216 comparisons and no failed bounds, with maximum published after p99 of 932.1 µs. Thirteen 1,000-tab comparisons increased by over 10 µs under different loads (`app-input-comparison.json`). These measurements do not measure a product action's process/thread/footprint cost or explain either open failure. No final combined full check or subsequent green CI is claimed here.
