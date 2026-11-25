# References: `recto ref` and `recto resolve`

> Part of the engineering notes: [`README.md`](README.md). The core is `crates/recto-core/src/locator.rs` (design: `docs/design/locator-core.md`).

A reference is a short token that names a page by its content as well as its number, so a page can be found again after another program added, removed or moved pages. `recto ref <page>` makes one, and `recto resolve <ref>` says where that page is now. Both are read-only (`Level::ReadOnly`), run on one executor (`crates/recto-agent/src/locating.rs`), and are registered by `crates/recto-agent/src/commands/locating.rs`. The window has no control for them: there is no person's path to a reference yet.

**What the answer is.** `resolve` always answers with a status and exits 0: `found` where the reference said, `moved` when the file's only page with that content is somewhere else, `ambiguous` when several have it (the candidates are listed, never the nearest), `gone` when every page was read and none has it, and `incomplete` when a cap or an unreadable page stopped the search, which never says gone. Document tags are information beside the status (`document`: `same`, `different`, `unknown`) and never decide it. A hinted page with the reference's own digest is `found` without reading the rest, whatever the file's version: identical page content is the same page.

**Refusals** are for what can't be answered at all: `bad_reference` (exit 2) for a token that isn't one, refused when the line is read, before the level or the tab is asked, in constant words that never repeat it; `reference_too_new` (exit 2) for a version this build doesn't know; `not_referenceable` (exit 1) for a renderer-only file, a page with no identity, a document with no tag, or a page added from another PDF and not yet saved; and the existing `no_such_page`, `no_such_tab`, `file_unreadable`, `file_changed` (the file changed while it was read), `source_changed` (the tab's edits are about another version of the file, or its pages are withheld: as `text` and `contents` refuse) and the protected-document refusal.

**Unsaved edits.** A reference names a page of the tab's own file. `ref N` counts pages as the tab shows them and finds the file page standing there; `resolve` answers `file_page` (of the file on disk) and `page` (its place in the tab now), which is null when unsaved edits removed it. A spot's rectangle is in the file's shown frame, before any unsaved turn, and the answer's `note` says so.

**Cost and safety.** Nothing is kept: no state, cache, thread, event or per-tab field. The session lock is held only inside `tab_file`; the file is then opened by its own descriptor (`recto_session::identify::Opened`), read under `ScanCaps::STANDARD` (1 GiB of page reads, at most 200,000 pages), and its stamp is checked again after. The token is never logged: `log_details` holds the tab and the page number the token hints at.

**Measured** (`crates/recto-core/examples/locator_scan.rs`, release build, a reference whose page is gone, so every page is read):

2026-11-25, load about 6.1 before and after (other workers building and testing beside it), a single run each, so an estimate and not a distribution. Scan cap 1 GiB.

| File | Size | Pages | Open | Resolve | Pages read | Bytes read | Answer |
|---|---|---|---|---|---|---|---|
| `gov_nasa_ntrs20240002628.pdf`, the most pages in bench100 | 97 MiB | 383 | 12 ms | 810 ms | 383 | 277,389,770 | `incomplete`: every page was read, but four pages (zero-based 127, 137, 156, 197) have no identity under `page_id::Caps::STANDARD` |
| `gov_usgs_usgs70277338.pdf`, the 330 MiB map | 330 MiB | 1 | 4 ms | 191 ms | 1 | 119,948,577 | `incomplete` (its one page came back with no identity); `reference` to it is `NoIdentity` |

Both are far under 10 s. Peak `phys_footprint` of the whole process for one scan (this run, release build, load about 6.1, one run each): 6.2 MiB on both, since the file is mapped and not copied; the same 6.2 MiB as at the 256 MiB cap. The scan is bounded by `ScanCaps` (1 GiB of reads, which bounds time, not memory), not by memory.

## Known limits

- On the 383-page NASA report (97 MiB) a page that is really gone still answers `incomplete`, and the scan cap is no longer the reason. At 256 MiB the scan stopped 24 pages short, because a stream a page draws is charged each time it is drawn (a 97 MiB file costs 277 MB to read through). At 1 GiB it reads all 383 pages (277,389,770 bytes), and four of them (zero-based 127, 137, 156, 197; charged 634,024, 200,868, 384,324 and 166,147 bytes) come back from `page_id::read_page` with no identity under `page_id::Caps::STANDARD`. Why is not known and is a `page_id` question (a per-page or per-stream cap, or a filter it can't read). `gone` is reachable only where every page has an identity.
- The 330 MiB USGS map has one page, and that page has no identity: resolve answers `incomplete` after 119,948,577 bytes and `reference` to it is `NoIdentity`, so it can't be referenced with certainty.
- `resolve`'s `page` is as the tab shows it, which may be the old bytes before the app has looked, the same as `text`.
- Both identity gaps go to the plan as follow-ups; nothing in `page_id` was changed for them.

## Tests

`crates/recto-agent/src/commands/locating.rs` (the token's refusals, its example, its log) and `crates/recto-app/src/core/headless/locating.rs` (moved, gone, ambiguous, unsaved edits, a changed file, the owner's edits untouched at every level, a protected document, the CLI's words).
