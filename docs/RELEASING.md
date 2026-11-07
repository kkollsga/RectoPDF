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
5. **The repository has to stay public, or the updater can't read it:** it fetches `releases/latest/download/latest.json` with no credentials, on purpose. A private repository answers 404, which reads as "no releases yet".

Without those secrets the release workflow fails at once. There's no unsigned release.

## Releasing

`.github/workflows/release.yml` runs when a `vX.Y.Z` tag is pushed, **all or nothing**:

1. *Tag and CI*: the tag matches `Cargo.toml`, and CI passed on the tagged commit.
2. *Mac build*: `app/release-zip.sh --signed`, which checks the app, notarizes and staples it, zips it, and checks the zip with Gatekeeper the way the updater will.
3. *Draft release*: `RectoPDF-X.Y.Z.zip`, `SHA256SUMS` and `latest.json`, not published.
4. *Withdraw the tag* if anything failed or was cancelled, so a fix can tag the same version again.

```sh
# bump [workspace.package] version in Cargo.toml, commit on main, push, then:
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

**What's unproven until the first real signed release:** that a real Developer ID build passes the team and Gatekeeper checks in the installer, that a notarized bundle survives `ditto` and the swap with its ticket and signature, and the `--signed` and workflow steps themselves. `docs/PLAN.md` carries the item. The refusal side is tested.

**Hardened runtime:** a signed build runs under it, and the renderer helper loading `libpdfium.dylib` needs that dylib signed by the same team, which `build.sh` does. This needs a real signature to prove too.
