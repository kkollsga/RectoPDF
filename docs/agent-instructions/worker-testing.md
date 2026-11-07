# Testing as a worker

Paste this into a worker's brief. Name the exact check the work needs, because "run the gate" gets the whole gate.

The whole suite decides a hand-back. It's never how you find out why something changed. Agents under pressure reach for the biggest test, so ask the smallest question that answers yours first.

## Which check answers which question

| The question | The check |
|---|---|
| Does this one behaviour work? | One named test: `HEAVY_CLASS=quick tools/sysguard/heavy.sh cargo nextest run --locked -p <crate> -E 'test(=<name>)'` |
| Did my fix work? | `tools/check.sh --failed`: only what failed last time |
| Does it compile, and does what I changed still pass? | `tools/check.sh --quick`: clippy, the Windows compile check, changed crates' tests |
| Why did these inputs change? | Just those inputs, not the whole corpus |
| Is this ready to hand back? | `tools/check.sh --full`, once, last |
| How many full runs has this branch had? | `tools/check.sh --runs` |

## How to work

1. **Settle what you're testing first.** No final run on a dependency pin, a merge or a set of findings that's about to change. Fix all of a review's findings, then run once.
2. **Fix before rerunning.** A failure names its test. Read it, fix it, then run that test or `--failed`. The queue refuses a command that failed on code that hasn't changed.
3. **Run the whole suite once, last,** on the commit you're handing back, and again only if code changed.
4. **Commit before the full run.** A dirty tree runs, but it's never recorded, so its pass is worth nothing later.

## What the tools do

- **A pass on identical code counts.** `--full` on a clean tree whose code has passed before says which commit passed it and when, lets the push guard accept this commit, and runs nothing. That covers reverts, doc-only commits and a merge identical to its branch. Documents are `docs/` and any `.md`. `CHECK_AGAIN=1` runs it for real, to look into a pass you have reason to doubt.
- **A branch gets two full runs:** one for hand-back, one after a review's fixes. The count starts again when the branch takes in new code from `main`. `main` gets two per range of commits not yet pushed, and a push starts its count again.
- **A third run needs a reason:** `tools/check.sh --full --why "what this run decides"`. It starts at once, and the reason goes in the ledger with the time, branch, commit, result and seconds. It's a speed bump to make the run deliberate and visible, never a lock. If you're asked for one, first ask whether a smaller check would answer it.
- **Stopping at the first failure** means a failed run shows one failure. A run stopped from outside, or refused by the queue, doesn't count against the budget.

## What a pass doesn't cover

The memory of a pass covers the tracked code, the test tools' versions (nextest, Swift, macOS), whether the Windows target is installed, and settings that change what tests do (`RECTO_*`, `NEXTEST_*`, `RUSTFLAGS`). It doesn't cover the real PDFs in the distillPDF checkout, which `recto-reader`'s `looks_on_real_files` and the fuzz seeds read when they're there. After changing those, use `CHECK_AGAIN=1`.

A test that fails only sometimes can pass once and then be remembered, so a failure you've seen still gets a fix. CI and the reviews still run on everything.

Never `git stash`, which is shared across worktrees. To compare with old code, use a temporary commit or a second worktree.
