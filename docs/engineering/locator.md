# References: `recto ref` and `recto resolve`

> Part of the engineering notes: [`README.md`](README.md). The core is `crates/recto-core/src/locator.rs` (design: `docs/design/locator-core.md`).

A reference is a short token that names a page by its content as well as its number, so a page can be found again after another program added, removed or moved pages. `recto ref <page>` makes one, and `recto resolve <ref>` says where that page is now. Both are read-only (`Level::ReadOnly`), run on one executor (`crates/recto-agent/src/locating.rs`), and are registered by `crates/recto-agent/src/commands/locating.rs`. The window has no control for them: there is no person's path to a reference yet.

**What the answer is.** `resolve` always answers with a status and exits 0: `found` where the reference said, `moved` when the file's only page with that content is somewhere else, `ambiguous` when several have it (the candidates are listed, never the nearest), `gone` when every page was read and none has it, and `incomplete` when a cap or an unreadable page stopped the search, which never says gone. Document tags are information beside the status (`document`: `same`, `different`, `unknown`) and never decide it. A hinted page with the reference's own digest is `found` without reading the rest, whatever the file's version: identical page content is the same page.

**Refusals** are for what can't be answered at all: `bad_reference` (exit 2) for a token that isn't one, refused when the line is read, before the level or the tab is asked, in constant words that never repeat it; `reference_too_new` (exit 2) for a version this build doesn't know; `not_referenceable` (exit 1) for a renderer-only file, a page with no identity, a document with no tag, or a page added from another PDF and not yet saved; and the existing `no_such_page`, `no_such_tab`, `file_unreadable`, `file_changed` (the file changed while it was read), `source_changed` (the tab's edits are about another version of the file, or its pages are withheld: as `text` and `contents` refuse) and the protected-document refusal.

**Unsaved edits.** A reference names a page of the tab's own file. `ref N` counts pages as the tab shows them and finds the file page standing there; `resolve` answers `file_page` (of the file on disk) and `page` (its place in the tab now), which is null when unsaved edits removed it. A spot's rectangle is in the file's shown frame, before any unsaved turn, and the answer's `note` says so.

**Cost and safety.** Nothing is kept: no state, cache, thread, event or per-tab field. The session lock is held only inside `tab_file`; the file is then opened by its own descriptor (`recto_session::identify::Opened`), read under `ScanCaps::STANDARD` (256 MiB of page reads, at most 200,000 pages), and its stamp is checked again after. The token is never logged: `log_details` holds the tab and the page number the token hints at.

**Measured** (`crates/recto-core/examples/locator_scan.rs`, release build, a reference whose page is gone, so every page is read):

2026-11-25, load 6 to 8 (other workers building and testing beside it), a single run each, so an estimate and not a distribution.

| File | Size | Pages | Open | Resolve | Pages read | Bytes read | Answer |
|---|---|---|---|---|---|---|---|
| `gov_nasa_ntrs20240002628.pdf`, the most pages in bench100 | 97 MiB | 383 | 7 ms | 1,027 ms | 359 | 269,070,134 | `incomplete` (stopped at 256 MiB of reads, 24 pages short) |
| `gov_usgs_usgs70277338.pdf`, the 330 MiB map | 330 MiB | 1 | 1 ms | 225 ms | 1 | 119,948,577 | `incomplete` (its one page came back with no identity) |

Both are far under 10 s. Peak `phys_footprint` of the whole process for one scan (review 2, 2026-11-25, release build, load 4 to 6, two runs of the NASA report and one of the map): 6.2 MiB on the NASA report and 4.0 MiB on the map, since the file is mapped and not copied. The scan is bounded by `ScanCaps` (256 MiB of reads), not by memory.

## Known limits

- On the 383-page NASA report (97 MiB) a page that is really gone answers `incomplete`, not `gone`: the scan read 359 pages and 269,070,134 bytes and stopped at the 256 MiB cap, 24 pages short. A page read is charged more bytes than the file holds; why was not looked into. `gone` is only reachable where the whole identity read fits in 256 MiB.
- The 330 MiB USGS map has one page, and that page has no identity (resolve answered `incomplete` after 119,948,577 bytes), so it can't be referenced with certainty; a `ref` to it would probably be `not_referenceable` (not tried).
- `resolve`'s `page` is as the tab shows it, which may be the old bytes before the app has looked, the same as `text`.
- Both go to the plan as follow-ups; nothing was changed for them.

## Tests

`crates/recto-agent/src/commands/locating.rs` (the token's refusals, its example, its log) and `crates/recto-app/src/core/headless/locating.rs` (moved, gone, ambiguous, unsaved edits, a changed file, the owner's edits untouched at every level, a protected document, the CLI's words).

## `--ref` on `text` and `render`

`recto text --ref <ref>` and `recto render --ref <ref> --out <file>` read the page a reference points at now, in place of a page number. They resolve through the same resolver as `resolve` (`crates/recto-agent/src/locating.rs`, `page_of`, `ScanCaps::STANDARD`) and then read through the same `locate` path as a page number, so the levels (`text` Read-only, `render` Full), `DRAWS`, `READS_PAGES`, the protected-document refusal and `render`'s file rules are the commands' own and are not repeated for `--ref`.

**Page or `--ref`, not both.** The line carries `page` or `ref`; both or neither is `usage` (exit 2) in the command's own words. A token that can't be a reference is refused as the line is read (`bad_reference`, or `reference_too_new`), before the level or the tab is asked, and its words never repeat it. A reference's spot or characters don't narrow `text` or `render`: they read the whole page.

**Only a page gives a read.** `found` and `moved` give a page. `gone`, `ambiguous`, `incomplete`, and a page that unsaved edits removed from the tab are each refused with their own constant words as `reference_unresolved` (exit 3, not found), and nothing is drawn or told to the host as drawn. The next step says `recto resolve` shows where the reference points and lists any candidates. A `document: different` answer still reads; the answer says so.

**The answer** carries `ref: { status, was_page, file_page, document }` (no token), and `page` is the page's place in the tab as it was found. The file's stamp is checked after the read: if the file or the tab changed between finding the page and reading it, the refusal is `file_changed`, "The tab changed while the reference was being found. Try again." The session lock is held only where `resolve` holds it (inside `tab_file` and the page lookup), never during the scan. The activity log holds `ref_page` (the page the token hints at), never the token.

**Not yet:** `goto`, `highlight`, `select`, `copy`, `snapshot`, `export svg` and `images` take a page only; each follows this pattern when it is given the flag.
