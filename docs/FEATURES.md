# Adding a feature

How a feature goes from nothing to done, after phase R. One page: each step names the file it touches and what it adds there. The rules behind it
are in `AGENTS.md` (rules 1 to 7, "One action, one implementation") and the guide (`docs/agent-instructions/rust_app_development.md`, §2.8 to §2.10).
The worked example is **Find**: read its files beside each step. `tools/new-feature.sh <name>` makes the first files and prints this list, and
`docs/agent-instructions/feature-brief.md` is the brief a feature's worker gets. `tools/doc-paths.sh` checks that every path and name on this page
exists, in every `--quick` and `--full` run.

**Done means** an agent can do it, see it and prove it: a command, the CLI, the window's control sending that command, a QA look, tests at the cheapest
tier that can see each failure, and shots light and dark.

## The path

1. **The command, its level and its words.** It goes in one file, `crates/recto-agent/src/commands/<feature>.rs` (Find: `crates/recto-agent/src/commands/finding.rs`).
   It holds a payload type that implements `Cmd`. None of `Cmd`'s properties has a default, so the compiler asks for each one: `TAG`, `ROWS`,
   `DRAWS`, `READS_PAGES`, `PROTECTED_OK`, `ABOUT` (whose document it reads, for the protected gate), `WAITS`, `tab`, `access`, `route`, `log_details`,
   `run` and `human`. Its rows (`Spec` in `crates/recto-agent/src/table.rs`) are the CLI's grammar, the help and the **level** (`Access::At`).
   `log_details` holds numbers and cleaned names, never document text. `human` gives the plain words.
   **Registration:** one variant line in a `family!` of `crates/recto-agent/src/commands/mod.rs` (Find: `Find(finding::Find)` in `Move`), plus
   `pub mod <feature>;` in the same file.
2. **Its typed payload.** When a command's choices are words or flags that can't go together, the payload is a type and its wire form is a `line`
   struct. `TryFrom` refuses what can't be (Find: `FindAsk`, and `#[line(finding::line::Find)]` on the variant). A refusal carries a `Code` from the one
   table, `crates/recto-agent/src/code.rs`, with its next step. A new code is one line there.
3. **Its state's home.** If the session must remember something, it belongs to one component of `Session`, reached through an accessor in
   `crates/recto-session/src/parts.rs`. Outside the crate it is changed only through a `PartMut` handle's setters (Find: `FindRecord` in
   `crates/recto-session/src/viewing.rs`, `viewing()` and `viewing_mut()`). A new component is listed in `crates/recto-session/src/tests/parts.rs`.
   State the window needs and the session doesn't is the app's (Find: `crates/recto-app/src/find.rs`). The app's life (launch, keeping, quitting)
   is `Lifecycle` in `crates/recto-app/src/core/life.rs`, whose pure machine is `crates/recto-app/src/lifecycle/`, and the record's lock is carried
   out by the keeping desk (`Desk`, `Carrier`) in `crates/recto-app/src/core/keeping.rs`. A feature that needs a step of the life asks first.
4. **The executor.** `run` calls the feature's own function with `Cx`, which carries the session, the `Actor` and the events
   (Find: `command` in `crates/recto-agent/src/find/command.rs`). There is one executor, `execute` in `crates/recto-agent/src/lib.rs`: it checks the
   level (`Actor::may`), the quiet run's files (`quiet_files::check`) and a protected document (`ABOUT`) before `run` is called. A difference
   between a person and an agent is asked of `Actor` (`sees_protected`, `at_the_window`, `author`) and never written as a branch of its own. What the
   window must do afterwards is an `Effect` (`crates/recto-agent/src/effect.rs`). What others hear is an `Event` (`crates/recto-agent/src/event.rs`),
   sent only when something changed.
5. **The CLI.** Nothing to write: `recto` parses every row (`parse_cli` in `crates/recto-agent/src/table.rs`) and prints `human`. Bless the help and
   wire goldens (`RECTO_BLESS=1`, `crates/recto-agent/tests/golden/`) and read their diff: it is the review of the command's surface.
6. **The window's control and its translation.** A control is a `UiAction` (`crates/recto-app/src/core/controls.rs`). `Inner::act` has one arm per
   control that calls the feature's pure translation in `crates/recto-app/src/core/<feature>.rs` (Find: `typed`, `opened`, `step` in
   `crates/recto-app/src/core/find.rs`). That translation returns the *same* command an agent sends, and `submit` in
   `crates/recto-app/src/core/person.rs` runs it as the person. A menu item is a row (Find: `row` in `crates/recto-app/src/core/find.rs`),
   registered in one line of `ROWS` in `crates/recto-app/src/core/menu_rows.rs`. Its title, key, enable rule and action are still written in
   `crates/recto-app/src/menus.rs` (`find_entries`, `enable_rule`, `action`): three short lines in a shared file.
7. **What it publishes.** The window draws view models that are pure functions of state (`crates/recto-app/src/model.rs`). `reconcile`
   (`crates/recto-app/src/core/reconcile.rs`) publishes them after each pass, and the `Listener` (`crates/recto-app/src/listener.rs`) tells the host
   to pull. A feature with its own view publishes it from its own module (Find: `FindView`).
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
| The ratchet | `tools/ratchet.sh`, `tools/ratchet/baseline.txt` | a file, function, struct, enum or type grew past its budget, or a new test launches the app. Split by concern; never raise the number |
| Every command has a row | `every_command_has_a_row_and_every_session_row_has_a_command` | the command isn't in `every_command` in `crates/recto-agent/src/policy_guards.rs` |
| Levels | `walking_every_command_at_every_level`, `every_row_is_enforced_at_every_level_over_the_socket` | something is reachable below its row's level, or opens at Off |
| A protected document | `no_command_tells_an_agent_anything_of_a_protected_document` | an agent's answer differs with the protected document's length or words: check `ABOUT` |
| The owner's edits | `no_command_below_full_loses_the_owners_edits` | an agent below Full lost an owner's step or changed their file |
| Files written | `no_command_writes_over_a_file_thats_there`, `nothing_in_the_crate_writes_a_file_but_through_its_one_way` | a file was written over; use `OwnTemp` and `publish_picture` |
| Copy-restricted files | `a_file_that_asks_not_to_be_copied_asks_anyway_of_every_copy` | a copy or export lacks `--anyway`, or ignores it |
| The quiet run's wall | `no_text_argument_reaches_a_file_outside_the_run`, `every_path_a_command_holds_is_declared_a_file` | a file argument isn't `Kind::Path`, or reaches outside a quiet run |
| Codes | `crates/recto-agent/tests/codes.rs` | a code isn't in the table, or something decides by a code's string or a message's words |
| Controls | `every_control_is_a_command_or_the_hosts_with_a_reason`, `every_menu_id_has_an_enable_rule` | a control sends no command and isn't on the host's list, or its golden line moved |
| What parts expose | `crates/recto-session/src/tests/parts.rs`, `crates/recto-reader/src/surface.rs`, `crates/recto-app/src/core/headless/inner_guards.rs` | a part's reach widened: a reviewer must see that line |
| Fuzzing | `crates/recto-fuzz/src/targets/agent.rs` | parsing a hostile line panics or runs past its budget (every command's line is reached through the table) |

All the registration guards run in every `--quick` (`GUARD_TESTS` in `tools/check-scope.sh`). A failure names the item and says what to do.
Don't loosen a guard or raise a baseline: call back (`docs/agent-instructions/worker-testing.md`, "Call back at an edge case").

## Hand-back

Use the brief's list (`docs/agent-instructions/feature-brief.md`): whole-type counts, the shared files touched and how (`tools/ratchet.sh --report main`),
the guards met, input to frame before and after (`crates/recto-app/src/core/headless/input_to_frame.rs`), the seconds added to CI, and the shots.
