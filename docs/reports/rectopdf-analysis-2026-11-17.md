# RectoPDF review and next priorities — 2026-11-17

RectoPDF has a strong foundation for a fast Mac reader. The biggest gap is between that shipped reader and the dependable document workspace the spec describes: large-file responsiveness still misses its targets, some copy/edit workflows lose fidelity or reach a dead end, and durable agent references and safe removal are not built. The next work should finish those foundations, not reopen phase R or start a broad collection of tools.

This review covers `cdf7c1cc` (the release record), whose shipped implementation is `ae0bf8a2`, using the spec, owner decisions, current source, release reports, test-duration ledger and three independent read-only reviews: product, safety/reliability, and architecture. It is a source/evidence review, not a new benchmark, penetration test or execution of every feature. No personal documents were opened. Existing tests were read; no new runtime measurements are claimed.

## What is already strong

- **The memory model is a genuine product advantage.** Cold tabs do not retain a renderer, descriptors, threads or tiles. The final release-candidate report puts 1,000 map tabs at 1.3–1.4 MiB over two tabs. Only the active and two recent documents stay warm. These are measurements from the 2026-11-16 release review, not this review.
- **The rendering boundary is deliberate.** PDFium is a killable helper with sandbox/wall checks, resource limits and a restricted protocol; Rust owns state and policy. The host draws native AppKit views. This is substantially better suited to the stated memory goal than introducing a web or general GPU shell.
- **Saving is designed around preservation.** One lopdf writer, append/rewrite paths, atomic replacement, parser agreement checks, refusal of rewrites over signed originals, protection of certified/locked files, and recovery tests are already present. Those are valuable foundations for later editing, not reasons to replace the writer.
- **The agent interface exists below the future sidebar.** Search, selection, rendering, page commands, watching and QA can be driven without synthetic input. Actor-aware commands and policy walks are substantial infrastructure.
- **Phase R made useful structural changes.** Commands are shared by the window and CLI, the app core has an in-process test kit, and menus/answers now receive parts. The guide's older statement that those registries still take the whole app is stale (`core/menu_rows.rs:124`, `core/person.rs:5`). The owner's decision to stop chasing structure-only targets remains appropriate.

Sources: `docs/engineering/README.md`, `docs/engineering/sandbox.md`, `docs/engineering/saving.md`, `docs/reports/performance-2026-11-16.md` (final candidate and zoom-fix sections), and the source paths below.

## The largest gaps

| Rank | Gap | Why it matters | Status/evidence |
|---|---|---|---|
| 1 | Responsiveness on expensive paths | It is part of the product's defining promise, and a UI wait is visible regardless of how good idle memory is | Confirmed source wait and existing measured misses; details below |
| 2 | Reader/editing trust | Missing real text in a copy and edits that cannot be saved break otherwise ordinary workflows | Current rules and a protected-save test demonstrate the behavior |
| 3 | Durable references and progressive retrieval | Agents still work with transient page/line answers rather than a reusable address into the document | Planned 8.1–8.3, not implemented; page identity is already available |
| 4 | Safe redaction and hidden-data removal | Neither a black rectangle nor rewriting the current PDF covers the app's retained copies | Not built; retention design is a prerequisite, not a discovered redaction leak |
| 5 | Assurance and delivery margin | The remaining display/lifecycle/security evidence gaps and thin CI margin can make subsequent features costly | Gaps and ledger figures, not proof of product failures |

### 1. Responsiveness: fix the actual slow path

The final release evidence is mixed in a useful way:

| Case | Target | Last recorded release evidence | Assessment |
|---|---|---|---|
| Ordinary cold start, page drawn | p50 ≤300 ms, p95 ≤500 ms | 244 / 262 ms after the zoom fix | Met in quiet off-screen runs |
| 10,000-page made 2 GiB PDF, first open | ≤600 ms to sharp page | `recto local … facts` 2.03, 2.03, 2.04 s on fresh clones, with most samples in `IndexedReader::page_map` | Structural open alone exceeds the target; fresh clones are a cold-cache approximation |
| Window-thread passes | ≤16 ms | Worst 35–44 ms in candidate quiet runs | Still over budget; last named action is a lead, not proof of cause |
| Sleeping text tab, sharp | ≤100 ms | 101.3–111.3 ms over five candidate wakes | Small, repeatable miss; plain paper before drawing is intended |
| Map memory, fit/~2x/~3x | ≤450 MiB accepted exception | About 323/346/336 MiB after the final zoom fix | Fixed; do not keep the morning's 456–515 MiB as an open bug |
| Shaded page memory | ≤100 MiB | 96.3–96.8 MiB; 27.1 MiB after 15 s | Met with little margin |

Source: `docs/reports/performance-2026-11-16.md`, quiet rerun and final candidate sections; `docs/releases/0.1.2.md`.

**The most concrete current UI wait is SVG paste.** `crates/recto-app/src/core/snapshot_tool.rs:39` sets a three-second patience, and `wait_for_paste` at line 687 waits on a condition variable on the UI thread. The owner accepted that temporary bound and already agreed to prepare the SVG before paste (`docs/OWNER-RULES.md:26`). That is a narrowly scoped, authorized fix with a clear acceptance property: the clipboard callback consumes ready data without waiting for rendering.

For the shorter stalls, sample the first viewport and the state-report path and retain both CPU and wall time. Some recorded stalls happened while `qa state` ran; measuring may contribute. Avoid declaring every `scroll` label the cause. The whole-file signature look on an edited 2 GiB PDF also consumes 3.5–5.3 seconds of one core, but whether Save waits remains unmeasured.

The fork already has an inbox request for lazy page lookup, signature scanning and the older large-object amplification issues. Do not change its checkout from this task. Keep work on the SVG callback and host stalls moving while that answer is pending.

### 2. Reader trust: preserve content and finish the saveable workflow

**Copy fidelity deserves higher priority than more aggressive header filtering.** The recorded real failures include table headers, continued captions, form labels and map legend text removed as page chrome. `crates/recto-index/src/chrome.rs:12` and `docs/engineering/selection.md:13` describe the heuristic. The related hidden-text rule has known split-run/white-on-black limitations (`crates/recto-index/src/lib.rs:72`, `:526`). Use the named real cases, test the actual copied output, and favor keeping uncertain content. Do not solve one class by expanding a heuristic across an unmeasured corpus.

**Protected documents have a safe but incomplete editing workflow.** `crates/recto-app/src/core/headless/protected_save.rs:10` opens a protected document, rotates it, and confirms it is edited; the test at line 35 confirms both Save and Save As are off. This protects the file, but leaves the user with edits they cannot save. The recorded owner decision is still needed: encrypted save, an explicitly unprotected copy, or no offered edits until saving is supported. This review does not choose that behavior.

Other real correctness leads should remain ahead of appearance polish: highlights on duplicated pages/after delete-save-restore, a save report that fails to answer its request, generic crash-follow `.not-merged` notice loss, and page-matcher slice 3. The 0.1.2 downgrade merge fixed a particular two-version case; it does not prove generic crash recovery notices persist (`recto-session/src/restore.rs:81`, `recto-control/src/record_store/went_back.rs:21`).

### 3. Retrieval: complete one vertical slice before libraries or chat

Search currently returns page/line/snippet data (`crates/recto-agent/src/search.rs:484`, `:1028`). The shared page identity exists in `crates/recto-core/src/page_id.rs`; locator encoding/resolution, reference-bearing search and block/section traversal remain planned.

The next feature milestone should be **8.1 plus 8.2 on one open document**:

1. Search yields a bounded, versioned reference and a link.
2. The same reference can resolve and retrieve text/selection/render output through the shared executor.
3. After a file changes, it resolves to the same page or reports moved/gone rather than guessing.
4. Every route keeps agent levels, owner-edits rules and protected-document limits.

After that vertical slice, paragraph/section expansion (8.3), UI copy-link integration (8.4), and persistent libraries (8.5–8.7) can follow their explicit prerequisites. The owner chose locator and redaction first, with the sidebar afterward (`docs/OWNER-RULES.md:32`); the sidebar is not required to wait for completion of all library steps. Sequencing beyond that is this review's recommendation. Text/content-difference comparison remains a separate unbuilt tool; existing comparison panes do not satisfy that part of SPEC §6. The `.recto` choice is settled too: our own file shaped by KGLite, with a `.kgl` export. Do not wait on a KGLite feature it declined.

Two enabling issues should be explicit:

- **Headless persistent key access is unresolved.** `crates/recto-control/src/find_keys.rs:85` gives a normal-state standalone CLI `NoKey`; it scans live pages. CLI commands connected to the running app use its index. The fast encrypted-index measurements do not establish a persistent headless service. Settle the key lifecycle before libraries.
- **The cache identity omits PDFium's version.** `crates/recto-index/src/corpus.rs:132` hashes format, normaliser and file identity/samples. After a renderer upgrade, cached character offsets can diverge from live text. Fix before the next PDFium update, using a key-mismatch test.

### 4. Removal: make retention a first-class operation

Current retention is intentional for normal editing: Save makes an original backup (`crates/recto-core/src/save.rs:352`), its hooks keep pages for undo (`crates/recto-agent/src/saving.rs:434`), and update/state recovery copies journals (`crates/recto-control/src/record_store.rs:1198`). `PRIVACY.md:24` describes these stores. They are incompatible with claiming content is irrecoverably removed unless the removal operation handles them.

Before implementation, specify the boundary and the crash behavior for the current PDF, backups/kept pages, journals and old state formats, update copies, Find text and generated facts, warm/queued render data, and any app-managed library index. State what external copies/backups lie outside the app's control rather than promising to erase them. Decide how recovery behaves before and after rename. Encode that contract in the save operation rather than leaving the present future-redaction comment as the guard.

Acceptance must include independent extraction of text, images and metadata from the output and inspection of app-managed remnants after forced interruption. A signed original's existing bytes must remain unchanged. This is the highest-risk next tool, so start with the design review; broad text editing, form creation and signing can wait.

### 5. Assurance, authority and delivery

**CI is inside budget with almost no margin.** Read directly from `.git/recto-ci-times` on 2026-11-17, the last five green longest-job durations are 1,234, 1,309, 1,176, 1,254 and 1,133 seconds. Median **20:34**, budget **21:00**, margin **26 seconds**. These are the latest retained measurements, not a new CI run or a live GitHub status. The prior process review already made this the first task after the release.

`tools/check.sh --slow` was read without running tests. Its latest retained full run is `636ff38d` on 2026-11-16: core/process 105 seconds wall, app tiers 143 seconds wall, slow stage 46 seconds. The largest individual recorded cases were the late-restore test at 44.8 seconds, hostile-state fuzz smoke at 39.7, and long session at 36.5. Investigate work and runner variation before cutting coverage; long runtime alone does not prove a test is wasteful. Preserve the project's seeded-bug proof for any removed or rewritten test.

**Evidence still owed:** a corrected 24-hour flat-memory soak, visible-window memory and actual 120 Hz display frames, real RTL text, host gestures/context menus and printer integration, dark-wake/locked-screen update behavior, and reconciliation of sandbox OS-matrix/full signed-versus-ad-hoc evidence. Notarization alone does not close those tests. The headings helper is isolated and watched but unsandboxed, with its/distillPDF's internal fuzzing still a gap; a fuzzed answer does not fuzz the producer.

**Keep two narrow authority risks visible without overstating them.** An explicit `RECTO_STATE_DIR` selects a disk `find.key`, even for a release-launched instance (`recto-control/src/state.rs:44`, `find_keys.rs:85`). This does not decrypt the normal app's corpus; review whether that exception is confined as intended. Also, menu `RowParts::person()` is available to rows which can run through `Agents::AsTheMenu` (`core/row_parts.rs:173`, `core/menu_rows.rs:224`). No present misuse was found. The carried-forward keychain first-unlock/certificate/shared-team probes, lasting-key recovery case, helper-entitlement coverage and helper-identity proposal remain explicit in PLAN. Intentional `index --agent` Markdown exports are plaintext by the owner's "Ship as is, stated" decision and are distinct from encrypted corpus storage. Narrow that capability when next touching those rows, rather than launching a new architecture phase.

The graph helped locate code, but its method ownership conflates some same-named types across crates and includes test helpers among apparent production functions. No naive whole-type counts are reported as measurements. Existing ratchet ceilings are guards, not fresh structural counts.

## Recommended next delivery

Start with a **bounded CI-time diagnosis and the SVG no-wait fix**, then use two independent tracks: **locator 8.1 → reference-bearing search 8.2**, and **redaction/Remove Hidden Data retention design → guarded implementation**. Land small copy/recovery fixes alongside where file ownership allows. This follows the owner's agreed direction and addresses both the differentiating agent workflow and a commonly needed tool.

Do not start the sidebar, cross-library graph service, broad text editing, cryptographic signing, Windows host or another general restructuring in this milestone. Those remain planned; they are not release regressions. `recto-sign` is explicitly a placeholder and does not validate signatures (`crates/recto-sign/src/lib.rs:1`).

No delivery dates or new performance claims are inferred from old agent-day estimates. Each implementation brief should set its own measured acceptance criteria and date when work starts.

## Backlog cleanup performed

`docs/PLAN.md` now leads with the published state and an ordered next-work list. The duplicated summary/release log was removed from the active plan; its prior form remains at `cdf7c1cc:docs/PLAN.md`. Unique outstanding release findings and decisions were carried forward.

Closed or corrected with evidence: disk sleep previews and their derived defects/questions; obsolete map/shaded-memory misses; signed update rehearsal; the highlight pilot; existing comparison-pane/highlight scope while retaining the separate text-difference tool; completed snapshot/link/Save routes; implemented one-second lock retries; delivered KGLite asks and the settled file choice. Redaction's duplicated save prerequisites were consolidated into one contract. The generic crash notice, protected-save decision, sandbox evidence, highlight identity defects and other unresolved safety findings remain open.

This is a prioritization and evidence cleanup, not a claim that every historical unchecked item was reproduced or that a successful release closes all its follow-ups. The detailed phase sections keep those leads until a targeted read or test decides them.
