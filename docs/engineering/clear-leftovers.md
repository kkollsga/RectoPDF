# Clear Leftovers, and the sheet over the window

> Part of the engineering notes: [`README.md`](README.md). Design: `docs/design/removal.md` §10 (D5 and D6).

**One action.** Edit > Clear Leftovers… is not a feature of its own: it sends the person's `removal check`, the same command `recto removal check` is. The yes sends the person's `removal clear --confirm <plan>`. An agent at Full does it with `recto removal check` and `recto removal clear`, and sees the window's side through `recto qa state` (`sheet`) and `recto qa removal-state` (`pending`, `clearing`, `held`, `sheet`: `none`, `offer`, `looked` or `told`, and `ready`).

**Where it lives.**
- `recto-control` `removal_store/leftovers/words.rs`: every word, the window's included (`MENU_TITLE`, the sheet titles, `CANCEL`, `OK`, `LOOK_AGAIN`, `COULDNT`); `saves_of.rs`: whose the saves are, for the person's confirmation only (reads the folders the survey listed, at most 16 folders, 4,096 entries each, 64 saves).
- `recto-app` `core/removal/sheet.rs`: `Leftovers` (the sheet's state), the menu row, the answer of the person's commands and the sheet's answer. `core/sheets.rs`: the one sheet model and its two tables (what is showing, what answers), one line a feature; saving's problem is the other line. Host: `app/Sources/RectoPDF/SheetModel.swift` draws any `SheetModel` as an alert, buttons in Rust's order, the destructive one marked.
- Shared files touched, one line each: `menu_rows.rs`, `person.rs` (`ANSWERED`), `answer_parts.rs` (`leftovers_answered`), `menus.rs`, `core/qa/levers.rs`, `controls.rs` (`SheetAnswered`).

**The rules in the code.**
- The sheet is made from the person's answers only; the plan is held in the sheet's state until the yes and is in no model, log or QA answer.
- An answer to a sheet that isn't the one showing changes and sends nothing. Any button but an offer's first (Cancel, a window closed with no button, a number out of range) sends nothing and takes the sheet away.
- Errors are told by code: eight codes show their own sentence, any other `COULDNT`, never the message of an error an agent was meant to read.
- The yes never waits for the clear: it starts on a thread of its own, and a menu choice meanwhile is told it is clearing.

**Tests.** `removal/sheet/tests.rs` (the state, pure), `core/headless/leftovers_sheet.rs` (the whole flow on the app's core, an agent never opening a sheet), `headless/menu_twins.rs` (the actor rule `TheSheetIsThePersons`), `headless/sheets.rs` (the save problem on the same model), `recto-cli/tests/app/leftovers.rs` (one step of the save/close/Save As launch: the sheet shot light and dark, the yes trashing a crashed save), and `app/qa/contact-sheet.sh` (`clear-leftovers`).
