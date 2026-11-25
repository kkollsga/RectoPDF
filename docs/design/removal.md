# Safe removal: retention and transaction design

Design draft, 2026-11-18. Read against RectoPDF `bf700492`'s worktree base, the current `SPEC.md` and owner decisions. No removal command, new writer or deletion is implemented by this document. Proposed interfaces and budgets below need review before code.

The first tool should remove **explicitly selected metadata, attachment and action categories**, with truthful counts and exclusions. Region redaction is a separate content-transformation problem. Both need the same retention transaction before either can claim a completed removal.

## 1. Settled rules and open decisions

The owner chose normal Save/Save As, tools changing the open document rather than having their own save buttons (`SPEC.md` §4/§6, decision 16). Removal rewrites through the lopdf writer; PDFium never writes a document. A confirmed redaction or hidden-data removal clears history (`SPEC.md`, decision 25). A signed original's existing bytes never change: Save is off for a rewrite and Save As makes an unsigned copy with the existing notice (decision 27; `docs/OWNER-RULES.md`, 2026-11-07). An agent's destructive commit needs Full; an agent at any level still has no access to protected-document content or the person's password.

The coordinator's design steer of 2026-11-17 accepts this scope: sanitized output plus enumerated RectoPDF-managed derivatives which could restore removed content; an explicitly retained Save As source stays; signed sources stay unchanged; external exports, OS snapshots and physical-media erasure are excluded and said. Incomplete provenance refuses rather than purging unrelated work. This is a design constraint, not permission for implementation to delete anything yet.

The remaining choices must be resolved before the slice which depends on them:

| Choice | Recommendation to review | Why it needs an explicit decision |
|---|---|---|
| Exact categories behind Remove Hidden Data | First expose metadata, attachments and actions separately; show what remains | Removing all annotations, form values, alternate text or invisible text can destroy wanted content; the product has not chosen those defaults |
| Shared derivatives and aliases | Operate on verified provenance and grants; refuse an ambiguous or shared artifact | Same text, a matching basename or a partial file stamp is not authority to discard another document's history |
| Save As source derivatives | Distinguish the retained source's legitimate state from history borrowed into the new output; clear the output's history | Keeping the source intentionally permits its content to be read/indexed again. A global “no old content anywhere in the app” claim would contradict that exception |
| Clipboard | Never claim clipboard erasure; decide whether to release only a still-owned RectoPDF offer | Another app may already have read it, and clearing a changed pasteboard would discard somebody else's copy |
| Region-redaction strategy | Keep vector-preserving removal and an explicitly labelled rasterized copy as unselected alternatives | Rasterization can remove recoverable source objects but loses text/vector fidelity and changes size/resolution; it is a product choice, not an implementation shortcut |
| Save confirmation vocabulary | Use the existing save executor with a typed removal confirmation bound to its plan | The flags/codes below are proposed API, not settled wire schema |
| Durability | Specify app-crash guarantees separately from power-loss guarantees; accept no best-effort durability warning as completed safe removal | Present Save can report durability warnings. Removal needs a stricter result contract and filesystem evidence |

## 2. What completion means

“Removed” means that the selected kinds are absent from the new PDF's reachable and historical data, and the enumerated app-owned stores cannot restore the removed version. It does **not** mean overwritten SSD blocks, erased APFS snapshots, deleted user exports or remote backups. Unlinking encrypted corpus files is logical removal, not cryptographic erasure: the corpus key is derived from a lasting master key.

For Save As, the result must explicitly name that an unmodified source remains. Sanitizing that new output does not grant deletion of the source, its other exports or an independently retained source history. If the source is deliberately opened again, reading/indexing that source is expected, not resurrection into the sanitized output. The output starts a clean history and may never borrow old source pages to undo removal.

The guarantee applies only after a successful commit. Before commit the source still exists. After a failure the answer says whether the target is unchanged, a sanitized target is already in place with cleanup pending, or its identity could not be established. Never use the ordinary “nothing was written” error once the target may have been replaced.

## 3. Retention inventory and caller evidence

Storage paths here are relative to the configured private state root unless a cache root is stated. Source citations beginning with a crate name are relative to `crates/`; subsequent abbreviated module paths keep the crate named in that row. `docs/`, `Cargo.toml`, `Cargo.lock` and `clippy.toml` are relative to the repository root. Fork paths in §7 are relative to the named pinned lopdf commit. `RECTO_STATE_DIR` still carries the quiet-run file wall; a removal transaction cannot widen it. Every row is required for the inventory, not a promise that current APIs already remove it safely.

| Store | What can survive | Current source | Removal obligation / gap |
|---|---|---|---|
| Live source and target PDF; sibling writer temporary | Original streams, incremental revisions, hidden fields, metadata; an interrupted staged file | `recto-core/src/save.rs:292–385`, `:427–454` | Rewrite, never append; verify candidate before retention changes; no clone of the original as a removal candidate; keep explicit Save As source exception |
| Original backup, `backups.noindex/original-<path hash>.pdf`, partial backup | Whole original, including removed pages | `recto-core/src/save.rs:352–355`; `save/backups.rs:9–64` | Suppress new backup and retire verified earlier backups. A canonical-path hash has no complete rename lineage; unknown old aliases are a blocker, not permission to clear all backups |
| `session/journals/journal-*.log`, `closed-*.log` | Unsaved/undo steps, page sources, highlights, prior saved point and source paths | `recto-control/src/record_store.rs:1–17`, `:585–719`; `recto-session/src/record.rs:879–923` | Stop writers; remove target's old checkpoints and produce clean post-save state. Clearing only the active list is insufficient |
| Journal variants `.unreadable`, `.kept`, `.not-merged`, `.earlier`, temporary/replaced names | Older whole checkpoints or partial lines which ordinary restore passes over | `record_store.rs:1233–1283`, `:1429`; `record_store/went_back.rs:129–151` | Include recognized variants. An unreadable/foreign-version artifact must not be guessed to belong to the target or silently skipped |
| `session/journals/kept-*.keptpages`, older `.pdf` names | Whole deleted pages, including their resources | `record_store.rs:962–997`; `recto-session/src/record.rs:564–583` | Suppress `keep_pages`; retire verified old kept sources and all referring histories. Shared references need an explicit scope decision or refusal |
| `session/journals/pages-*.ids` | Digests/page identities associated with old edit history | `record_store/page_ids.rs`; `record_store.rs:6` | Retire with its journal; never use it to infer arbitrary content equality or deletion authority |
| `session/journals/sleep-*.bin` | Page sizes, contents/headings and source/version facts; historical formats may contain page images | `record_store/sleep_files.rs:1–25`, `:43–113`; `recto-session/src/sleep.rs:24–25` | Cancel queued sleep writes and remove affected versions. Normal first-launch cleanup is capped and best-effort: it is not a removal proof |
| `session/record.a.json`, `record.b.json`, `places.log` | Old tab/journal references, Find query/draft and place | `record_store.rs:3–17`, `:803–846`; `recto-session/src/record.rs:37–39` | Both slots must cease referring to old target history. Review target-related Find text; do not wipe other documents' query/history without agreed scope |
| `session.v<N>/`, `session.damaged[.N]/`, other known session flavors | Older-format records/journals; damaged records can carry old query text and references | `record_store.rs:406–515` | Enumerate known app-owned roots, inspect bounded ownership metadata, and sanitize/retire only verified target material. A new marker alone cannot control old binaries |
| `<Caches>/RectoPDF/corpus.noindex/<key>/<format>/app/` | Encrypted text pack and metadata, offsets, ready/refused state, in-progress output | `recto-index/src/corpus.rs:202–240`; `corpus/evict.rs:101–164` | Cancel builds, await exit, clear every verified old version and aliases, then verify absence. `clear` currently refuses a build-held lock; `clear_all` reports failures and is not a targeted purge |
| Corpus `agent/pages`, manifest, `.page.tmp`, retired agent folders | Plain Markdown page text and partially written output | `corpus.rs:223–239`; `corpus/agent_life.rs:1–22`, `:58–80`, `:108–140` | Stop issuing exports, release the session claim, remove affected live and retired folders. A normal next-launch sweep is bounded/best-effort and cannot stand for confirmed removal |
| Corpus `by-file/` alias/version table | Links from paths to old corpus keys | `corpus.rs:85–88`, alias helpers in that module | Enumerate all known versions; prove coverage. A capped alias list is not a complete historical provenance ledger |
| `<Caches>/RectoPDF/contents.noindex/` | Plain generated headings from removed text | `recto-index/src/generated.rs:216–285` | Cancel generation and await exit; clear affected path/version keys; reject any failed deletion. Path-tagged caches have the same rename/provenance question |
| `update/before/` and its temporary `.restoring` files | A clone/copy of session and journals, restored after a failed update | `recto-update/src/install.rs:601–679`; `record_store/sleep_files.rs:116–147` | Hold the installer coordination lease; sanitize affected entries or invalidate/replace that rollback snapshot under an agreed scope. Do not delete unrelated recovery state |
| Active render/page state, shared surfaces, previews, selection, Find, snapshot offers | Old page pixels/text or a pending clipboard SVG; late results can publish after a clear | `recto-app/src/core/snapshot_tool.rs`; `recto-reader/src/engine/`; `recto-app/src/sleeper.rs` | Increment a target/version epoch, cancel work and await acknowledgments outside UI/session locks; release target data and refuse old results. No physical RAM-erasure claim |
| Index/heading helper jobs and their workers | Read descriptors, decrypted text, the per-corpus key, a result which can rebuild a cleared cache | `recto-control/src/indexing.rs:358–367`, `:409–440`; `generating.rs:114–121`, `:353–359` | Cancellation alone is not quiescence. Require job-ended acknowledgment and no old-version publication before purging; another live session's claim is a blocker |
| Activity logs, update logs and failure evidence | Normally command metadata rather than PDF text; a search query can be private | `PRIVACY.md:34`, `recto-agent` logging policy; test evidence lives under `target/recto-evidence/` | Do not add document text, redaction text or rectangles to permanent logs. Audit new refusal strings. QA uses synthetic sensitive markers only, never personal data |
| User-created exports, print jobs/spool, clipboard already consumed, filesystem snapshots/backups, other processes' memory | Independent copies beyond the transaction | `PRIVACY.md:36,44,63`; existing export/printing contracts | Explicit exclusions; no silent deletion or assurance that they are erased |
| Future `.recto` libraries | Appended old text segments and structural facts | `docs/research/2026-11-14-locator.md:647–649`; PLAN 8.5 | A library must register a purge/compaction adapter before it can participate. Invalidating a segment's index does not remove its bytes |

Current retention is intentional for ordinary editing. These are prerequisites for a new operation, not newly discovered redaction leaks in an existing tool.

## 4. Identity, ownership and hostile storage

`journal_file_name` omits the device because a volume may return under another device number (`recto-session/src/record.rs:525–540`). `journal_is_for` is designed for restore, not erasure authority (`:602–615`). Backups use a canonical-path hash, corpora use a file identity plus sampled bytes, generated contents use a path tag, and kept pages are named by the replacement stamp. None alone establishes the complete derivative lineage needed here.

The removal plan therefore needs a verified `Origin` (source descriptor identity, complete version and actor-approved path), a target identity and a bounded set of artifact ownership proofs. New derivatives should register their origin at their creation/rename under each subsystem's writer. Migration must reconcile existing manifests/headers without loading PDFs or replaying histories just to decide ownership. A missing, unreadable, too-new or ambiguous owner yields `scope_incomplete`; it must not become an empty set.

Never equate another PDF with the target using its basename, page digest, content hash, size/time or inode alone across volumes. Identical copies can be independently edited or granted. An artifact shared with another history cannot simply be unlinked; either split/rebase its unaffected history without retaining removed material under reviewed rules, obtain the appropriate explicit scope, or refuse before commit. The first implementation supports only an independently complete scope; it does not invent a cross-alias cleanup rule.

Persistent transaction data is hostile input. It contains no PDF bytes, page text, title, removed strings, screenshots, password or key. It may need private bounded source/target paths, stamps, root-relative artifact identifiers, category bits and progress digests; those are sensitive metadata and must be private/excluded from backups. The decoder validates format, checksum, lengths, ordering, root kind, leaf-name grammar and every integer. A decoded string is never an unrestricted deletion path. Trusted adapters re-derive locations within configured roots and recheck a no-follow descriptor and expected identity immediately before mutation. Links, devices, directories or replaced files at an expected regular artifact name stop that step; never follow them or sweep a shared folder.

## 5. Commands and one execution path (proposed)

First the CLI, then window controls sending the same typed command and actor:

| Command/route | Proposed level and behavior |
|---|---|
| `remove-hidden --kinds metadata,attachments,actions` | Stages a typed, undoable removal draft on its named tab. Control for agents under existing owner-edits rules; protected-document reads refused. Inspection counts are untrusted document-derived facts under existing read policy |
| `remove-hidden --show` | Read-only inspection of draft/category counts, no PDF mutation |
| Normal `save` / `save-as --out ...` with a removal draft | One save executor builds an exact target-specific plan; returns `removal_confirmation_required` and a bounded opaque plan id, or a refusal. The window draws the Rust-owned confirmation |
| The same `save` / `save-as` with `--confirm-removal <id>` | Commits the exact plan. Full for agents, including a new-file Save As, since the operation intentionally ends history. A person confirms their own plan through the same command |
| `removal status` / `removal resume <id>` | Read-only status at the normal document read level; resume is the same destructive authority as commit. No output of removed material |

The confirmation binds actor, source version, target and target mode, category set/region plan, affected ownership set and plan digest. Any edit, file change, target change, scope change or actor change invalidates it. A token is **not** a security boundary against a hostile Full agent: the actor/level gate remains the authority. A person's confirmation cannot be borrowed by an agent. Plan ids live in a bounded session-owned compartment and expire by the injected clock; cold tabs gain no timer/thread/handle or per-tab plan buffer.

There is no second window-only save wrapper. UI menu ids, palette, CLI and future sidebar share the executor, refusal codes and policy walks. A tool's Apply may stage a draft, but the irreversible confirmation is for the concrete Save target and retention scope. Before it, Undo can remove the draft; after a successful commit, Undo cannot recover removed content. A failed commit must report any history already retired instead of pretending that it was rolled back.

The permanent command table additions and refusal strings are not part of the first pure primitive. Suggested refusal concepts: `scope_incomplete`, `removal_confirmation_required`, `removal_plan_changed`, `removal_busy`, `removal_recovery_required`, `removal_cleanup_failed`, `removal_target_uncertain`, `removal_unsupported`. Final identifiers follow the project's closed code table and registration guards.

## 6. Crash-safe transaction

This is one state machine. Effects are performed by a single carrier only after the preceding effect is acknowledged; tests inject ordering and I/O results. No file I/O, keychain lookup, helper wait or session-lock wait occurs on the UI thread. A compact session transition freezes the affected draft and passes an immutable snapshot to the save worker. A single active removal transaction initially avoids overlapping origin/purge/update conflicts; unrelated reading stays usable.

The persistent recovery barrier belongs under an app-owned `removal/` root **outside** `session/`, old-format side folders and `update/before/`, so rollback cannot replace it. The proposed name/schema is not final. It is durable and read back before any cleanup can begin. Every startup, restore and updater rollback in a removal-aware build must consult it before taking affected material into state. A broken or too-new barrier never becomes “nothing pending.”

| State | Durable evidence / effect | Crash or failure behavior |
|---|---|---|
| `Draft` | No retention changes; ordinary draft checkpoint only | Ordinary editing recovery; no removal guarantee yet |
| `Planning` | Check grants, descriptor identity, parser agreement, signature policy, target, provenance and bounds; form exact confirmation | Failure leaves PDF and retention unchanged; no silent capped inventory |
| `Confirmed` | Confirm actor/level and plan digest; reserve target and take subsystem/update coordination leases | If permission changed, target moved or scope changed, refuse before destructive work |
| `Quiescing` | Stop old-version indexing/generation/render/sleep/snapshot work; await exit/epoch acknowledgments | Timeout refuses; keep existing state. Do not delete caches while their writer can recreate them |
| `CandidateReady` | Rewrite to a private sibling candidate; verify its exact semantics and stamp; sync it | Candidate failure preserves original and history; no `keep_backup`/`keep_pages` call is permitted |
| `BarrierDurable` | Persist the complete verified candidate identity, trusted ownership plan and “cleanup not complete”; sync file+directory and read back | Cannot write/sync/read back barrier: no cleanup. Barrier corruption at restart blocks affected restore and asks for recovery |
| `Purging` | Retire each proven old artifact/reference with durable acknowledgments; update both record slots and applicable old/update copies; keep progress replayable | Failure keeps barrier and sanitized candidate, source unchanged; do not restore purged histories or make a new backup of them |
| `Purged` | Verify all listed artifacts absent/replaced and no old epoch can publish; durably record the point of no return | Only forward completion or an explicit recovery decision is permitted. “Cancel” cannot promise old undo/history back |
| `Replacing` | Recheck source/target descriptor/version under the writer's own atomic replacement checks; install candidate | An interruption is classified by exact candidate/target identities, never guessed from pathname, page count or mtime |
| `Replaced` | Read back target; acknowledge sanitized identity; update session to clean history with no kept-source references | If cleanup/state finalization fails, say sanitized target is present with recovery pending. Barrier blocks stale restore; no false “nothing written” |
| `Complete` | Verify target and retention once more, durably mark complete, publish events/view invalidation; release leases | Success only now. Retire barrier after durable clean state; never delete a pending barrier to make a warning disappear |

The existing `prepared` callback is a useful hook, but its current `Keeping`/`Saving` journal marker is **insufficient**: normal `carry_saved` continues undo history, and normal `prepared` may keep deleted pages (`recto-agent/src/saving.rs:434–445`; `record_store.rs:282–315`). Removal must carry a distinct typed retention intent all the way into the writer and recovery adapter. It must be impossible for the ordinary backup hook to run merely because an outer caller forgot to pass `None`.

Failing after `Purging` can leave less recovery history even though the source remains unchanged. That is an inherent multi-store transaction consequence; the confirmation must say history is cleared and an interrupted removal resumes forward. A design which promises both atomic rollback of all history and guaranteed absence of all copies at every crash point is not credible. Staging sensitive copies in a “trash” directory to roll back the purge would itself retain the material.

### Recovery and cancellation authority

Before `BarrierDurable`, cancellation abandons only the candidate/draft work. After it, a removal-aware launch blocks affected restore and resumes idempotent cleanup/replacement only when existing grants and the original actor's authorized scope still hold. Automatic recovery never upgrades an agent's level or executes an arbitrary manifest path. Read-only agents may inspect status, never clear a barrier. A person or an agent at Full may request resume; a person-only recovery choice may abandon an uncommitted transaction after being told history may already be cleared. A barrier can be retired only after a verified terminal state; there is no `force clear` command that makes stale journals readable again.

If the target is the expected candidate, finish forward. If the original is unchanged and the verified candidate remains, resume forward. If either identity is another file, or the candidate disappeared after purge began, retain the barrier and report exact recovery needs without overwriting anything. Recovery cannot recreate removed history from a backup or update copy.

Disk-full/read-only/media-offline behavior: no barrier allocation means no purge. Once the barrier exists, lack of space or an unsyncable directory leaves recovery pending. No progress bit is trusted unless its write was acknowledged. Reserve bounded journal space before purging, and make replay work with the last acknowledged state rather than requiring a new progress allocation after every unlink. A completed safe removal must not downgrade an unconfirmed durable write to an ordinary warning. Real power-loss evidence remains separate from injected failure tests.

### Older binaries and updates

Old RectoPDF releases do not understand a new barrier. It cannot alone prevent them from restoring `update/before/`, `session.v<N>/`, `.not-merged` or old checkpoints. Quiescence therefore includes updater coordination; the inventoried legacy restore paths must be made incapable of restoring the removed version **before replacement counts as committed**. Preserve unrelated records by a reviewed targeted rewrite, not by deleting the entire rollback snapshot. If a foreign schema cannot be transformed safely, refuse the operation until a migration/explicit scope decision supports it.

Test the signed release's real updater/rollback with the resulting state before shipping, using the existing carry harness. The first pure primitive does not claim cross-version recovery safety or modify update installation. Save As sources deliberately left on disk remain excluded, so an old app reading that named source is not something this barrier prevents.

## 7. Content transformation and the fork boundary

RectoPDF pins lopdf `e5f497163b364fae5217f9b1288ccacd39a9870a` in `Cargo.toml:32` and `Cargo.lock`. The fork checkout was at another commit during this read; the findings below come from `git show <pinned>:<path>`, not assumptions about its working tree. No fork file was edited, built or requested externally.

At that pin, `src/indexed_reader/rewrite/mod.rs` documents a new reachability-based file, old revisions/unreferenced objects omitted, raw live streams copied in 64 KiB chunks, and explicit signature/encryption refusals. `RewriteChanges` has `allocate`, `add_object`, `write_object`, `write_stream_from`, `set_root` and `set_info` (`:540–641`). It deliberately preserves reachable embedded files, XMP, Info, JavaScript and document parts (`:71–74`); its generic rewrite is not a hidden-data remover. `set_info` accepts a reference or dictionary, **not** `None`/Null: an empty dictionary can clear values but leaves an empty Info entry. A literal requirement to omit the entry needs a small API extension. `remove_object` exists but RectoPDF's `clippy.toml` forbids it: dropping every reference to an arbitrary object is not a safe content-removal implementation.

### First selected categories: narrow, inspectable, fail closed

- **Metadata:** replace/remove Info values, document/page/object XMP, selected private data and thumbnails under a written key/type policy. Count each affected container/object once even if shared. Titles/outlines, accessible alternate text, form values and annotation comments are not implicitly “metadata”; enumerate them separately or say they remain. Image EXIF/XMP inside live encoded images is a separate bounded decoder/rewriter problem and must remain reported unsupported until implemented.
- **Attachments:** remove EmbeddedFiles name-tree entries, associated-file (`/AF`) references and file-attachment annotations/appearances under an explicit policy, prune all alternate references, and let reachability omit their streams. A payload shared with still-visible page content cannot be treated as an isolated attachment. Unreadable objects, references past the budget or an unknown attachment representation refuse the selected guarantee; never return a successful count after skipping them.
- **Actions:** enumerate exactly which action entry points go (`/OpenAction`, `/AA`, annotation/bookmark/field actions and document-level JavaScript names). The default category should say whether external links/GoTo navigation remain; removing useful links is a product choice. Dropping script actions is not “all hidden content removed.”

Each category's draft reports counts and an explicit unsupported/remaining list. The output uses only lopdf's atomic rewrite, with RectoPDF's policy creating sanitized dictionary/stream replacements. No PDFium object edit or save API is used. A whole reachable-object traversal must have reader/work/retained-object limits and a complete visited set; finishing an incomplete traversal is not success. The existing save's `objects_missing` and `resources_left_whole` warnings are inadequate for a removal proof: an unverified selected category is refused, while unrelated fidelity warnings still need a product decision.

### Region redaction is not built by these primitives

Drawing a rectangle, setting a Redact annotation or clipping an SVG only conceals content. Genuine removal must address text operators/font decoding, inline and XObject images, nested/shared forms, soft masks/transparency, marked content/ActualText/Alt, annotations/appearances, optional content, attachments and historical streams. A shared image cropped on one page can retain every original pixel if reused by another page. The existing hidden-text detector is a copy/display heuristic, not a permanent-removal proof.

Two possible later strategies remain unselected:

1. A bounded, fail-closed content editor preserves supported vector/text semantics, replaces affected objects/streams and refuses unsupported constructs. It needs a separate design and a real semantic corpus before a user-facing redact guarantee.
2. A labelled sanitized raster copy creates new page images from the sandboxed renderer, overwrites selected pixels before encoding, embeds only those resulting images through lopdf, and carries no original streams/text/metadata. This can provide a simpler logical-removal boundary but loses searchability, accessibility, vectors and original-image fidelity; all-page rendering and output/DPI limits need explicit product agreement and measurement. It is not silently chosen here.

### Fork request draft, retained here for review

**No request is sent by this document.** If the coordinator approves an inbox request, include the pin, RectoPDF callers `recto-core/src/save.rs:427–454` and `recto-agent/src/saving.rs:434–445`, and these precise needs:

1. A way to omit trailer Info explicitly, distinct from no replacement/empty Info; tests with direct and indirect Info. Existing `set_info(Object)` cannot express omission. This is needed only if the agreed metadata contract requires the entry absent rather than its values gone.
2. A removal-safe rewrite verification mode which fails when selected required structure cannot be examined, while preserving the existing ordinary-save warning behavior. First ask whether callers can derive a complete proof from current reports; do not request a global behavior change without evidence.
3. Advice on bounded content-stream transformation/region-redaction building blocks already available or planned, with exact non-goals. Do not ask the fork to “support redaction” without semantic rules, shared-resource cases, caps and independent extraction tests. The existing writer can accept sanitized replacement objects; it is the proof-producing transformation which is missing.
4. Keep atomic pre-replacement preparation and source/target identity checks usable for a retention coordinator; clarify durable outcome reporting after rename. Do not add a second PDF writer or an irreversible callback after a rename without a recovery contract.

Items 2–4 are questions/design inputs, not demonstrated missing APIs. Current dictionary replacement is enough to start a narrowly specified category spike once retention and category semantics are reviewed. The older object-amplification request already exists; do not duplicate or build it here.

## 8. Compartment and bounded-resource proposal

The immediate slice is a pure `recto-core::removal` module (proposed `crates/recto-core/src/removal/{mod.rs,plan.rs,transaction.rs,tests.rs}` plus a one-line registration; as built, see D1a under §10: `removal.rs` with `removal/{plan,progress,tests}.rs`). It models semantic phases, fixed root/role kinds, proof completeness, bounded inventory and acknowledgments. It performs no I/O and has no arbitrary path-deletion capability. The locator worker currently owns `recto-core/src/lib.rs`; registration must wait for coordination or be integrated by the coordinator. This document is the only file currently authorized on this branch.

Later adapters live in their features: `recto-agent::removal` for actor/confirmation and one save route, `recto-control::removal_store` for the private barrier and store adapters, feature-local cancel/epoch acknowledgments in index/generation/reader/sleep, and a small updater adapter. Do not put category rules or purge loops in Session, the bridge, a menu hub or the Swift window. Existing save code needs a typed retention policy extension point before adding a removal mode; registration alone cannot smuggle new logic into its already large function.

Proposed initial bounds are **design limits, not measured performance**: one active transaction; at most eight confirmation plans per session; 8,192 artifact identifiers and 2 MiB total encoded inventory/barrier data; 4,096 bytes per actor-approved path; batches of at most 128 artifact checks; at most four simultaneously open artifact descriptors. A complete plan exceeding a limit refuses before `BarrierDurable`. No truncation, eviction of an active recovery barrier or per-idle-tab allocation/thread/handle is allowed. Pending barriers have a small global cap; at that cap new removals refuse while existing recovery remains readable.

The pure plan must document/assert its actual struct sizes when implemented. A transaction's added memory target is bounded inventory/state overhead, not the renderer's entire process tree. Existing 1 MiB save-object input bound, 256 MiB held-rewrite cap and 16 million copy-work allowance remain in force; object amplification is still a dependency risk. Content work starts at most one helper/worker chain, with existing limits, and no helper starts for validation that finds nothing to remove. No permanent background polling is introduced.

For implementation hand-back, measure actual whole-process/tree `phys_footprint`, write bytes, threads/processes/descriptors and input-to-frame before/after on an ordinary document and the heavy map; include a no-op category request. None is measured by this design. A long file read can be cancelled/limited off the UI thread; no on-disk deletion on a user/network path is made synchronous just because it is called “cleanup.”

## 9. Tests and QA built with each slice

Use the cheapest tier that can observe the failure. Pure state and policy tests run in process; no new app-launch test. Every new test names a real failure and is seen failing with its bug seeded. Small injected limits test cap-plus-one; injected clocks and acknowledgments test order, with no sleeps/default timeout waits.

| Layer | Required failure cases |
|---|---|
| Pure plan/state | A missing proof becomes an empty purge; a foreign/shared artifact gets deleted; a success arrives before target+purge acknowledgments; a cap truncates inventory; stale plan/actor/epoch accepted; replacement identity uncertain; pending barrier can be “cleared” to restore old state |
| Barrier parser/store | Corrupt/too-new/truncated/checksummed hostile input, traversal/absolute path, symlink/fifo/hardlink substitution, read-only/full disk and missing directory; no allocation before bounded frame data; replay twice is idempotent; no document text in encoded barrier |
| Save/recovery adapters | At each edge: stop/kill before barrier, each purge acknowledgment, pre-rename, post-rename and post-state write. Original or sanitized target classified correctly; old history/cache cannot reappear; failure never says “nothing written” after rename; existing unrelated state survives |
| Quiescence | Held helper/sleep/render result lands after purge request, old worker holds source, another session owns an agent-folder claim, updater tries snapshot/restore concurrently; stale result is refused and no success while a writer remains |
| PDF categories | Unique sensitive markers in every selected store; reachable alias/shared object, direct/indirect Info, nested name tree, actions in fields/annotations, selected image metadata unsupported; malformed/over-budget object refuses; no old revision or unused stream survives |
| Region redaction later | Glyphs/forms/shared/cropped images/OCR/ActualText/Alt/annotations/OCG/transparency; independent text/image/metadata extraction plus visual fidelity outside the region. A black box covering an intact source must fail the proof |
| Actor/policy walks | Every command and confirmation route at Off/Read-only/Control/Full; person token sent by agent; protected source/added page; owner's unsaved history in an affected alias; quiet-run outside paths; Full never signs or changes a signed original |
| Existing release carry | Signed current-to-prior-to-current flow cannot restore removed material from any retained app-managed state. Old-version code ignoring the barrier is specifically exercised; reusing only a new-build fake would miss it |

QA vocabulary is feature-local and uses the same command/state machine: draft/category state, plan completeness/counts, transaction phase/pending reason, injected hold at each effect boundary, and artifact outcome counts. QA must not expose removed strings, private retained-file paths or keys at a lower level. Light/dark real-app screenshots demonstrate the draft, confirmation, completion and recovery-pending states after the relevant UI slice; they are not a substitute for extraction/recovery proof.

## 10. Delivery slices and stop conditions

| Slice | Deliverable / acceptance | Dependencies and allowed scope |
|---|---|---|
| D0, this branch | Reviewed inventory, scope, state machine, exact unresolved choices and this request draft | Documentation only; no deletion/content code or fork request |
| D1, first code proposal | Bounded pure retention plan and transaction state machine; no I/O, no command, no per-tab state; transition/cap/authority tests | Independent design review and coordinator file-scope approval; resolve overlap with locator registration first |
| D2 | Provenance ledger/adoption design and exact per-store quiescence/purge adapters, each independently tested | Resolve shared-alias and legacy/foreign-format handling; cannot ship success while completeness is unknown |
| D3 | Durable barrier/store and startup/updater recovery enforcement, with interruption matrix | D1/D2, durability decision, narrow destructive scope; feature modules and minimal registrations scoped in new briefs |
| D4 | Explicit metadata/attachment/action category inspection and staged draft | Category policy chosen; prototype current fork replacements first; no blanket “all hidden data” claim |
| D5 | One Save executor commit with no-backup/no-kept-history policy, confirmation, CLI and feature-local QA | D2–D4, cleanup+output verification and policy walks, reviewed interoperability matrix |
| D6 | Window controls/confirmation/recovery presentation and existing launch QA integration | Same commands as D5, screenshots light/dark, input-to-frame/resource evidence |
| D7 | Region redaction | Separate semantic/strategy approval and proof-producing content implementation; not included implicitly in D4 |

**D1a as built.** The pure plan and the progress tracker are in `crates/recto-core/src/removal.rs` and `removal/{plan,progress,tests}.rs`, not the names §8 proposed. Departures from this design:
- There is no Draft or Planning phase in code: a plan is built whole or refused.
- `Progress` was tied to a plan by its length only; D1b binds it by digest (below).
- Find text is cleared unconditionally. It is the app's one global last query, so this may wipe another document's history, which §3 says not to do without an agreed scope. **Decided by the owner, 2026-11-25** (`OWNER-RULES.md`): a completed removal wipes the remembered Find search; the unconditional clear is the intended behaviour. Also decided the same day: "actions" keeps ordinary links, and true redaction starts as picture pages.
- The actor and epoch staleness tests belong to D1b.
- Snapshot and clipboard offers are not yet a `Writer`; that is a follow-up.

**D1b as built.** `removal/digest.rs` and `removal/confirm.rs` (with `confirm/tests.rs`). Pure, no I/O, nothing per tab.
- `Plan::digest()` is a SHA-256 over the request, every step, every artifact id and the 22 report lines, in a fixed little-endian encoding with explicit code tables (never `Debug`); computed once in `plan`. A golden test pins it.
- `Confirmations<A: Copy + Eq>` is owned by the caller, holds at most 8 plans (design limit, not a measurement) for 10 minutes against a clock passed in, and allocates nothing until the first offer. A new plan for the same (actor, target) replaces the old one; when full the asker's own oldest goes, and an asker with none gets `Full`, so no actor pushes out another's plan. Ids count from 1 and are never reused. Paths are kept only as a hash (`TargetKey`). Core compares actors for equality and states no rule about who may do what; D5 supplies the actor type.
- `take` checks in this order: unknown id, other actor (nothing changes, nothing else revealed), expired, other target, stale `Version { epoch, FileStamp }`, plan changed; each refusal after the actor check removes the entry. Success returns a `Confirmed`, which is not `Clone` and can be made only there.
- `Progress::start(plan, Confirmed)` is the only way to begin a walk; `Progress` keeps the digest, and `resume`, `next`, `acknowledge`, `outcome` and `succeeded` refuse another plan (`WrongPlan`) even of equal length. `outcome` now returns a `Result`.

Do not proceed to real deletion because D1 passes. It proves ordering and bounds only. Missing provenance, unsupported content semantics, missing quiescence acknowledgments, inability to durably preserve the barrier, unsafe old-version restore or signed/protected-file refusal each blocks the affected operation. These blocks do not prevent locator work, copy fixes, the narrow metadata investigation or unrelated reading.
