# A feature's brief

The coordinator fills in this template and sends it to the worker who builds one feature. It is the brief's frame, not its content: the
coordinator writes the behaviour from `SPEC.md` and `docs/OWNER-RULES.md`. Paste `docs/agent-instructions/worker-testing.md` under it whole, since that
is how a worker tests; this page only adds what a feature needs on top. The path is `docs/FEATURES.md`, and `tools/new-feature.sh <name>`
makes the first files.

---

**Feature:** `<name>`. One paragraph: what a person and an agent can do with it when it's done, in the owner's words where there are some.

**Set up:** `tools/worktree.sh new build/<name> .worktrees/<name>`, then `tools/new-feature.sh <name>` there and commit what it makes.

**Where it lives** (its own files; create what it needs):
- the command: `crates/recto-agent/src/commands/<name>.rs`, with its logic in a module of its own if it is long;
- its state, if any: one component of `Session` in `crates/recto-session/src/`, or the app's own part beside the life
  (`crates/recto-app/src/core/life.rs`) and the keeping desk (`crates/recto-app/src/core/keeping.rs`) if it is about launch, keeping or quitting;
- the window: `crates/recto-app/src/core/<name>.rs` (its controls' translations, its menu rows, its view) and its headless tests in
  `crates/recto-app/src/core/headless/<name>.rs`;
- the host: one Swift file of its own in `app/Sources/RectoPDF/` for drawing it;
- its notes: `docs/engineering/<name>.md`.

**Shared files it may touch, a registration each and no logic** (list only the ones it needs, one line each; the coordinator checks them against
every running brief, and names the files other workers are in):
- `crates/recto-agent/src/commands/mod.rs`: its variant in one `family!`, and its `pub mod`;
- `crates/recto-agent/src/code.rs`: a line for each new refusal;
- `crates/recto-agent/src/policy_guards.rs`: the command in `every_command`;
- `crates/recto-agent/tests/golden/`: blessed, with the diff read;
- `crates/recto-app/src/core/controls.rs`: a `UiAction` variant and its `act` arm, one line each;
- `crates/recto-app/src/core/menu_rows.rs`: its line in `ROWS`; `crates/recto-app/src/menus.rs`: its entries, enable rule and action;
- `crates/recto-app/src/core/headless/mod.rs`: its `mod` line; `crates/recto-app/src/core/headless/golden/controls.txt`: its new lines;
- `crates/recto-ffi/src/lib.rs`: one forwarding line per view; `app/qa/contact-sheet.sh`: one step.
A shared file that needs more than its registration is missing an extension point: say so before writing it (guide §2.8).

**Level and route:** the level for each command (and why not one lower), its route (`Runs`), whose document it reads (`ABOUT`), and whether it
waits (`WAITS`). If the brief doesn't say, ask.

**Tests, by tier** (the cheapest tier that can see each failure; each test's doc comment names the failure it catches):

| Tier | Where | What it holds here |
|---|---|---|
| Unit | the feature's modules | the rules it decides, its typed payload's refusals, its words |
| Headless app | `crates/recto-app/src/core/headless/<name>.rs` | the person's control and an agent's command doing the same thing through the one executor; what the window is told; what an agent below Full can't do (the owner's edits) |
| Control golden | `crates/recto-app/src/core/headless/golden/controls.txt` | its new controls' lines, and no other line moved |
| Process | `crates/recto-cli/tests/<name>.rs`, only if needed | one CLI run per command at most |
| Window | an existing launch in `crates/recto-cli/tests/app.rs` | only what only the real app shows; no new launch |
| Walks | `crates/recto-agent/src/policy_guards.rs` and the levels walk | met by adding the command to `every_command`; nothing written by hand |

List the named tests here before the work starts, with the tier each is at; the worker adds what they find.

**Call back** (on top of worker-testing.md's list): a guard that the feature would need to loosen; a person and an agent getting different
answers to the same command beyond what `Actor` says; a new level or a change to one; anything that reads a protected document or writes
a file; a shared file needing more than a registration; a Swift change that decides something.

**Hand-back** (one message, numbers from their source, measured under the load you say):
- what was built, with its paths, and how a person and an agent each do it;
- whole-type counts for every type it touched (fields, and methods across every file the type's `impl` blocks are in), before and after;
- the shared files touched and the lines added to each (`tools/ratchet.sh --report main`), each a registration;
- the guards it met and how each was satisfied; any baseline line that moved, and why it went down;
- input to frame before and after, at 1, 100 and 1,000 tabs (`crates/recto-app/src/core/headless/input_to_frame.rs`), with the load;
- the seconds it added to the check, by tier (`tools/check.sh --slow`), and that it added no test that launches the app;
- its tests run 20 times as CI runs them (`tools/check.sh --like-ci '<filter>' --times 20`);
- its shots, light and dark, from `app/qa/contact-sheet.sh`, looked at, with the paths;
- what wasn't done or tested, and the follow-ups for the plan.
