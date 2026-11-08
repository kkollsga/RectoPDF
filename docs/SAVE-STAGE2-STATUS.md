# Save stage 2 (the streaming rewrite): status when parked

Parked on 2026-11-08 by the owner's decision ("Lets release after phase 3. the rest we tackle after"). Branch `build/save-rewrite`, from main `d27a0e7`, in `.worktrees/save-rewrite`. Not for merging until stage 2 resumes. Merge main into it first, as main moved on.

## State of the branch

Every commit builds. `tools/check.sh --quick` passed on `c8ea31a`. The commits after it ran only their named tests: `an_added_pages_form_fields_can_be_filled`, `irs_forms_added_to_irs_forms_stay_fillable` and `a_page_from_an_added_pdf_that_changed_isnt_drawn`. **No `--full` run, no app build and no screenshots** on this branch.

## Done

- **Core (`recto_core::save`).** `Plan::write` goes through a `Sink`, either an appended update (`AppendSink`) or the rewrite's changes (`RewriteSink`), so the planner, copier and remapper are shared.
  - A delete is a rewrite: `remove_page` for each removed page.
  - The file's own tree stays when the kept pages are in order; otherwise there's a new tree.
  - Moves, turns, added pages and deletes go in one rewrite.
  - Copied streams are opened lazily (`LazyStream`), so a million copies hold no reader each.
  - What a rewrite holds is bounded by `MAX_REWRITE_HELD_BYTES`, 256 MiB.
  - `RewriteError` maps to `SaveError`.
- **Signatures (SPEC decision 27).**
  - `SignatureFacts::signed` says whether a signature field has a value, with `/FT` inherited. `has_persons_signature()` covers signed, certified or locked.
  - A rewrite over a file with a person's signature is refused with `signed_needs_copy`.
  - Save As uses `RewritePolicy::remove_signatures()`. So does any copy of a certified or locked file, edited or not.
  - Usage rights alone get `remove_usage_rights()` in place. Appends still drop UR3 in the new revision, as stage 1 did.
- **Outcome and answer.** `SaveOutcome` has `pages_removed`, `signatures_removed`, `document_data_kept` (as `DocumentData`, with `kinds()` in words) and `resources_left_whole`. `objects_missing` includes the rewrite's.
  - `recto save`'s answer carries these and adds notes: "The new copy won't be signed. The original keeps its signature." (`recto_agent::saving::UNSIGNED_COPY`), a warning for objects missing, the document data kept when pages were removed, and resources left whole.
- **Undo past a delete keeps only the needed pages.**
  - `recto_core::save::keep_pages` writes those pages through the rewrite; it syncs the copy and its folder.
  - `RecordStore::keep_pages_of(path, target, &pages)` replaces the whole-file clone: private, mtime set, fsynced again, at most 64 MiB.
  - Kept copies are named `kept-*.keptpages`, so Spotlight has no importer for them. `mdimport -t` says "with no plugIn", against PDF.mdimporter for a `.pdf`. `is_kept_name` still accepts the old `.pdf` names.
  - `AddedSource::holds` and `AddedRecord::holds` record which old pages a copy holds. `rebase_on_saved` moves Restore steps onto the copy's numbers; a page the copy doesn't hold rolls off.
  - `PageEdits::own_pages_needed` and `Session::own_pages_needed` supply the page list. `SaveSupport::keep_pages`, implemented by the app's `AppSaving`, makes the copy, from the agent's `Hooks::prepared`, before the rename.
- **App (Rust decides).**
  - `Session::needs_rewrite`.
  - `save_needs_copy(facts, needs_rewrite)` turns Save off: `ReaderView::save_enabled`, `can_save`, and a tooltip saying why.
  - Save As's note is `UNSIGNED_COPY`.
  - Signature refusals offer Save As.
  - `SaveUi` caches `SignatureFacts` per version.
  - Swift: `saveButton.isEnabled = reader.saveEnabled`.
- **Form fields of added pages (IRS).**
  - Widgets keep their field chain, and a field's kids are filtered to added pages.
  - Top fields join `/AcroForm /Fields`; a clash is renamed `name_2`.
  - The source's `/DR /Font` merges into the target's form.
  - Signature fields aren't carried.
  - Corpus check: every IRS form in bench100 with the next one's pages added passes `qpdf --check` with no unreachable widget. Without the change, the first pair fails.
- **A changed added PDF isn't drawn.** `PageMap::expected` holds each added file's stamp; the engine stats before each job and puts `ADDED_FILE_CHANGED` on the page.
- **Tests.** `never_in_pdf.rs`'s delete is on the real path, with the kept copy holding only the removed page. There are core tests for each case above, and CLI tests `a_delete_is_saved_by_rewriting` and `a_signed_pdfs_delete_goes_to_an_unsigned_copy`.

## Not done

- **Fuzz target** for the rewrite plan: a `rewrite_in_memory` beside `plan_in_memory`, through `write_unverified_to(std::io::sink())`, added to `recto-fuzz`'s `pdf.rs` and `save_copy`.
- **Measurements:** a delete on the 346 MiB map and on a 1,000-page file, giving time, peak whole-process `phys_footprint` and bytes written, under load.
- **QA verbs, app tests and screenshots (light and dark):**
  - a delete saved in the app, then undone past the save, through `qa button save`;
  - a signed file with Save off and the icon's tooltip;
  - Save As's sheet note, which a quiet shot draws blank, so check `qa state`'s `sheet.message`;
  - the certified alert's Save As button.
  - An app test that the kept copy is made through `AppSaving` and that undo draws from it.
- **Redaction items** (crash leaves no history or kept file; a redaction's save keeps nothing). Redaction isn't built (Phase 5), and nothing in the product reaches `ClearReason::Redaction` today. `Hooks::prepared` has a comment where that rule goes.
- **Docs:** PLAN ticks for the stage 2 items, `docs/ENGINEERING.md`'s Save section, and SPEC §6's "Saving" line. None are updated yet.

## Findings for whoever resumes

- **Page counts.** The fork's verify counts pages from the output, so a new tree with added pages passes it.
- **Signature removal and caller objects.** The fork's planning reads caller objects before the source (`load`: edits, then caller objects, then source). A replaced root `/Pages` or catalog therefore composes with its removal edits. Never use `remove_object`; `clippy.toml` refuses it.
- **Removing every page.** `NoPagesLeft` fires when every original page is removed, even with added pages kept. The message says so.
- **Replacement by rewrite.** `rewrite_atomically` runs `prepare_replacement` before its own verify, so the hook's `check_pages` and journal note come first, as with append.
- **Agent levels.** A rewrite needs Full at the agent level, as before. Control is refused with exit 5.
- **Headless sessions keep no copy.** A headless `recto serve` session (`BackupsOnly`) keeps no copy, so undo past a delete rolls off there. Only the app's record store keeps one.
- **Approval signatures and turns.** An approval-signed file with only turns still appends, and keeps its signature. Only certified or locked files, or a rewrite, go to an unsigned copy.
- **Save As of a signed file.** An unedited Save As of a certified file now makes an unsigned copy. Stage 1 refused it.
