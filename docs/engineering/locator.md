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

- **2026-11-25 (`build/page-id-caps`):** the names cap went from 256 to 8,192, so the 383-page NASA report now answers `gone` for a page that is really gone: all 383 pages read, 295,938,755 bytes, 668 ms resolve, 10 ms open, peak `phys_footprint` 1.87 MB (`/usr/bin/time -l`, release build, load about 13, one run, so an estimate). Before, four pages (zero-based 127, 137, 156, 197) had no identity because they draw more than 256 images. Eleven pages of bench100 gained an identity, and no other digest changed. At 256 MiB the scan had stopped 24 pages short, because a stream a page draws is charged each time it is drawn.
- The 330 MiB USGS map has one page, and that page still has no identity (it reaches `page_bytes`, correct under rule 1; three other bench100 pages are the same): resolve answers `incomplete` after 135,979,278 bytes (119,948,577 before the cap rose: it now does more work before it is refused) and `reference` to it is `NoIdentity`, so it can't be referenced with certainty.
- `resolve`'s `page` is as the tab shows it, which may be the old bytes before the app has looked, the same as `text`.
- Both identity gaps go to the plan as follow-ups; nothing in `page_id` was changed for them.

## Tests

`crates/recto-agent/src/commands/locating.rs` (the token's refusals, its example, its log) and `crates/recto-app/src/core/headless/locating.rs` (moved, gone, ambiguous, unsaved edits, a changed file, the owner's edits untouched at every level, a protected document, the CLI's words).

## `--ref` on `text` and `render`

`recto text --ref <ref>` and `recto render --ref <ref> --out <file>` read the page a reference points at now, in place of a page number. They resolve through the same resolver as `resolve` (`crates/recto-agent/src/locating.rs`, `page_of`, `ScanCaps::STANDARD`) and then read through the same `locate` path as a page number, so the levels (`text` Read-only, `render` Full), `DRAWS`, `READS_PAGES`, the protected-document refusal and `render`'s file rules are the commands' own and are not repeated for `--ref`.

**Page or `--ref`, not both.** The line carries `page` or `ref`; both or neither is `usage` (exit 2) in the command's own words. A token that can't be a reference is refused as the line is read (`bad_reference`, or `reference_too_new`), before the level or the tab is asked, and its words never repeat it. A reference's spot or characters don't narrow `text` or `render`: they read the whole page.

**Only a page gives a read.** `found` and `moved` give a page. `gone`, `ambiguous`, `incomplete`, and a page that unsaved edits removed from the tab are each refused with their own constant words as `reference_unresolved` (exit 3, not found), and nothing is drawn or told to the host as drawn (this is about the unresolved case only: once a page is read, the draw happened and is told, and a refusal after it, for a changed file, doesn't undo that). The next step says `recto resolve` shows where the reference points and lists any candidates. A `document: different` answer still reads; the answer says so.

**The answer** carries `ref: { status, was_page, file_page, document }` (no token), and `page` is the page's place in the tab as it was found. The tab is settled once, where the reference is found (`--tab` is resolved there, and the active tab moving later changes nothing, as for a page number). Before the read, the page the tab gives for that place must be the page that was found (`agree`): if a page added from another PDF or another page of the file stands there, the refusal is `file_changed`, "The tab changed while the reference was being found. Try again." (the only place those words are used). The file's stamp is checked after the read (`still_located`): if the file changed since the page was found, the refusal is `file_changed`, "The file changed while it was being read. Try again." For `render`, the picture this command wrote is taken away when that refusal is made (exactly `--out`, which it created), so a retry isn't `output_exists`. The session lock is held only where `resolve` holds it (inside `tab_file` and the page lookup), never during the scan. The activity log holds `ref_page` (the page the token hints at), never the token.

**Not yet:** `goto`, `highlight`, `select`, `copy`, `snapshot`, `export svg` and `images` take a page only; each follows this pattern when it is given the flag.
