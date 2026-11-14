# Adding a feature

How a feature goes from nothing to done, after phase R. One page: each step names the file it touches and what it adds there. The rules behind it
are in `AGENTS.md` (rules 1 to 7, "One action, one implementation") and the guide (`docs/agent-instructions/rust_app_development.md`, §2.8 to §2.10).
The worked example is **Find**: read its files beside each step. `tools/new-feature.sh <name>` makes the first files and prints this list, and
`docs/agent-instructions/feature-brief.md` is the brief a feature's worker gets. `tools/doc-paths.sh` checks that every path and name on this page
exists, in every `--quick` and `--full` run.

**Done means** an agent can do it, see it and prove it: a command, the CLI, the window's control sending that command, a QA look, tests at the cheapest
tier that can see each failure, and shots light and dark.

## Make room first

A feature does not start by growing a hub. Before step 1, look at the files it will touch (`tools/ratchet.sh --report main` on a trial branch says how
much each shared file grew) and ask of each whether the feature's lines would push it past a budget or put the feature's logic in a shared file. If so,
**the extension point is its own commit, first, and moves code unchanged**, with the guard that pins what the move must not change already in place
(`AGENTS.md`, "Guards go in before a move"). The highlights pilot (R4.3) made room with nine such commits before its first line (`git log --grep="R4.3
extension point"`): `PageEdits`' undo history became one part, `History` (private, in `crates/recto-session/src/edit.rs`), so a new kind of step (`Step::Mark`) joined
a list and not a type with 60 methods; a save's catalog additions, copying and in-memory form moved to `crates/recto-core/src/save/catalog.rs`,
`crates/recto-core/src/save/copying.rs` and `crates/recto-core/src/save/in_memory.rs`, so the new annotations had a file (`crates/recto-core/src/save/annotate.rs`) beside them; the record's checks and its capped-list reader
moved to `crates/recto-session/src/record/validate.rs` and `crates/recto-session/src/record/capped.rs`, where the new step's checks went; the page view's known images moved to
`crates/recto-render-service/src/host/images.rs`; what the pointer looks like over a page moved out of the bridge. What it did not make room for it paid
for later, with a debt: a row and an answer that took the whole `Inner` (whole-`Inner` 98 to 100, repaid by `RowParts` and `AnswerParts`: step 6),
and a field on the page view's shared state (`Shared` 15 to 16, repaid by `workers::Seen`). **If room can't be made from what the feature owns, call
back before writing the feature.**

## The path

1. **The command, its level and its words.** It goes in one file, `crates/recto-agent/src/commands/<feature>.rs` (Find: `crates/recto-agent/src/commands/finding.rs`).
   It holds a payload type that implements `Cmd`. None of `Cmd`'s properties has a default, so the compiler asks for each one: `TAG`, `ROWS`,
   `DRAWS`, `READS_PAGES`, `PROTECTED_OK`, `ABOUT` (whose document it reads, for the protected gate), `WAITS`, `tab`, `access`, `route`, `log_details`,
   `run` and `human`. Its rows (`Spec` in `crates/recto-agent/src/table.rs`) are the CLI's grammar, the help and the **level** (`Access::At`).
   `log_details` holds numbers and cleaned names, never document text. `human` gives the plain words. **`WAITS` is true when the command may hold the
   session's thread for long** (it reads pages: Highlight, `page insert`, Save). For a person that changes how it runs: `person::submit`
   (`crates/recto-app/src/core/person.rs`) runs a `WAITS` command on a thread of its own and everything else on the core queue. **The person's ordered commands
   (every edit, a save, a print, a close) keep their order through the person's lane** (`crates/recto-app/src/queue/lane.rs`, started in
   `core/person_lane.rs`): a feature with a new ordered `WAITS` command passes its turn where its command has fixed what it acts on, by
   `crate::queue::lane::pass_here()` (a print once its pages are read, a save once begun), or at its end.
   **Registration:** one variant line in a `family!` of `crates/recto-agent/src/commands/mod.rs` (Find: `Find(finding::Find)` in `Move`), plus
   `pub mod <feature>;` in the same file.
2. **Its typed payload.** When a command's choices are words or flags that can't go together, the payload is a type and its wire form is a `line`
   struct. `TryFrom` refuses what can't be (Find: `FindAsk`, and `#[line(finding::line::Find)]` on the variant). A refusal carries a `Code` from the one
   table, `crates/recto-agent/src/code.rs`, with its next step. A new code is one line there.
3. **Its state's home.** If the session must remember something, it belongs to one component of `Session`, reached through an accessor in
   `crates/recto-session/src/parts.rs`. Outside the crate it is changed only through a `PartMut` handle's setters (Find: `FindRecord` in
   `crates/recto-session/src/viewing.rs`, `viewing()` and `viewing_mut()`). A new component is listed in `crates/recto-session/src/tests/parts.rs`.
   **A new record is persisted, guarded and fuzzed** (files on disk are hostile, rule 6). If it is saved in the record or a journal, it is a variant
   of `JournalOp`/`StepRecord` in `crates/recto-session/src/record.rs` with caps on every list (`crates/recto-session/src/record/capped.rs`; `MAX_MARKS`, `MAX_MARK_RECTS` and
   `MAX_MARK_LINES` in `crates/recto-session/src/edit/marks.rs`), its checks in `crates/recto-session/src/record/validate.rs` (`MarkRecord::all_fit`), and a version bump for the
   journal that holds it, so an older app refuses what it can't read (`MARKED_JOURNAL_VERSION`, `journal_marked`). **A size guard:** what the history
   charges for a step is tested against what it really holds (`what_a_highlight_step_will_hold_is_what_it_holds`, which first walked only two of
   four step kinds and passed with the others wrong: walk every variant), and a field added to a shared struct stays under its compile-time size
   (`size_of::<Shared>() <= 2784` in `crates/recto-reader/src/engine/selection.rs`). **A fuzz target:** the one that parses the record, the proto frame
   or the file the feature reads reaches the new record by construction, not by luck (`crates/recto-fuzz/src/targets/record.rs`, `crates/recto-fuzz/src/targets/edits.rs`,
   `crates/recto-fuzz/src/targets/pdf.rs` for what a save writes and a file's own annotations, `crates/recto-fuzz/src/targets/proto.rs` for a frame to the worker); the hostile corpus grows with it.
   State the window needs and the session doesn't is the app's (Find: `crates/recto-app/src/find.rs`). The app's life (launch, keeping, quitting)
   is `Lifecycle` in `crates/recto-app/src/core/life.rs`, whose pure machine is `crates/recto-app/src/lifecycle/`, and the record's lock is carried
   out by the keeping desk (`Desk`, `Carrier`) in `crates/recto-app/src/core/keeping.rs`. A feature that needs a step of the life asks first.
4. **The executor.** `run` calls the feature's own function with `Cx`, which carries the session, the `Actor` and the events
   (Find: `command` in `crates/recto-agent/src/find/command.rs`). There is one executor, `execute` in `crates/recto-agent/src/lib.rs`: it checks the
   level (`Actor::may`), the quiet run's files (`quiet_files::check`) and a protected document (`ABOUT`) before `run` is called. A difference
   between a person and an agent is asked of `Actor` (`sees_protected`, `at_the_window`, `author`) and never written as a branch of its own. What
   others hear is an `Event` (`crates/recto-agent/src/event.rs`), sent only when something changed. `Effect` (`crates/recto-agent/src/effect.rs`) and
   `CoreCall::Find` are Find's own channel to the window, not an extension point: a feature that needs the window to act after its command calls back
   first.
   **A command that changes the file goes through the one writer** (rule 5): a new kind of change is a plan the lopdf writer in
   `crates/recto-core/src/save.rs` carries out, with its own file under `crates/recto-core/src/save/` (highlights: `crates/recto-core/src/save/annotate.rs`, which dresses the
   new annotations, and reads the written file back to say where each went: `MarksWritten`), and a function in the session's own history that carries
   the steps *on* past a Save (`crates/recto-session/src/edit/marks/saved.rs`: the history goes on after a Save, and what can't be carried is counted
   as rolled off, never dropped quietly). What a Save would lose of the owner's steps is refused below Full (`owner_edits_at_stake`), and the walk in
   `crates/recto-agent/src/policy_guards.rs` starts from the new step, done and undone. A removal from the file is a rewrite, which an agent needs Full for.
   The bound on the work (pages read, objects touched) is the save's, not the feature's. Never write through PDFium.
5. **The CLI.** Nothing to write: `recto` parses every row (`parse_cli` in `crates/recto-agent/src/table.rs`) and prints `human`. Bless the help and
   wire goldens (`RECTO_BLESS=1`, `crates/recto-agent/tests/golden/`) and read their diff: it is the review of the command's surface.
6. **The window's control and its translation.** A control is a `UiAction` (`crates/recto-app/src/core/controls.rs`). `Inner::act` has one arm per
   control that calls the feature's pure translation in `crates/recto-app/src/core/<feature>.rs` (Find: `typed`, `opened`, `step` in
   `crates/recto-app/src/core/find.rs`). That translation returns the *same* command an agent sends, and `submit` in
   `crates/recto-app/src/core/person.rs` runs it as the person. **How it went is shown to the person by the feature's `answered`, one line in
   `ANSWERED` there** (Find: `answered` in `crates/recto-app/src/core/find.rs`): without it a refusal shows the person nothing, as an agent's words
   are never shown to them. A menu item is a row (Find: `row`), registered in one line of `ROWS` in `crates/recto-app/src/core/menu_rows.rs`.
   **Neither is given the app.** A row is `fn row<'a>(parts: &RowParts<'a>, asked: &Asked<'_>) -> Option<Row<'a>>`
   (`crates/recto-app/src/core/row_parts.rs`), and an answer is `fn answered(parts: &AnswerParts<'_>, ran: &Ran<'_>)`
   (`crates/recto-app/src/core/answer_parts.rs`). Their fields are private, so what a row or an answer can do is the list of methods
   on its parts type. Sending a command as the person is `parts.person().submit(command)`. A row's way copies the parts into its closure first
   (`let parts = *parts;`). When a row or an answer needs something that isn't on the list, the feature adds **one method named for the need**
   (`can_save`, `fail_open`; not `tools()` or `session()`) to `RowParts` or `AnswerParts`. That is a registration, and it shows in the listing in
   `crates/recto-app/src/core/headless/inner_guards.rs`. It never reaches a field, and never takes `Inner`. Most of today's methods serve one
   feature each: 14 of `RowParts`' 19 (four are shared, one makes it), 10 of `AnswerParts`' 12 (`say` is shared), and `save` and
   `snapshot_anyway` of `Person`'s 5 (each starts the person's run on a thread of its own, which holds the app; the print's Print anyway is its
   command, sent with `submit`). **`find_handle` on `AnswerParts`
   is the one wide reach:** it hands an answer a `FindHandle` whose fields are `pub` (the session, the panes, the page view, the listener), so it is
   Find's alone and no other answer calls it. **A way marked `Agents::AsTheMenu` never calls `parts.person()`:** an agent's palette run executes that
   way, and nothing an agent sends may run as the person (AGENTS.md, "One action, one implementation"). A parts type that keeps gaining one
   feature's methods is turning into a hub: say so in the hand-back. The counts, by the ratchet's parser, on 2026-11-14: `RowParts` 18 methods,
   `AnswerParts` 11, `Person` 6. Its
   title, key, enable rule and action are still written in `crates/recto-app/src/menus.rs`, a hub: Find's take about a dozen lines in five places
   there (`find_entries` and its `chain`, the ids under an `Enable` rule, three `action` arms, two `Action` variants).
   **Find's other reaches are not the pattern.** The pattern is one line each: a `UiAction` variant and its arm, a `ROWS` line, an `ANSWERED` line,
   one forwarding export per view in `crates/recto-ffi/src/lib.rs`. Find also has a `CoreCall` variant of its own (`crates/recto-app/src/queue.rs`)
   with its arm and a step every pass in `crates/recto-app/src/core/session.rs`, a hook on its events by name in `crates/recto-app/src/core/host.rs`,
   and a share of the sidebar's layout in the bridge: a feature that needs any of those asks first.
7. **What it publishes.** The window draws view models that are pure functions of state (`crates/recto-app/src/model.rs`). `reconcile`
   (`crates/recto-app/src/core/reconcile.rs`) publishes them after each pass, and the `Listener` (`crates/recto-app/src/listener.rs`) tells the host
   to pull. A feature with its own view publishes it from its own module (Find: `FindView`).
   **Everything that changes what a page shows reaches every renderer,** not only the one on screen. The pilot's set of file highlights a person took
   out and hasn't saved is drawn without them by the worker (`crates/recto-proto/src/hidden.rs`, `crates/recto-render-service/src/host/hidden.rs`,
   `crates/recto-render/src/pdfium/hidden.rs`, set in memory and never written), and the same set had to reach the page view's draws and its in-flight
   ones (`crates/recto-reader/src/engine/run.rs`: a draw landing after the set changed isn't kept), the thumbnails and the sleep pictures, a print, an
   export and an agent's render, snapshot and SVG (`crates/recto-agent/src/highlighting.rs`, `hidden_in`), the person's Snapshot tool, and search. It
   was found by review three times, one renderer each. `every_renderer_that_draws_a_tab_is_given_its_set` in `crates/recto-agent/src/highlighting.rs`
   now scans the app's crates for every renderer opened outside tests and lists each in `RENDERERS` with what it is for: a feature that changes the
   picture adds a row to that kind of list and the scan fails until each renderer says whether it gets the change. Make the set once a change, not once a
   pass (`a_tabs_set_is_made_once_a_change` in `crates/recto-app/src/core/highlight.rs`, by the tick of the list's last change), and let it go with a closed or sleeping tab.
8. **The bridge and the host's drawing.** `crates/recto-ffi` converts types and forwards, one line per call, under a size budget the ratchet holds
   (Find: `find_view` in `crates/recto-ffi/src/lib.rs`, types in `crates/recto-ffi/src/find.rs`). A person's actions all go through the one export,
   `act`. Swift only draws and reports what was done (Find: `app/Sources/RectoPDF/FindPane.swift`). A rule found in Swift is a bug (rule 2).
9. **Its QA.** An agent sees the feature in `recto qa state` and in shots. A QA verb is only for what only the host has (the pointer, a key in a
   field, a shot), never a stand-in for a command. Its row and its `QaVerb` line go beside the other features' rows in
   `crates/recto-agent/src/commands/qa/rows_features.rs`. The app answers it in `crates/recto-app/src/core/qa/mod.rs`, and the host in its area's
   `app/Sources/RectoPDF/QAChannel+Find.swift`. `swift_answers_exactly_the_tables_verbs` holds the two sides to the table.
10. **Its tests, by tier.** Each failure is tested at the cheapest tier that can see it (`AGENTS.md`, "CI doesn't get slower"):
    - *Unit*, in the module: the command's rule, its typed payload, its words (Find: `crates/recto-agent/src/find.rs` and
      `crates/recto-app/src/find/tests.rs`).
    - *Headless app*, in process, no window: what a person's control and an agent's command do through the real app core, with `HeadlessApp`
      (`crates/recto-app/src/testkit/headless.rs`), in `crates/recto-app/src/core/headless/<feature>.rs` (Find:
      `an_agents_step_to_a_hit_selects_it_on_the_page`). Most of a feature's tests live here.
    - *The control golden*: `every_control_is_a_command_or_the_hosts_with_a_reason` (`crates/recto-app/src/core/headless/coverage.rs`) records
      every control's command and what the host heard, in `crates/recto-app/src/core/headless/golden/controls.txt`. A new control changes one line.
    - *Process*: `recto` as a process, one test per command at most (Find: `crates/recto-cli/tests/find.rs`, listed in `PROCESS_BINARIES` in
      `tools/check-scope.sh`).
    - *Window*: the real app, only for what only it shows (drawing, AppKit, the real launch). **The number of tests that launch the app only goes
      down** (the ratchet counts them), so a new window check joins a launch that already exists (Find: `find_in_the_sidebar_lists_steps_and_keeps_to_its_room`
      in `crates/recto-cli/tests/app.rs`).
11. **Its shots.** One step in `app/qa/contact-sheet.sh` (each feature once, light and dark, laid out on two sheets) and, if the feature needs
    depth, a script of its own beside it (`app/qa/selection-shots.sh` is a small one). The coordinator looks at them before the owner does.
12. **Its notes.** `docs/engineering/<feature>.md` (Find: `docs/engineering/find.md`), plus one line in the table in `docs/engineering/README.md`.

## The guards it will meet, and what a failure means

| Guard | Where | It fails when |
|---|---|---|
| The ratchet | `tools/ratchet.sh`, `tools/ratchet/baseline.txt` | a file, function, struct, enum or type grew past its budget, a new test launches the app, or one more function takes the whole `Inner` (a row or an answer that does is the wrong signature: step 6). Split by concern; never raise the number |
| Every command has a row | `every_command_has_a_row_and_every_session_row_has_a_command` | the command isn't in `every_command` in `crates/recto-agent/src/policy_guards.rs` |
| Levels | `walking_every_command_at_every_level`, `every_row_is_enforced_at_every_level_over_the_socket` | something is reachable below its row's level, or opens at Off |
| A protected document | `no_command_tells_an_agent_anything_of_a_protected_document` | an agent's answer differs with the protected document's length or words: check `ABOUT` |
| The owner's edits | `no_command_below_full_loses_the_owners_edits` | an agent below Full lost an owner's step or changed their file |
| Files written | `no_command_writes_over_a_file_thats_there`, `nothing_in_the_crate_writes_a_file_but_through_its_one_way` | a file was written over; use `OwnTemp` and `publish_picture` |
| Copy-restricted files | `a_file_that_asks_not_to_be_copied_asks_anyway_of_every_copy` | a copy or export lacks `--anyway`, or ignores it |
| The quiet run's wall | `no_text_argument_reaches_a_file_outside_the_run`, `every_path_a_command_holds_is_declared_a_file` | a file argument isn't `Kind::Path`, or reaches outside a quiet run |
| Codes | `crates/recto-agent/tests/codes.rs` | a code isn't in the table, or something decides by a code's string or a message's words |
| Controls | `every_control_is_a_command_or_the_hosts_with_a_reason`, `every_menu_id_has_an_enable_rule` | a control sends no command and isn't on the host's list, or its golden line moved |
| What a person is shown | `what_a_person_is_shown_after_each_command_is_as_it_was` (`crates/recto-app/src/core/person.rs`) | a feature's answer says other words, or says anything for an agent's refusal: a new answer adds its cases to the table |
| What parts expose | `crates/recto-session/src/tests/parts.rs`, `crates/recto-reader/src/surface.rs`, `crates/recto-app/src/core/headless/inner_guards.rs` | a part's reach widened: a reviewer must see that line |
| Every renderer | `every_renderer_that_draws_a_tab_is_given_its_set` in `crates/recto-agent/src/highlighting.rs` | a renderer opened outside tests isn't in `RENDERERS`, or draws a tab's pages without the set of what the tab changed |
| Fuzzing | `crates/recto-fuzz/src/targets/agent.rs` | parsing a hostile line panics or runs past its budget (every command's line is reached through the table) |

The guards named in `GUARD_TESTS` (`tools/check-scope.sh`) run in every `--quick` whatever changed; the rest (the walks in
`crates/recto-agent/src/policy_guards.rs`, the parts' listings, fuzzing) run with their crate's tests, in `--quick` when that crate changed and in
every full run. A failure names the item and says what to do.
**When a registration raises a guard.** A new feature can't avoid a few lines in shared files, and a few of the guards count them: the ratchet's
whole-`Inner` count, a struct's fields, a type's methods, a file's lines. The pilot raised two (whole-`Inner` 98 to 100, `Shared` 15 to 16). The way
through: (1) look first for the registration that doesn't raise one, a row and an answer that take the parts types (step 6) and not `Inner`; (2) if the
count really must rise, it is a **debt with a date**: a hand edit of `tools/ratchet/baseline.txt` that a reviewer sees, its reason in the commit, and a line in
`docs/PLAN.md` that says who repays it and by when; never a quiet raise, and never in a worker's branch without a call back; (3) code moved unchanged
keeps its entries with `tools/ratchet.sh --moved <old> <new>`, and a number that has shrunk is locked in with `tools/ratchet.sh --update`; (4) when a
merge leaves `tools/ratchet/baseline.txt` in conflict (two branches each lowered a number, or each added a key), `tools/ratchet.sh --resolve` writes the
merge from git's three versions, then `tools/ratchet.sh` runs on the merged tree, since two branches adding to the same thing can make the true number
higher than either side's, and that is raised only by a hand edit. `tools/ratchet.sh --report main` says which shared files the branch touched and by how
many lines: the hand-back quotes it.
Don't loosen a guard or raise a baseline: call back (`docs/agent-instructions/worker-testing.md`, "Call back at an edge case").

## Hand-back

Use the brief's list (`docs/agent-instructions/feature-brief.md`): whole-type counts, the shared files touched and how (`tools/ratchet.sh --report main`),
the guards met, input to frame before and after (`crates/recto-app/src/core/headless/input_to_frame.rs`), the seconds added to CI, and the shots.
