# Releasing RectoPDF

A release is **Developer ID signed and notarized**: a zip for people to download and for the in-app updater (SPEC decision 24). Nothing here publishes anything until someone pushes a tag and presses Publish. **None of this has run on GitHub yet:** the workflow and `--signed` were written and read, not run, as they need secrets that aren't there yet. `app/release-zip.sh` with no arguments has been run on this Mac.

## What the owner has to add

Once, by you: the certificate and the key are tied to your Apple account. Never paste a password or a key into a chat or a command line. The commands below read them from files or ask without echoing. It's the same Apple Developer team as Seedwell: `6GND8Z2C7Z`, in `TEAM_ID` in `crates/recto-update/src/lib.rs`. If this app is to be signed by another team, change it there first.

1. **Developer ID Application certificate.** You have one for Seedwell, and it can be used here as it is. Check: `security find-identity -v -p codesigning` shows `Developer ID Application: <your name> (6GND8Z2C7Z)`.
2. **Export it for CI.** Keychain Access › login › My Certificates › that certificate › right-click › Export… as `.p12`, with a new strong password.
3. **Notarization key.** <https://appstoreconnect.apple.com> › Users and Access › Integrations › App Store Connect API › Team Keys › `+`, access *Developer*. Download `AuthKey_<KEY ID>.p8` (only once possible), and note the Key ID and the Issuer ID. Seedwell's can be used here too.
4. **The five secrets** on this repository:
   ```sh
   base64 -i ~/Desktop/developer-id.p12 | gh secret set MACOS_CERT_P12 --repo kkollsga/RectoPDF
   gh secret set MACOS_CERT_PASSWORD --repo kkollsga/RectoPDF      # the .p12's password
   base64 -i ~/Downloads/AuthKey_<KEY ID>.p8 | gh secret set NOTARY_KEY_P8 --repo kkollsga/RectoPDF
   gh secret set NOTARY_KEY_ID --repo kkollsga/RectoPDF            # the Key ID
   gh secret set NOTARY_ISSUER_ID --repo kkollsga/RectoPDF         # the Issuer ID
   ```
   Then delete the `.p12` and the `.p8` from Desktop and Downloads.
5. **A protected Environment, so a pushed tag alone can't reach the keys.** GitHub › the repository › Settings › Environments › New environment, named `release`, with yourself under *Required reviewers*. Move the five secrets to it (`gh secret set NAME --env release --repo kkollsga/RectoPDF`), and delete them from the repository's. Without the review it still works, and then anyone who can push a tag can have something signed.
6. **The repository has to stay public, or the updater can't read it:** it fetches `releases/latest/download/latest.json` with no credentials, on purpose. A private repository answers 404, which reads as "no releases yet".

Without those secrets the release workflow fails at once. There's no unsigned release.

## Releasing

`.github/workflows/release.yml` runs when a `vX.Y.Z` tag is pushed, **all or nothing**:

1. *Tag and CI*: the tag matches `Cargo.toml`, and CI passed on the tagged commit.
2. *Mac build*, a job with no secrets in it at all: `app/release-zip.sh --built-only` compiles everything, checks it reads no override, and runs it in a quiet run that opens a PDF and has to draw it.
3. *Mac signing*, in the `release` Environment, a job that compiles nothing and runs nothing of the product while a key is there: `--signed --already-built` signs what was built, notarizes, staples and zips it. The keychain and every file of a secret are removed, and then `--check-signed` runs the stapled app drawing a PDF, and checks the zip with the updater's own reader and Gatekeeper. The certificate's password reaches `openssl` by the environment, never a command line. The keychain's own throwaway password, made in the job, is on one.
4. *Draft release*: `RectoPDF-X.Y.Z.zip`, `SHA256SUMS` and `latest.json`, not published.
5. *Withdraw the tag* if anything failed or was cancelled, so a fix can tag the same version again. Never a release that's been published: run again after that, this fails and leaves it alone.

CI runs only on `main` and skips a commit that changes only documents, so the gate asks about the newest commit that changes anything else.

**A known limit: several commits pushed at once.** CI runs on what a push leaves at the head, not on each commit in it, so a tag on an earlier commit of the same push has no run of its own, and the gate waits an hour and withdraws it. Tag the head of a push, or push the commit to be tagged on its own first. The converting of the certificate by `openssl` hasn't run either: if the runner's LibreSSL can't read the `.p12`, export it again with the older algorithms (Keychain Access does both), and say so here.

```sh
# bump [workspace.package] version in Cargo.toml, run `cargo check` so Cargo.lock follows (builds use --locked), commit both on main, push, then:
git tag v0.2.0 && git push origin v0.2.0     # needs the owner's go-ahead each time, as tagging and publishing do
gh run watch
```

Read the draft on GitHub and press **Publish**. That's the moment every installed copy is offered the update, within a day. A draft or a pre-release never resolves as `latest`. A bad release is withdrawn by deleting it or marking the one before as Latest: an install reads `latest.json` again before it starts, so one that's gone doesn't go in.

## A check on this Mac (not a release)

```sh
tools/sysguard/heavy.sh app/release-zip.sh --allow-dirty   # full LTO, into app/build/release/
```

Without `--signed` it signs ad hoc and doesn't notarize, whatever the environment says. It checks everything else, including that the real binaries read no override with every override set.

## The version

One source: `version` under `[workspace.package]` in `Cargo.toml`. `app/build.sh` writes it to `CFBundleShortVersionString`, which is what the updater compares.

## How updates work

The app reads `<repo>/releases/latest/download/latest.json` once a day: `version`, `tag`, `minimum_macos`, `zip`, `sha256`, all written by `release-zip.sh`. See `docs/ENGINEERING.md`, "Updates", for the rest.

**A release build is whatever `RECTO_RELEASE=1` compiled.** `app/build.sh` on its own makes a development build, which checks only when asked and never replaces itself, so your own dev builds are safe from it. Keep it that way: don't set `RECTO_RELEASE` by hand.

**What's unproven until the first real signed release:** that a real Developer ID build passes the team and Gatekeeper checks, that a notarized bundle survives the updater's unpacking and the swap with its ticket and signature, what a background reopen does to a real screen (the code sends the window behind everything and waits to go into full screen, and nothing here has seen it on a desktop), how long a first launch takes under Gatekeeper, and the `--signed` and workflow steps themselves. `docs/PLAN.md` carries the item. The refusal side is tested.

**Hardened runtime:** a signed build runs under it, and the renderer loading `libpdfium.dylib` needs that dylib signed by the same team, which `build.sh` does. Tried on this Mac with the owner's Developer ID on 2026-11-08, not notarized: the sandboxed helper loaded the library under library validation and drew.

**The renderer helper.** PDFium's worker is in `Contents/Helpers/RectoRender.app`, sandboxed, laid out and signed by `app/render-helper.sh`, from the inside out and never with `--deep`. `release-zip.sh` runs `render-helper.sh --check` on the built app, the signed one and the one in the zip, and fails if the worker isn't sandboxed with exactly its one entitlement, isn't signed by the app's team with the hardened runtime, or anything could run in its place unsandboxed. A Developer ID build's helper is `io.github.kkollsga.RectoPDF.Render`, and an ad hoc one `...Render.adhoc`, so no container a development build left behind meets a release.

**The identity goes to codesign by its SHA-1.** codesign reads a name on its command line as Mac Roman whatever the locale, so a name with an "å" in it is never found. `RECTO_SIGN_IDENTITY` stays the name, matched against the keychain search list byte for byte, the same list the workflow looks in. Two valid certificates of one name are refused, naming both hashes, and an expired one says it has expired. Only `build.sh --sign-only` reads `RECTO_SIGN_IDENTITY`: every other build, `tools/check.sh` and the tests sign ad hoc whatever the shell has set, so none of them raises a keychain prompt.
