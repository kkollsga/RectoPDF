# Archive metadata guard — 2026-11-18

The first combined full check on `25e3a4bb` passed all 3,264 Rust tests, built the app, then failed the blanket assertion that the bundle has no extended attributes. Native window/lifecycle checks did not run. The failure was `com.apple.provenance` on fresh bundle files, including newly signed CodeResources.

A no-build probe on this Mac showed that fresh files, directories and an executable copied with `cp -X` already carry provenance. Both recursive clearing and explicit deletion returned success while that protected attribute remained. The source's strict signature verified. Plain ditto packaging followed by raw extraction created AppleDouble files inside the sealed bundle and broke its signature; sequestered metadata followed by the same extraction verified. This is observed OS behavior on this development Mac, not a claim that every macOS behaves identically.

The real release already used `--sequesterRsrc`. `app/archive.sh` now owns that unchanged recipe for release, notarization and stage fixtures, so tests and releases cannot silently drift. It executes ditto directly and receives the same two quoted arguments; signing, notarization and updater-parser policy are unchanged. No app is launched by the guard.

The existing `zip::tests::a_zip_from_ditto_unpacks_whole` now proves the actual property with a tiny native ad-hoc signed fixture and an explicitly injected harmless resource attribute on every supported Mac. It verifies the source signature, calls the real shared archive producer, extracts with the real bounded updater reader, verifies the extracted signature, and retains the existing streamed 300,000-byte data, executable-mode and plist checks. Archive metadata is not installed as files. Signing/verification/packaging calls introduced by the guard use the existing bounded process helper. It never compresses the whole debug app.

The test's private fixture is removed on success. On failure it retains the source, archive, raw output, tool commands/results and rerun line. The failure evidence itself was tested:

- Good checkpoint `b4538b5c` passed in 0.246 s.
- Temporary committed seed `e3b37613` removed only metadata sequestering; the same existing test failed after raw extraction (0.083 s), while the source signature had passed. The seed was dropped.
- After adding evidence retention, temporary committed seed `43e9cf93` failed after extraction (0.253 s). Inspection found the 6,995-byte archive, raw AppleDouble additions, successful source verification and failed extracted verification with the sealed-resource cause. The seed was dropped to `8de1f31d`.
- Only after that proof was the invalid zero-xattr test removed. Its failure mode is now covered by the cheaper existing producer/consumer test, rather than an exception that ignores provenance.
- Final focused guard 0.125 s; good-release fixture 0.484 s. The final quick run passed 3,263 tests, including the strengthened guard at 0.100 s. These are single-run observations on different loads, not a performance benchmark. The former simple ZIP guard took 0.034 s and the deleted bundle scan 0.022 s in the first combined full; no new app-launch test was added.
- `tools/check-test.sh` passed after its bundle-registration row changed. `tools/release/release-test.sh` passed after its fixture also copied the shared archive helper. Independent source and raw-evidence review found no remaining blocker.

The final metadata guard plus good-release fixture passed 20/20 CI-like rounds. The second combined full is recorded after completion. The first failed full remains a failure; no native-check pass is inferred from the Rust tests.

Development evidence is retained under `/tmp/rectopdf-20261117-start/archive-metadata-*`; the made failure's private fixture is `/private/var/folders/7h/n6cl9kt96g79hz0s16q5vfxc0000gn/T/recto-stage-archive-metadata-39447-0`. No installed application or fork was changed by this validation work.

## Caller audit and native repair

The second combined gate exposed an omitted plain-ditto native update fixture. Seven native update scenarios were refused at signature validation before their behavior could be tested (196/203 window tests passed). Registration checks passed; lifecycle did not run. The original consolidation had not inventoried every producer. Evidence and logs were copied to `/tmp/rectopdf-20261117-start/native-update-failed-evidence` before cleanup.

The completed audit found six remaining producers: restore update fixtures, dev placement fixtures, installer fixtures, the dev script, preflight transport and the workflow build/sign transport. All now call `archive.sh`; all remaining ditto calls are extraction or directory copy. Development packaging keeps its existing `--norsrc --noextattr` behavior via explicit `--no-metadata`; releases use sequestering. Native extraction stays unchanged. The three changed Rust fixtures bound the leaf helper to 30 seconds, kill their owned child on timeout and reap it through the existing process helpers.

The existing tiny unit guard now proves both modes. A temporary committed wrong stripping-mode seed `3448fd9a` preserved metadata instead; both strict signatures still passed, but the archive-entry assertion failed (0.116 s). Its retained archive/entries/verification evidence was inspected, then the seed dropped. Final source `4d753d8f` passed the three no-app focused checks (0.130/0.308/0.381 s), quick 3,263/3,263, release shell suite, independent review and pure guard 20/20. Parent then ran exactly the seven failed native update scenarios once with CI process limits: 7/7 passed in 93.870 s. No native test or timeout was removed or relaxed. The final full gate follows this evidence with an explicit reason recorded in the ledger.

## Final integrated result

`d3ccd339` passed `CHECK_ALL=1 tools/check.sh --full --why ...`, exit0: 3,263 Rust tests, native window203/203, four slow lifecycle checks, bundle2/2 and registrations3/3. The reason records the completed producer audit and seven repaired native scenarios. No test timeout was raised and no source changed after this gate. Main merge `db24c956` contains the identical checked tree. Source CI run44211481076 subsequently passed all four jobs; its longest job was18:01. The local result and CI timing remain separate observations.
