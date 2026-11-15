"""check-carry: the state a person has, carried from one release to the next, shown before publishing. Run by tools/release/check-carry.sh.

Quiet runs only (off every screen, never active, temp state), on one state folder in the system's temp folder, with the real binaries:
  A  the previous release (as published) opens three PDFs, makes a session (tabs, places, a zoom, a rotated page left unsaved, Find
     text, two tabs put to sleep) and hands off as an update does;
  B  the new build starts on that state: everything that should carry is compared, the whole of `qa state` as the restore tests compare
     it (their exceptions, crates/recto-cli/tests/restore/exceptions/, and this pair's own, carry/<old>-to-<new>.txt), and what by design
     doesn't carry is asserted (older sleep files aren't read: each idle tab wakes once from its PDF). Then it makes what the previous
     release never wrote (a highlight, whose journal is version 2; a sleep file of its own format) and hands off;
  C  the previous release again on what B left, as a copy that went back would: it must start, keep every tab and place, leave the
     newer journal alone (byte for byte), and show that tab as it says it does;
  D  the new build again: nothing B made was lost on the way.
Each comparison says what it compared; every difference not named in an exception, with its reason, fails the check.
"""

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

LSREGISTER = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
repo, previous_zip, new_app, evidence = sys.argv[1:5]
failures, notes = [], []


def say(line):
    print(line, flush=True)


def ok(what):
    say(f"ok    {what}")


def fail(what, detail=""):
    failures.append(f"{what}{': ' + detail if detail else ''}")
    say(f"FAIL  {what}{': ' + detail if detail else ''}")


def check(what, good, detail=""):
    (ok if good else lambda w: fail(w, detail))(what)
    return good


def note(what):
    notes.append(what)
    say(f"note  {what}")


# ---- the PDFs: plain text pages, each page saying its name and number, and a word for Find ----
def make_pdf(path, name, count):
    objs = ["<< /Type /Catalog /Pages 2 0 R >>", f"<< /Type /Pages /Kids [{' '.join(f'{3 + 2 * i} 0 R' for i in range(count))}] /Count {count} >>"]
    for i in range(count):
        text = (f"BT /F1 24 Tf 72 700 Td (Carry check {name} page {i + 1}) Tj 0 -40 Td /F1 14 Tf "
                f"(The quick brown fox jumps over the lazy dog, on page {i + 1}.) Tj 0 -24 Td (A second line with the word marker on it, for Find.) Tj ET")
        objs.append(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 {3 + 2 * count} 0 R >> >> /Contents {4 + 2 * i} 0 R >>")
        objs.append(f"<< /Length {len(text)} >>\nstream\n{text}\nendstream")
    objs.append("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    out, offsets = bytearray(b"%PDF-1.4\n"), []
    for n, body in enumerate(objs, 1):
        offsets.append(len(out))
        out += f"{n} 0 obj\n{body}\nendobj\n".encode()
    xref = len(out)
    out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode() + b"".join(f"{o:010d} 00000 n \n".encode() for o in offsets)
    out += f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    with open(path, "wb") as f:
        f.write(out)


# ---- the exceptions: (path prefix, why) ----
def exceptions_from(path):
    found = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if line.strip() and not line.startswith("#"):
                key, _, why = line.partition(" ")
                found.append((key, why))
    return found


def flatten(value, path="", out=None):
    out = {} if out is None else out
    if isinstance(value, dict):
        for key, inner in value.items():
            flatten(inner, f"{path}.{key}" if path else key, out)
    elif isinstance(value, list):
        if not value:
            out[path] = []
        for i, inner in enumerate(value):
            flatten(inner, f"{path}[{i}]", out)
    else:
        out[path] = value
    return out


def differs(a, b):
    if isinstance(a, (int, float)) and isinstance(b, (int, float)) and not isinstance(a, bool) and not isinstance(b, bool):
        return abs(a - b) > 1e-6
    return a != b


def compare(what, before, after, excepted, new_ok=True):
    """Every path of `before` is in `after` with the same value, but those excepted; paths only `after` has are new in the newer build."""
    one, two = flatten(before), flatten(after)
    is_excepted = lambda path: any(path.startswith(prefix) for prefix, _ in excepted)
    wrong = [f"{p}: {one[p]!r} then {two.get(p, '<nothing>')!r}" for p in sorted(one) if not is_excepted(p) and (p not in two or differs(one[p], two[p]))]
    new = sorted(p for p in two if p not in one and not is_excepted(p))
    if not new_ok:
        wrong += [f"{p}: nothing then {two[p]!r}" for p in new]
    if check(f"{what}: {len(one)} values compared, {len([p for p in one if is_excepted(p)])} excepted by name", not wrong, f"{len(wrong)} differ\n        " + "\n        ".join(wrong[:40])):
        pass
    return new


# ---- a quiet run ----
class Run:
    def __init__(self, app, label, state, pdfs):
        self.app, self.label, self.state = app, label, state
        self.recto = os.path.join(app, "Contents/MacOS/recto")
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("RECTO_")}
        self.env["RECTO_STATE_DIR"] = state
        self.pid = None

    def raw(self, *args, timeout=120):
        return subprocess.run([self.recto, *args, "--json"], env=self.env, capture_output=True, text=True, timeout=timeout)

    def cmd(self, *args, timeout=120):
        ran = self.raw(*args, timeout=timeout)
        try:
            reply = json.loads(ran.stdout)
        except json.JSONDecodeError:
            raise SystemExit(f"{self.label}: `recto {' '.join(args)}` gave no answer (exit {ran.returncode}): {ran.stdout[:300]} {ran.stderr[:300]}")
        if not reply.get("ok"):
            raise SystemExit(f"{self.label}: `recto {' '.join(args)}` was refused: {json.dumps(reply)[:400]}")
        return reply["data"]

    def start(self):
        data = self.cmd("qa", "start", "--app", os.path.join(self.app, "Contents/MacOS/RectoPDF"), "--time-limit", "10m", "--owner", str(os.getpid()), timeout=180)
        self.pid = data.get("pid")
        self.settled()
        return data

    def settled(self):
        return self.cmd("qa", "wait-settled", "--timeout", "60s", timeout=90)

    def alive(self):
        return self.pid is not None and subprocess.run(["kill", "-0", str(self.pid)], capture_output=True).returncode == 0

    def snapshot(self, tabs):
        snap = {
            "state": self.cmd("qa", "state"),
            "tabs": self.cmd("tabs"),
            "show": self.cmd("session", "show"),
            "recent": self.cmd("recent"),
            "view": self.cmd("view"),
            "permission": self.cmd("permission", "show"),
        }
        for t in tabs:
            snap[f"where #{t}"] = self.cmd("where", "--tab", f"#{t}")
            snap[f"edits #{t}"] = self.cmd("edits", "--tab", f"#{t}")
        return snap

    def handoff(self):
        data = self.cmd("qa", "handoff", "now")
        for _ in range(100):
            if not self.alive():
                break
            time.sleep(0.1)
        check(f"{self.label}: handed off as an update does, and ended", data.get("handed_off") is True and not self.alive(), json.dumps(data))
        return data


def save(name, value):
    with open(os.path.join(evidence, f"{name}.json"), "w", encoding="utf-8") as f:
        json.dump(value, f, indent=1, sort_keys=True)


def files_of(state):
    found = {}
    for root, _, names in os.walk(os.path.join(state, "session")):
        for name in names:
            path = os.path.join(root, name)
            with open(path, "rb") as f:
                found[os.path.relpath(path, state)] = f.read()
    return found


def sleep_format(data):
    """A sleep file's own format, from its meta: 0 for one written before the format was said (0.1.0's)."""
    try:
        header, rest = data.split(b"\n", 1)
        json.loads(header)
        meta, _ = json.JSONDecoder().raw_decode(rest[:1 << 20].decode("utf-8", errors="replace"))
    except (ValueError, IndexError):
        return None
    return meta.get("sleep_format", 0) if isinstance(meta, dict) else None


def journal_version(data):
    try:
        return json.loads(data.split(b" ", 1)[1].split(b"\n", 1)[0]).get("version")
    except (ValueError, IndexError):
        return None


def version_of(app):
    return subprocess.run(["/usr/libexec/PlistBuddy", "-c", "Print :CFBundleShortVersionString", os.path.join(app, "Contents/Info.plist")], capture_output=True, text=True).stdout.strip()


def main():
    os.makedirs(evidence, exist_ok=True)
    work = tempfile.mkdtemp(prefix="check-carry.")
    os.chmod(work, 0o700)
    apps = []
    try:
        old_app, new_copy = os.path.join(work, "old/RectoPDF.app"), os.path.join(work, "new/RectoPDF.app")
        subprocess.run(["ditto", "-x", "-k", previous_zip, os.path.dirname(old_app)], check=True)
        subprocess.run(["ditto", new_app, new_copy], check=True)
        apps = [old_app, new_copy]
        old_v, new_v = version_of(old_app), version_of(new_copy)
        say(f"check-carry: {old_v} (as published) then {new_v} ({new_app}), on one state folder in {work}")
        across = exceptions_from(os.path.join(repo, "tools/release/carry", f"{old_v}-to-{new_v}.txt"))
        restore_dir = os.path.join(repo, "crates/recto-cli/tests/restore/exceptions")
        restart = [e for name in sorted(os.listdir(restore_dir)) if name.endswith(".txt") for e in exceptions_from(os.path.join(restore_dir, name))]
        state_ex = restart + [(k, w) for k, w in across if not k.startswith(("fault:", "tabs.", "show.", "permission."))]
        pdfs = os.path.join(work, "pdfs")
        state = os.path.join(work, "state")
        os.makedirs(pdfs, mode=0o700)
        os.makedirs(state, mode=0o700)
        for name, count in (("alpha", 30), ("beta", 12), ("gamma", 5)):
            make_pdf(os.path.join(pdfs, f"{name}.pdf"), name, count)

        # ---- A: the previous release makes a session ----
        a = Run(old_app, f"A ({old_v})", state, pdfs)
        a.start()
        for name in ("alpha", "beta", "gamma"):
            a.cmd("open", os.path.join(pdfs, f"{name}.pdf"))
        a.settled()
        a.cmd("tap", "#1"); a.settled()
        a.cmd("goto", "17"); a.settled()
        a.cmd("page", "rotate", "3", "--by", "90")
        a.cmd("qa", "zoom", "1.4"); a.settled()
        a.cmd("tap", "#2"); a.settled()
        a.cmd("goto", "5"); a.settled()
        a.cmd("find", "marker")
        a.cmd("tap", "#3"); a.settled()
        a.cmd("goto", "4"); a.settled()
        a.cmd("session", "sleep", "--tab", "#1")
        a.cmd("session", "sleep", "--tab", "#2")
        a.settled()
        before = a.snapshot([1, 2, 3])
        save("A", before)
        a.handoff()
        a_files = files_of(state)
        old_sleeps = {p: sleep_format(d) for p, d in a_files.items() if os.path.basename(p).startswith("sleep-")}
        check(f"A: {old_v} wrote its record, a journal for the unsaved turn and its sleep files", any(p.startswith("session/record.") for p in a_files)
              and any(os.path.basename(p).startswith("journal-") for p in a_files) and len(old_sleeps) >= 2, ", ".join(sorted(a_files)))

        # A made fault, for proving this check fails on one (tools/release/check-carry.sh --seeded): never set in a real run.
        seed = os.environ.get("CHECK_CARRY_SEED", "")
        if seed == "journal":
            for p in a_files:
                if os.path.basename(p).startswith("journal-"):
                    os.remove(os.path.join(state, p))
        # ---- B: the new build on that state ----
        b = Run(new_copy, f"B ({new_v})", state, pdfs)
        b.start()
        if seed == "place":
            b.cmd("goto", "2"); b.settled()
        if seed == "find":
            b.cmd("find", "--close"); b.settled()
        after = b.snapshot([1, 2, 3])
        save("B", after)
        for key in ("tabs", "recent", "view") + tuple(f"{w} #{t}" for w in ("where", "edits") for t in (1, 2, 3)):
            compare(f"B: `{key}` as {old_v} left it", before[key], after[key], [(k[len(key) + 1:], w) for k, w in across if k.startswith(key + ".")], new_ok=False)
        compare("B: `session show` (tabs, places, zoom, Find's text, the panes) as it was", before["show"], after["show"],
                [("record.", "the record's own counters, of this run")] + [(k[5:], w) for k, w in across if k.startswith("show.")], new_ok=False)
        compare("B: `permission show` as it was", before["permission"], after["permission"], [(k[11:], w) for k, w in across if k.startswith("permission.")], new_ok=False)
        new_paths = compare("B: the whole of `qa state`, as the restore tests compare it", before["state"], after["state"], state_ex)
        note(f"B: {len(new_paths)} values of `qa state` are new in {new_v} and not compared (listed in {evidence}/B-new-paths.json)")
        save("B-new-paths", new_paths)
        # By design not carried: an older sleep file is never read, and goes at its first read; each tab that slept wakes once from its PDF.
        wakes = {}
        for t in (1, 2, 3):
            b.cmd("tap", f"#{t}"); b.settled()
            wakes[t] = b.cmd("qa", "state")["reader"]["last_wake"]
        save("B-wakes", wakes)
        check(f"B: by design, no tab woke from {old_v}'s sleep pictures (each from its PDF, once)",
              all(w.get("from_sleep") is False and w.get("sleep_pictures", 0) == 0 for w in wakes.values()), json.dumps(wakes))
        b_files = files_of(state)
        left = [p for p in old_sleeps if p in b_files and sleep_format(b_files[p]) == old_sleeps[p] and b_files[p] == a_files[p]]
        check(f"B: by design, {old_v}'s sleep files ({len(old_sleeps)}, format {sorted(set(old_sleeps.values()))}) are gone once read", not left, ", ".join(left))
        for t in (1, 2, 3):
            check(f"B: tab #{t} woke on its place", b.cmd("where", "--tab", f"#{t}")["page"] == before[f"where #{t}"]["page"])
        # What the previous release never wrote: a highlight (a journal of version 2) and a sleep file of this build's format.
        b.cmd("tap", "#3"); b.settled()
        b.cmd("highlight", "--page", "4", "--chars", "0-20")
        b.cmd("session", "sleep", "--tab", "#1")
        b.settled()
        b2 = b.snapshot([1, 2, 3])
        save("B2", b2)
        b.handoff()
        b_files = files_of(state)
        journals_b = {p: d for p, d in b_files.items() if os.path.basename(p).startswith("journal-")}
        v2 = [p for p, d in journals_b.items() if journal_version(d) == 2]
        check(f"B: {new_v} wrote the highlight's journal as version 2, and a sleep file of its own format",
              len(v2) == 1 and any(sleep_format(d) and sleep_format(d) >= 3 for p, d in b_files.items() if os.path.basename(p).startswith("sleep-")),
              f"journals {[(p, journal_version(d)) for p, d in journals_b.items()]}")

        # ---- C: the previous release again, on what the new build left ----
        c = Run(old_app, f"C ({old_v} again)", state, pdfs)
        c.start()
        went_back = c.snapshot([1, 2, 3])
        save("C", went_back)
        tabs_b, tabs_c = b2["tabs"]["tabs"], went_back["tabs"]["tabs"]
        check(f"C: {old_v} comes up on {new_v}'s state with every tab, in order", [t["untrusted"]["name"] for t in tabs_b] == [t["untrusted"]["name"] for t in tabs_c],
              f"{[t['untrusted']['name'] for t in tabs_b]} then {[t['untrusted']['name'] for t in tabs_c]}")
        check("C: every tab on its place, the active one as it was", [t["page"] for t in tabs_b] == [t["page"] for t in tabs_c] and b2["show"]["active"] == went_back["show"]["active"])
        v2_tab = next(t for t in tabs_b if t["untrusted"]["name"] == "gamma")["tab"]
        for t in tabs_c:
            if t["tab"] == v2_tab:
                # By design (docs/RELEASING.md, A bad release): a version-2 journal is left alone rather than read in part, and the tab says so.
                check(f"C: by design, the highlighted tab shows the file without its edits and says why ({(t['file_notice'] or {}).get('code')})",
                      t["edited"] is False and (t["file_notice"] or {}).get("code") == "edits_unreadable", json.dumps(t)[:300])
            else:
                twin = next(x for x in tabs_b if x["tab"] == t["tab"])
                check(f"C: tab #{t['tab']}'s edits as {new_v} left them", t["edited"] == twin["edited"] and went_back[f"edits #{t['tab']}"]["runs"] == b2[f"edits #{t['tab']}"]["runs"])
        c.cmd("tap", "#1"); c.settled()
        wake_c = c.cmd("qa", "state")["reader"]["last_wake"]
        note(f"C: {old_v} woke tab #1 {'from' if wake_c.get('from_sleep') else 'without'} {new_v}'s sleep file (format 3): {json.dumps(wake_c)}")
        check(f"C: {old_v} is still running after waking a tab from {new_v}'s files", c.alive())
        c.handoff()
        c_files = files_of(state)
        changed = [p for p in journals_b if c_files.get(p) != journals_b[p]]
        check(f"C: {old_v} left every journal {new_v} wrote as it was, byte for byte ({len(journals_b)})", not changed, ", ".join(changed))

        # ---- D: the new build again: nothing it made was lost ----
        d = Run(new_copy, f"D ({new_v} again)", state, pdfs)
        d.start()
        back = d.snapshot([1, 2, 3])
        back["highlights #3"] = d.cmd("highlights", "--tab", f"#{v2_tab}")
        save("D", back)
        check(f"D: the highlight {new_v} made is there again", len(back["highlights #3"]["edits"]) == 1, json.dumps(back["highlights #3"])[:300])
        check("D: the turned page is there again", back["edits #1"]["runs"] == before["edits #1"]["runs"])
        check("D: every tab on its place", [t["page"] for t in back["tabs"]["tabs"]] == [t["page"] for t in tabs_b])
        stale = [t for t in back["tabs"]["tabs"] if t.get("file_notice")]
        acknowledged = {k[len("fault:"):]: w for k, w in across if k.startswith("fault:")}
        for t in stale:
            code = t["file_notice"]["code"]
            if f"D.notice.{code}" in acknowledged:
                note(f"D: KNOWN FAULT, tab #{t['tab']} says `{code}` though its edits read back whole: {acknowledged[f'D.notice.{code}']}")
            else:
                fail(f"D: tab #{t['tab']} says `{code}` though its edits read back whole", json.dumps(t["file_notice"]))
        d.cmd("qa", "quit", "now")
    finally:
        for app in apps:
            subprocess.run([LSREGISTER, "-u", app], capture_output=True)
        shutil.rmtree(work, ignore_errors=True)
    say(f"check-carry: evidence in {evidence}")
    if failures:
        say(f"check-carry: {len(failures)} check(s) failed:")
        for f in failures:
            say("  " + " | ".join(f.splitlines()[:2]).replace("  ", " "))
        sys.exit(1)
    say(f"check-carry: what should carry from {old_v} to {new_v} did, and back again; {len(notes)} note(s) above.")


main()
