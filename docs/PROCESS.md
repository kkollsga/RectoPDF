# Process reviews

The coordinator is responsible for progress and quality. Owner, 2026-11-07: "Remember you are responsible for the progress and quality. So add a few process reviews every once in a while to catch inefficiencies".

**When:** at each phase's end, and at least after every fourth merge. **Looks at:** what landed against what's open, the queue (`tools/sysguard/heavy.sh --report`), CI, review rounds and their findings, and what the owner found that we didn't. **Ends with:** what changes, lessons added to `docs/agent-instructions/rust_app_development.md`, then checks at the next review whether it helped. An independent reviewer reads the numbers at each phase's end.

## 2026-11-07, about 17 hours in

**Numbers.** Phases 0 and 1 of 8 finished; 27 plan items done, 59 open. About 34,500 lines on `main`, about 25,000 more unmerged. GitHub 50 commits behind local `main`, with its last CI run failed. Queue: 8.3 h waiting against 6.8 h running; 36 of 177 jobs failed, using 0.5 h. Review findings: search 15 then 14, SVG 13 then 13, writer 11 then 6, app about 26 then 15.

**Worked.** Independent reviews found what tests didn't, such as documents leaking into each other and a save overwriting a newer file. The owner found more in minutes in a real window than screenshots had in hours.

**Waste, mostly the coordinator's.** SVG and fuzzing, from Phases 3 and 7, started before the first window merged. Seven workers spent more time waiting than building. New requests joined a branch under review, so its review never finished. Fix rounds added features, so review findings didn't fall. A third build slot opened before reading the guide's warning about reboots, and a merge script didn't stop on a conflict.

**Changing.** Merge first; freeze branches under review; two review rounds at most; about three workers; short dev-build cycles.

**Check next time.** Has the app merged? Is waiting less than running? Are review findings falling? Has anything sat unmerged over a day?
