#!/usr/bin/env -S python3 -I
"""notices: writes THIRD-PARTY-NOTICES.txt, every third-party component that ships in RectoPDF.app and its licence text.

    tools/notices.py            Writes the file from both lockfiles, cargo metadata (offline) and target/pdfium/licenses.
    tools/notices.py --check    Says whether the file is what it would write, and exits 1 if not.

Run it after any change to Cargo.lock, crates/recto-headings/Cargo.lock or the PDFium pin. `app/build.sh` copies the file into the
bundle as Contents/Resources/Licenses.txt, and `recto-cli`'s `notices` test fails when a lockfile has a package the file doesn't
account for, or the bundle's copy differs.

What ships: the normal (not dev, not build) dependencies, on aarch64-apple-darwin, of the binaries `app/build.sh` puts in the bundle:
recto-ffi (linked into the app), recto-cli, recto-render-service, recto-indexer, recto-update and the separate workspace's
recto-headings. Every other package in a lockfile is listed as not shipped, with why, so the file accounts for each one. The Swift
app has no package dependencies; the Swift runtime and the frameworks it uses are the system's. PDFium's own third-party licences
come from its release tarball, which tools/pdfium.sh checks by digest and unpacks into target/pdfium/licenses.
"""
import hashlib
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "THIRD-PARTY-NOTICES.txt")
PDFIUM = os.path.join(ROOT, "target", "pdfium")
TARGET = "aarch64-apple-darwin"
SHIPPED_ROOTS = {"recto-ffi", "recto-cli", "recto-render-service", "recto-indexer", "recto-update", "recto-headings"}
LICENCE_FILE = re.compile(r"^(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT|UNLICENSE)", re.I)
# Packages that don't carry their licence text: where it is instead.
FALLBACK = {
    # The same authors and repository as alloc-no-stdlib, whose LICENSE is that BSD-3-Clause text.
    "alloc-stdlib": ("crate", "alloc-no-stdlib"),
    # MPL-2.0's text, which the uniffi crates name and don't include. Their source is on crates.io and at github.com/mozilla/uniffi-rs.
    "uniffi": ("file", "tools/notices/MPL-2.0.txt"),
    "uniffi_core": ("file", "tools/notices/MPL-2.0.txt"),
    "uniffi_internal_macros": ("file", "tools/notices/MPL-2.0.txt"),
    "uniffi_macros": ("file", "tools/notices/MPL-2.0.txt"),
    "uniffi_meta": ("file", "tools/notices/MPL-2.0.txt"),
    "uniffi_pipeline": ("file", "tools/notices/MPL-2.0.txt"),
}
# SPDX ids a part of an MIT app can ship under: permissive, or MPL-2.0's file-level copyleft, met by these notices and the crates'
# published source. GPL, LGPL, AGPL, SSPL and the like aren't here, so a package needing one fails the check.
ALLOWED = {"MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unlicense", "0BSD",
           "Unicode-3.0", "Unicode-DFS-2016", "BSL-1.0", "MPL-2.0", "CC0-1.0"}


def metadata(manifest, filtered):
    args = ["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--manifest-path", manifest]
    if filtered:
        args += ["--filter-platform", TARGET]
    return json.loads(subprocess.run(args, cwd=ROOT, check=True, capture_output=True, text=True).stdout)


def lock_packages(path):
    """(name, version) of every package in a lockfile that comes from outside the repo."""
    found, name, version = set(), None, None
    for line in open(path, encoding="utf-8"):
        line = line.strip()
        if line == "[[package]]":
            name = version = None
        elif line.startswith("name = \"") and name is None:
            name = line.split('"')[1]
        elif line.startswith("version = \"") and version is None:
            version = line.split('"')[1]
        elif line.startswith("source = ") and name and version:
            found.add((name, version))
    return found


def terms(expression):
    """The AND terms of a licence expression, each a list of OR choices. Old `/` spellings count as OR."""
    expression = expression.replace("/", " OR ").replace("(", " ").replace(")", " ")
    return [[choice.strip() for choice in term.split(" OR ") if choice.strip()] for term in expression.split(" AND ")]


def acceptable(expression):
    return bool(expression) and all(any(choice in ALLOWED for choice in term) for term in terms(expression))


def shipped(meta):
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    members = set(meta["workspace_members"])
    todo = [i for i in members if packages[i]["name"] in SHIPPED_ROOTS]
    seen = set()
    while todo:
        i = todo.pop()
        if i in seen:
            continue
        seen.add(i)
        for dep in nodes[i]["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]):
                todo.append(dep["pkg"])
    return {i for i in seen if packages[i]["source"]}, packages


def why_not(meta_all, pkg_id):
    """Why a package in the lockfile doesn't ship: what kind of dependency reaches it, or that only another platform needs it."""
    kinds = set()
    for node in meta_all["resolve"]["nodes"]:
        for dep in node["deps"]:
            if dep["pkg"] == pkg_id:
                for kind in dep["dep_kinds"]:
                    if kind["target"] and kind["kind"] is None:
                        kinds.add("another platform")
                    else:
                        kinds.add({None: "a tool or test", "dev": "tests only", "build": "build scripts only"}[kind["kind"]])
    return ", ".join(sorted(kinds)) or "not used by anything that ships"


def licence_texts(package, by_name):
    rule = FALLBACK.get(package["name"])
    if rule and rule[0] == "file":
        return [(os.path.basename(rule[1]), open(os.path.join(ROOT, rule[1]), encoding="utf-8").read())]
    source = by_name[rule[1]] if rule else package
    folder = os.path.dirname(source["manifest_path"])
    texts = []
    # A crate from git that's a member of its repository's workspace has its licence at the workspace's root, up to two levels up.
    folders = [folder]
    if "/git/checkouts/" in folder:
        folders += [os.path.dirname(folder), os.path.dirname(os.path.dirname(folder))]
    for here in folders:
        for name in sorted(os.listdir(here)):
            path = os.path.join(here, name)
            if LICENCE_FILE.match(name) and os.path.isfile(path):
                texts.append((name, open(path, encoding="utf-8", errors="replace").read()))
        if texts:
            break
    if package.get("license_file") and not texts:
        path = os.path.join(folder, package["license_file"])
        texts.append((package["license_file"], open(path, encoding="utf-8", errors="replace").read()))
    return texts


def tidy(text):
    return "\n".join(line.rstrip() for line in text.replace("\r\n", "\n").replace("\t", "    ").strip("\n").splitlines()) + "\n"


def build():
    version = re.search(r'^\[workspace\.package\][^\[]*?^version = "([^"]+)"', open(os.path.join(ROOT, "Cargo.toml")).read(), re.M | re.S)
    version = version.group(1) if version else "?"
    pin = re.search(r"releases/download/(chromium/\d+)/", open(os.path.join(ROOT, "tools", "pdfium.sh")).read()).group(1)
    pdfium_version = ".".join(
        dict(line.strip().split("=", 1) for line in open(os.path.join(PDFIUM, "VERSION")) if "=" in line)[k] for k in ("MAJOR", "MINOR", "BUILD", "PATCH")
    )
    licence_dir = os.path.join(PDFIUM, "licenses")
    if not os.path.isfile(os.path.join(licence_dir, "pdfium.txt")):
        sys.exit("notices: target/pdfium/licenses is missing: run tools/pdfium.sh first.")

    ships, refused, inventory = {}, [], []
    accounted = set()
    for manifest, lock in (("Cargo.toml", "Cargo.lock"), ("crates/recto-headings/Cargo.toml", "crates/recto-headings/Cargo.lock")):
        meta = metadata(os.path.join(ROOT, manifest), True)
        meta_all = metadata(os.path.join(ROOT, manifest), False)
        ids, packages = shipped(meta)
        by_name = {p["name"]: p for p in meta_all["packages"]}
        shipped_keys = {(packages[i]["name"], packages[i]["version"]) for i in ids}
        for i in ids:
            p = packages[i]
            ships[(p["name"], p["version"])] = p, by_name
        for p in meta_all["packages"]:
            key = (p["name"], p["version"])
            if p["source"] and key not in shipped_keys and key not in ships and key not in accounted:
                inventory.append(("not-shipped", p["name"], p["version"], why_not(meta_all, p["id"])))
                accounted.add(key)
        # Anything in the lockfile cargo metadata didn't list: a package for a platform it never resolves for.
        for name, ver in sorted(lock_packages(os.path.join(ROOT, lock))):
            if (name, ver) not in ships and (name, ver) not in accounted:
                inventory.append(("not-shipped", name, ver, "not resolved for the app: an optional feature or another platform"))
                accounted.add((name, ver))
    inventory = [row for row in inventory if (row[1], row[2]) not in ships]

    lines = []
    out = lines.append
    out(f"RectoPDF {version}: third-party software notices")
    out("")
    out("RectoPDF is MIT licensed (see LICENSE). It includes the third-party software below, each under its own licence, whose")
    out("text follows. Generated by tools/notices.py from Cargo.lock, crates/recto-headings/Cargo.lock and PDFium's release; don't")
    out("edit it by hand. The Swift app uses only Apple's frameworks and the system's Swift runtime, with no package dependencies.")
    out("")
    out("Licence check against MIT: every component below can be shipped in an MIT app. Most are MIT, Apache-2.0, BSD or Zlib.")
    out("The uniffi crates are MPL-2.0, a file-level licence: they're used unmodified, their source is published on crates.io")
    out("and at https://github.com/mozilla/uniffi-rs, and the MPL-2.0 text is below. Where a component offers a choice of")
    out("licences, it's used under any of them that's listed. None needs the GPL, LGPL or AGPL: ICU's and LLVM's files below")
    out("mention the GPL only in exceptions and alternatives that widen what's allowed, and FreeType is used under its own")
    out("FreeType License (FTL).")
    out("")
    out("=" * 78)
    out("Inventory")
    out("=" * 78)
    out(f"pdfium {pdfium_version} BSD-3-Clause AND Apache-2.0 (bblanchon/pdfium-binaries {pin}, no V8, no XFA)")
    for (name, ver), (p, _) in sorted(ships.items()):
        out(f"ships {name} {ver} {p['license'] or 'see-text'}")
        if not acceptable(p["license"] or ""):
            refused.append(f"{name} {ver}: {p['license']}")
    out("")
    out("In a lockfile and not in the app, listed so every package is accounted for:")
    for kind, name, ver, why in sorted(inventory, key=lambda row: (row[1], row[2])):
        out(f"{kind} {name} {ver} ({why})")
    out("")
    out("=" * 78)
    out(f"PDFium {pdfium_version}, from bblanchon/pdfium-binaries {pin}, and the third-party code built into it")
    out("=" * 78)
    out("")
    out("--- pdfium-binaries (the build) ---")
    out(tidy(open(os.path.join(PDFIUM, "LICENSE"), encoding="utf-8").read()))
    for name in sorted(os.listdir(licence_dir)):
        out(f"--- {name} ---")
        out(tidy(open(os.path.join(licence_dir, name), encoding="utf-8", errors="replace").read()))
    out("=" * 78)
    out("Rust crates")
    out("=" * 78)
    out("")
    # Each crate with its own notices; a text many crates share word for word (Apache-2.0, mostly) is given once and referred to.
    shared, seen_texts = {}, {}
    for (name, ver), (p, by_name) in sorted(ships.items()):
        for fname, text in licence_texts(p, by_name):
            digest = hashlib.sha256(tidy(text).encode()).hexdigest()
            shared.setdefault(digest, []).append(f"{name} {ver}")
            seen_texts[digest] = tidy(text)
    common = {d for d, users in shared.items() if len(users) >= 3}
    order = sorted(common, key=lambda d: (shared[d][0], seen_texts[d]))
    names = {d: f"[shared text {n + 1}]" for n, d in enumerate(order)}
    for (name, ver), (p, by_name) in sorted(ships.items()):
        out(f"--- {name} {ver} ({p['license'] or 'see text'}) {p.get('repository') or ''}".rstrip() + " ---")
        texts = licence_texts(p, by_name)
        if not texts:
            refused.append(f"{name} {ver}: no licence text found")
        for fname, text in texts:
            digest = hashlib.sha256(tidy(text).encode()).hexdigest()
            if digest in common:
                out(f"{fname}: see {names[digest]} below.")
            else:
                out(f"{fname}:")
                out(seen_texts[digest])
        out("")
    for digest in order:
        out("=" * 78)
        out(f"{names[digest]}, for {', '.join(shared[digest])}")
        out("=" * 78)
        out(seen_texts[digest])
    return "\n".join(lines).rstrip("\n") + "\n", refused


def main():
    text, refused = build()
    if refused:
        sys.exit("notices: these can't ship in an MIT app as they are:\n  " + "\n  ".join(refused))
    if "--check" in sys.argv:
        current = open(OUT, encoding="utf-8").read() if os.path.exists(OUT) else ""
        if current != text:
            sys.exit("notices: THIRD-PARTY-NOTICES.txt is out of date: run tools/notices.py.")
        print("notices: up to date")
        return
    with open(OUT + ".tmp", "w", encoding="utf-8") as f:
        f.write(text)
    os.replace(OUT + ".tmp", OUT)
    print(f"notices: wrote {OUT}")


if __name__ == "__main__":
    main()
