#!/usr/bin/env -S python3 -I
"""corpus_run: runs `recto` over every PDF in a folder and records what each step did, its time and its peak memory.

    tools/corpus_run.py FOLDER OUT.jsonl [--recto PATH]

Run it through tools/sysguard/heavy.sh. Each step is its own `recto local FILE ...` process, on a throwaway state folder that a quiet
run has set to Full (as `recto qa start` does for any quiet run), so nothing touches the real state: facts, render of the first and
last page, text of the first and last page, a search for a word no page has (so every page is read), and an SVG export of page 1. Every output goes to a temp
folder that's removed at the end.

Peak memory is the whole tree of each step: `recto` and every worker it starts, polled every 5 ms through proc_pid_rusage.
`tree_mib` is the largest sum of their footprints at one poll, so a spike shorter than a poll can be missed; `largest_mib` is the
largest lifetime peak (ri_lifetime_max_phys_footprint) of any one process, which a poll can't miss once the process was seen, and
`recto_mib` that of `recto` itself. (wait4's peak resident size isn't used: on macOS it takes in the reaped workers' too.)
A worker that lives under 5 ms can be missed.
"""
import ctypes
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
libc = ctypes.CDLL("/usr/lib/libSystem.dylib")


def footprints(pid):
    """(phys_footprint, lifetime peak footprint) in bytes, or None once the process is gone."""
    buf = ctypes.create_string_buffer(512)
    if libc.proc_pid_rusage(ctypes.c_int(pid), ctypes.c_int(4), buf) != 0:
        return None
    # rusage_info_v4: a 16-byte uuid, then u64 fields; ri_phys_footprint is the 8th and ri_lifetime_max_phys_footprint the 29th.
    field = lambda n: int.from_bytes(buf.raw[16 + n * 8:16 + (n + 1) * 8], "little")
    return field(7), field(28)


def children(pid):
    buf = (ctypes.c_int * 256)()
    n = libc.proc_listchildpids(ctypes.c_int(pid), buf, ctypes.c_int(ctypes.sizeof(buf)))
    return [buf[i] for i in range(max(0, min(n, 256)))]


def run(recto, env, args, timeout=600):
    """(exit code, answer, seconds, recto's peak MiB, tree's peak MiB, largest process's peak MiB, workers seen)."""
    started = time.monotonic()
    proc = subprocess.Popen([recto, *args, "--json"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, text=True)
    peaks, tree, done = {}, [0], threading.Event()

    def watch():
        while not done.is_set():
            todo, now = [proc.pid], 0
            while todo:
                pid = todo.pop()
                seen = footprints(pid)
                if seen is not None:
                    now += seen[0]
                    peaks[pid] = max(peaks.get(pid, 0), seen[1])
                    todo.extend(children(pid))
            tree[0] = max(tree[0], now)
            done.wait(0.005)

    watcher = threading.Thread(target=watch, daemon=True)
    watcher.start()
    # Both pipes are read on threads, and the process reaped by wait4.
    got = {}
    readers = [threading.Thread(target=lambda k=k, f=f: got.__setitem__(k, f.read()), daemon=True) for k, f in (("out", proc.stdout), ("err", proc.stderr))]
    for reader in readers:
        reader.start()
    killer = threading.Timer(timeout, proc.kill)
    killer.start()
    _, status, _ = os.wait4(proc.pid, 0)
    killer.cancel()
    proc.returncode = os.waitstatus_to_exitcode(status)
    for reader in readers:
        reader.join()
    done.set()
    watcher.join()
    seconds = time.monotonic() - started
    if seconds >= timeout:
        return 124, {"error": {"code": "timed_out_here"}}, seconds, 0, 0, 0, 0
    out = got.get("out") or ""
    try:
        answer = json.loads(out) if out.strip() else {}
    except ValueError:
        answer = {"error": {"code": "not_json"}}
    own = peaks.get(proc.pid, 0)
    largest = max(peaks.values(), default=0)
    return proc.returncode, answer, seconds, own / 1048576, max(tree[0], largest) / 1048576, largest / 1048576, len(peaks) - 1


def clone_into(path, folder):
    """A copy of a file in the run's own folder, as a quiet run's session opens nothing outside the temp folders (2026-11-13): an APFS clone where the
    volume allows, which costs nothing, and a plain copy across volumes."""
    copy = os.path.join(folder, os.path.basename(path))
    if subprocess.run(["cp", "-c", path, copy], capture_output=True).returncode != 0:
        shutil.copyfile(path, copy)
    return copy


def main():
    folder, out_path = sys.argv[1], sys.argv[2]
    recto = sys.argv[sys.argv.index("--recto") + 1] if "--recto" in sys.argv else os.path.join(ROOT, "target/release/recto")
    app = os.path.join(ROOT, "app/build.noindex/RectoPDF.app/Contents/MacOS/RectoPDF")
    work = tempfile.mkdtemp(prefix="recto-corpus-")
    # The quiet run ends with this script, however it ends.
    env = dict(os.environ, RECTO_STATE_DIR=os.path.join(work, "state"), RECTO_QA_OWNER_PID=str(os.getpid()))
    try:
        # A quiet run sets its throwaway state folder to Full; `recto local` then runs on that folder with no app.
        subprocess.run([recto, "qa", "start", "--app", app, "--time-limit", "1m"], env=env, check=True, capture_output=True)
        subprocess.run([recto, "stop"], env=env, capture_output=True)
        pdfs = sorted(f for f in os.listdir(folder) if f.lower().endswith(".pdf"))
        with open(out_path, "w") as out:
            for n, name in enumerate(pdfs):
                path = clone_into(os.path.join(folder, name), work)
                row = {"file": name, "bytes": os.path.getsize(path), "steps": {}}

                def step(key, *args, timeout=600):
                    code, answer, seconds, own, tree, largest, workers = run(recto, env, ["local", path, *args], timeout)
                    error = answer.get("error") or {}
                    row["steps"][key] = {
                        "exit": code,
                        "ok": code == 0 and answer.get("ok") is not False,
                        "code": error.get("code"),
                        "message": (error.get("message") or "")[:300],
                        "s": round(seconds, 3),
                        "recto_mib": round(own, 1),
                        "tree_mib": round(tree, 1),
                        "largest_mib": round(largest, 1),
                        "workers": workers,
                    }
                    return answer.get("data") or {}

                facts = step("facts", "facts")
                pages = int(facts.get("pages") or 0)
                row["pages"] = pages
                row["structure"] = facts.get("structure")
                row["encrypted"] = facts.get("encrypted")
                if pages:
                    first = step("render_first", "render", "1", "--out", os.path.join(work, f"{n}-first.png"))
                    row["render_first_kind"] = first.get("kind")
                    row["render_first_scale"] = first.get("scale")
                    last = step("render_last", "render", str(pages), "--out", os.path.join(work, f"{n}-last.png"))
                    row["render_last_scale"] = last.get("scale")
                    t1 = step("text_first", "text", "--page", "1")
                    row["text_first_chars"] = t1.get("chars")
                    row["hidden_first"] = (t1.get("hidden_text") or {}).get("flagged")
                    tl = step("text_last", "text", "--page", str(pages))
                    row["text_last_chars"] = tl.get("chars")
                    # A word no page has, so every page is read.
                    found = step("search", "search", "zqxjvw", "--limit", "5")
                    coverage = found.get("coverage") or {}
                    row["search_pages"] = coverage.get("pages_searched")
                    row["search_failed_pages"] = len(coverage.get("failed_pages") or [])
                    row["search_no_text_pages"] = coverage.get("pages_with_no_text")
                    row["search_hits"] = coverage.get("hits_total")
                    svg = step("svg_first", "export", "svg", "1", "--out", os.path.join(work, f"{n}-first.svg"))
                    row["svg_bytes"] = svg.get("bytes")
                    row["svg_all_vector"] = svg.get("all_vector")
                    for f in os.listdir(work):
                        if f.startswith(f"{n}-"):
                            os.remove(os.path.join(work, f))
                os.remove(path)
                out.write(json.dumps(row) + "\n")
                out.flush()
                bad = [k for k, v in row["steps"].items() if not v["ok"]]
                print(f"{n + 1}/{len(pdfs)} {name}: {pages} pages" + (f", not ok: {', '.join(bad)}" if bad else ""), flush=True)
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
