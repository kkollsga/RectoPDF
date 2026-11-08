#!/usr/bin/python3 -I
"""RectoPDF 24-hour soak: a quiet run of the release app driven through `recto` and its QA verbs only.

    tools/soak/soak.py DIR APP_BUNDLE CORPUS_DIR [HOURS]

Run it detached (nohup, at background priority: `nohup nice -n 15 taskpolicy -b /usr/bin/python3 -I tools/soak/soak.py ... &`), on a copy
of a release bundle outside the checkout, so a rebuild doesn't change it underneath. It isn't a build, so it stays out of the build queue.
At the end it runs tools/soak/verdict.py on its CSV into DIR/verdict.txt.

DIR gets samples.csv (one row a minute), driver.log (errors and an hourly summary), run.json (PIDs and paths), state/ (the run's
RECTO_STATE_DIR) and work/ (temp copies that edits are saved into). Nothing outside DIR is written, the corpus is only read.

The sampler measures the whole tree: the app and every process under it (render workers, indexer, headings helper), by
proc_pid_rusage's phys_footprint, proc_pidinfo's thread count and descriptor list. No posted events: every action is a `recto`
command, as an agent would send it.
"""
import ctypes
import json
import os
import random
import shutil
import signal
import subprocess
import sys
import threading
import time

DIR, BUNDLE, CORPUS = sys.argv[1], sys.argv[2], sys.argv[3]
HOURS = float(sys.argv[4]) if len(sys.argv) > 4 else 24.0
RECTO = os.path.join(BUNDLE, "Contents/MacOS/recto")
STATE = os.path.join(DIR, "state")
WORK = os.path.join(DIR, "work")
CSV = os.path.join(DIR, "samples.csv")
LOG = os.path.join(DIR, "driver.log")
RUN = os.path.join(DIR, "run.json")
SAMPLE_EVERY = float(os.environ.get("SOAK_SAMPLE_S", "60"))
# The app's own limit is at most 24 h (recto's longest wait): the drive stops a few minutes before it.
APP_LIMIT_H = 24
DRIVE_S = min(HOURS * 3600, APP_LIMIT_H * 3600 - 300)
MAX_TABS = 8
WORDS = ["the", "data", "model", "results", "figure", "table", "section", "water", "shall", "income"]

libc = ctypes.CDLL("/usr/lib/libSystem.dylib", use_errno=True)
libproc = libc
RUSAGE_INFO_V4 = 4
PROC_PIDTASKINFO = 4
PROC_PIDLISTFDS = 1


def footprint(pid):
    """(phys_footprint bytes, cpu ns, idle wakeups) or None if gone."""
    buf = ctypes.create_string_buffer(512)
    if libproc.proc_pid_rusage(ctypes.c_int(pid), ctypes.c_int(RUSAGE_INFO_V4), buf) != 0:
        return None
    u64 = lambda off: int.from_bytes(buf.raw[off:off + 8], "little")
    # rusage_info_v4: uuid[16], user_time, system_time, pkg_idle_wkups, interrupt_wkups, pageins, wired, resident, phys_footprint
    return u64(72), u64(16) + u64(24), u64(32) + u64(40)


def threads(pid):
    buf = ctypes.create_string_buffer(256)
    n = libproc.proc_pidinfo(ctypes.c_int(pid), ctypes.c_int(PROC_PIDTASKINFO), ctypes.c_uint64(0), buf, ctypes.c_int(256))
    if n <= 0:
        return 0
    return int.from_bytes(buf.raw[84:88], "little")


def fds(pid):
    size = 8 * 4096
    buf = ctypes.create_string_buffer(size)
    n = libproc.proc_pidinfo(ctypes.c_int(pid), ctypes.c_int(PROC_PIDLISTFDS), ctypes.c_uint64(0), buf, ctypes.c_int(size))
    return max(n, 0) // 8


def descendants(root):
    try:
        out = subprocess.run(["/bin/ps", "-A", "-o", "pid=,ppid=,comm="], capture_output=True, text=True, timeout=20).stdout
    except Exception:
        return []
    kids, names = {}, {}
    for line in out.splitlines():
        parts = line.split(None, 2)
        if len(parts) < 2:
            continue
        pid, ppid = int(parts[0]), int(parts[1])
        names[pid] = parts[2] if len(parts) > 2 else ""
        kids.setdefault(ppid, []).append(pid)
    found, todo = [], [root]
    while todo:
        p = todo.pop()
        for c in kids.get(p, []):
            found.append((c, os.path.basename(names.get(c, ""))))
            todo.append(c)
    return found


def tab_name(t):
    return ((t.get("untrusted") or {}).get("name") or "")


def du_kib(path):
    try:
        out = subprocess.run(["/usr/bin/du", "-sk", path], capture_output=True, text=True, timeout=60).stdout
        return int(out.split()[0])
    except Exception:
        return ""


class Soak:
    def __init__(self):
        self.lock = threading.Lock()
        self.app_pid = None
        self.run_id = 0
        self.rounds = 0
        self.actions = 0
        self.errors = 0
        self.timeouts = 0
        self.phase = "starting"
        self.tabs_open = 0
        self.saves = 0
        self.started = time.time()
        self.stop = False
        self.err_kinds = {}
        self.env = dict(os.environ, RECTO_STATE_DIR=STATE)
        self.env.pop("RECTO_BIN", None)

    def log(self, text):
        with open(LOG, "a") as f:
            f.write(time.strftime("%Y-%m-%d %H:%M:%S ") + text + "\n")

    # --- the app ---------------------------------------------------------------------------------------------------------
    def recto(self, *args, timeout=150, quiet_errors=False):
        self.actions += 1
        try:
            p = subprocess.run([RECTO, *args, "--json"], capture_output=True, text=True, timeout=timeout, env=self.env, cwd=DIR)
        except subprocess.TimeoutExpired:
            self.timeouts += 1
            self.errors += 1
            self._kind("timeout:" + args[0] + (" " + args[1] if args[0] == "qa" and len(args) > 1 else ""))
            self.log(f"timeout: {' '.join(args)}")
            return None
        try:
            value = json.loads(p.stdout) if p.stdout.strip() else {}
        except ValueError:
            value = {}
        if p.returncode != 0 or value.get("ok") is False:
            code = (value.get("error") or {}).get("code", f"exit{p.returncode}")
            if not quiet_errors:
                self.errors += 1
                self._kind(f"{args[0]}{(' ' + args[1]) if args[0] in ('qa', 'page', 'session', 'wait') and len(args) > 1 else ''}:{code}")
                msg = (value.get("error") or {}).get("message", p.stderr.strip()[:200])
                self.log(f"error {code}: {' '.join(args)[:200]} -> {msg[:200]}")
            return None
        return value.get("data", value)

    def _kind(self, k):
        self.err_kinds[k] = self.err_kinds.get(k, 0) + 1

    def settle(self, t="60s"):
        return self.recto("qa", "wait-settled", "--timeout", t, timeout=90)

    def start_app(self):
        self.run_id += 1
        left_h = max(1, int((self.started + DRIVE_S + 300 - time.time()) / 3600) + 1)
        limit = f"{min(APP_LIMIT_H, left_h)}h"
        data = self.recto("qa", "start", "--time-limit", limit, timeout=60)
        if not data:
            return False
        self.app_pid = int(data["pid"])
        self.write_run()
        self.log(f"app started, run {self.run_id}, pid {self.app_pid}, limit {limit}")
        self.recto("qa", "scroller-style", "overlay")
        # Tabs left alone for two minutes go dormant by themselves, as five minutes would for a person.
        self.recto("qa", "dormant-after", "120")
        return True

    def alive(self):
        return self.app_pid is not None and footprint(self.app_pid) is not None

    def write_run(self):
        info = {
            "driver_pid": os.getpid(),
            "app_pid": self.app_pid,
            "run_id": self.run_id,
            "started": time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime(self.started)),
            "ends_by": time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime(self.started + DRIVE_S + 300)),
            "bundle": BUNDLE,
            "corpus": CORPUS,
            "csv": CSV,
            "log": LOG,
            "state": STATE,
            "verdict": f"/usr/bin/python3 -I {os.path.join(os.path.dirname(os.path.abspath(__file__)), 'verdict.py')} {CSV}",
        }
        with open(RUN + ".tmp", "w") as f:
            json.dump(info, f, indent=1)
        os.replace(RUN + ".tmp", RUN)

    # --- sampling --------------------------------------------------------------------------------------------------------
    def sampler(self):
        header = ("time,elapsed_s,run_id,phase,app_pid,app_mib,children_mib,total_mib,children,app_threads,total_threads,"
                  "app_fds,total_fds,app_cpu_s,app_wakeups,state_kib,tabs_open,rounds,actions,errors,timeouts,saves,children_names")
        new = not os.path.exists(CSV)
        with open(CSV, "a") as f:
            if new:
                f.write(header + "\n")
        n = 0
        next_at = time.time()
        while not self.stop:
            next_at += SAMPLE_EVERY
            pid = self.app_pid
            row = None
            fp = footprint(pid) if pid else None
            if fp:
                kids = descendants(pid)
                kid_fp = 0
                kid_thr = 0
                kid_fd = 0
                names = []
                for c, name in kids:
                    k = footprint(c)
                    if k:
                        kid_fp += k[0]
                        kid_thr += threads(c)
                        kid_fd += fds(c)
                        names.append(f"{name}:{k[0] / 1048576:.1f}")
                at, af = threads(pid), fds(pid)
                state = du_kib(STATE) if n % 10 == 0 else ""
                row = [time.strftime("%Y-%m-%dT%H:%M:%S"), int(time.time() - self.started), self.run_id, self.phase, pid,
                       f"{fp[0] / 1048576:.2f}", f"{kid_fp / 1048576:.2f}", f"{(fp[0] + kid_fp) / 1048576:.2f}", len(kids), at,
                       at + kid_thr, af, af + kid_fd, f"{fp[1] / 1e9:.2f}", fp[2], state, self.tabs_open, self.rounds, self.actions,
                       self.errors, self.timeouts, self.saves, " ".join(names)]
            else:
                row = [time.strftime("%Y-%m-%dT%H:%M:%S"), int(time.time() - self.started), self.run_id, "app_gone", pid or "",
                       "", "", "", "", "", "", "", "", "", "", "", self.tabs_open, self.rounds, self.actions, self.errors,
                       self.timeouts, self.saves, ""]
            with open(CSV, "a") as f:
                f.write(",".join(str(x) for x in row) + "\n")
            n += 1
            time.sleep(max(1.0, next_at - time.time()))

    # --- the drive -------------------------------------------------------------------------------------------------------
    def tabs(self):
        data = self.recto("tabs", "--limit", "200")
        if not data:
            return []
        return data.get("tabs", [])

    def drive(self):
        pdfs = sorted(os.path.join(CORPUS, f) for f in os.listdir(CORPUS) if f.lower().endswith(".pdf"))
        # The edit document: a copy of a small text PDF, saved into over and over, and copied fresh every 25 saves.
        small = [p for p in pdfs if 200_000 < os.path.getsize(p) < 3_000_000 and "ietf" in p] or pdfs[:1]
        edit_src = small[0]
        os.makedirs(WORK, exist_ok=True)
        edit_path = os.path.join(WORK, "edit-copy.pdf")
        shutil.copyfile(edit_src, edit_path)
        cursor = 0
        rng = random.Random(20261108)
        last_hour = int(time.time() - self.started) // 3600
        last_idle_hour = -1
        saves_on_copy = 0

        def ensure_app():
            if self.alive():
                return True
            if self.app_pid is not None:
                self.log(f"app pid {self.app_pid} is gone; restarting (counts against the soak)")
            self.phase = "starting"
            ok = self.start_app()
            if ok:
                self.recto("open", edit_path)
            return ok

        if not ensure_app():
            self.log("the app wouldn't start; giving up")
            return
        while time.time() - self.started < DRIVE_S and not self.stop:
            if not ensure_app():
                time.sleep(60)
                continue
            hour = int(time.time() - self.started) // 3600
            if hour != last_hour:
                self.log(f"hour {hour}: rounds {self.rounds}, actions {self.actions}, errors {self.errors}, timeouts {self.timeouts}, "
                         f"saves {self.saves}, runs {self.run_id}, kinds {json.dumps(self.err_kinds, sort_keys=True)}")
                last_hour = hour
            # Five minutes at rest once an hour, at minute 30: the idle floor.
            if (int(time.time() - self.started) % 3600) > 1800 and last_idle_hour != hour:
                last_idle_hour = hour
                self.phase = "idle"
                time.sleep(300)
                continue
            self.phase = "active"
            self.rounds += 1
            r = self.rounds
            tabs = self.tabs()
            self.tabs_open = len(tabs)
            # Open the next corpus file, closing the oldest plain tab past MAX_TABS: every file comes round in turn.
            path = pdfs[cursor % len(pdfs)]
            cursor += 1
            if len(tabs) >= MAX_TABS:
                for t in tabs:
                    if tab_name(t) != "edit-copy.pdf" and not t.get("edited") and not t.get("active"):
                        self.recto("close", "--tab", t.get("sel") or "#1")
                        break
            self.recto("open", path, timeout=180)
            self.settle()
            tabs = self.tabs()
            self.tabs_open = len(tabs)
            n = max(1, len(tabs))
            # Read this one: pages, scrolling, zoom.
            pages = 1
            for t in tabs:
                if tab_name(t) == os.path.basename(path):
                    pages = int(t.get("pages") or 1)
            self.recto("goto", str(rng.randint(1, max(1, pages))))
            self.settle()
            for _ in range(rng.randint(5, 20)):
                self.recto("qa", "scroll", str(rng.choice([300, 600, 900, -300])))
            self.settle()
            z = rng.choice(["0.5", "1.5", "2", "3"])
            self.recto("qa", "zoom", z)
            self.settle()
            for _ in range(4):
                self.recto("qa", "scroll", str(rng.choice([400, -400])))
            self.recto("qa", "zoom", "1")
            self.settle()
            # Switch to another tab, which may be dormant: it wakes.
            sels = [t.get("sel") for t in tabs if t.get("sel")] or ["#1"]
            self.recto("tap", rng.choice(sels))
            self.settle()
            if r % 3 == 0:
                self.recto("find", rng.choice(WORDS), "--limit", "5", timeout=60)
                self.recto("find", "--next")
                self.recto("find", "--next")
                self.settle()
                self.recto("find", "--close")
            if r % 4 == 0:
                # Copy in the window: a drag over the page showing, then Edit > Copy. And an agent's copy of a page.
                self.recto("goto", "1")
                self.settle()
                self.recto("qa", "mouse-down", "1:60,80")
                self.recto("qa", "mouse-drag", "1:500,600")
                self.recto("qa", "mouse-up")
                self.settle()
                self.recto("qa", "menu", "edit.copy")
                self.settle()
                self.recto("qa", "copied", quiet_errors=True)
                self.recto("qa", "mouse-down", "1:5,5")
                self.recto("qa", "mouse-up")
                self.recto("copy", "1", timeout=120, quiet_errors=True)
            if r % 5 == 0 and n >= 2:
                a, b = rng.sample(sels, 2) if len(sels) >= 2 else (sels[0], sels[0])
                self.recto("compare", "--tabs", f"{a},{b}")
                self.settle()
                for _ in range(5):
                    self.recto("qa", "scroll", "500")
                self.recto("compare", "--sync", "on")
                for _ in range(5):
                    self.recto("qa", "scroll", "500")
                self.settle()
                self.recto("compare", "--off")
                self.settle()
            if r % 6 == 0:
                # Edit and save into the temp copy, with an undo and redo on the way.
                self.recto("open", edit_path)
                self.settle()
                self.recto("page", "rotate", str(rng.randint(1, 3)), "--by", "90")
                self.recto("undo")
                self.recto("redo")
                if self.recto("save", timeout=120) is not None:
                    self.saves += 1
                    saves_on_copy += 1
                self.settle()
                if saves_on_copy >= 25:
                    self.recto("close")
                    shutil.copyfile(edit_src, edit_path)
                    saves_on_copy = 0
                    self.recto("open", edit_path)
                    self.settle()
            if r % 7 == 0 and n >= 2:
                # Put another tab to sleep now, then come back to it later by tapping.
                self.recto("session", "sleep", "--tab", rng.choice(sels), quiet_errors=True)
            if r % 50 == 0:
                # A whole-document Select All and copy on whatever's showing, to its caps.
                self.recto("qa", "menu", "edit.select_all")
                self.recto("qa", "menu", "edit.copy")
                self.settle("150s")
                self.recto("qa", "mouse-down", "1:5,5")
                self.recto("qa", "mouse-up")
            self.phase = "rest"
            time.sleep(rng.uniform(5, 40))
        self.phase = "ending"
        self.log(f"drive done: rounds {self.rounds}, actions {self.actions}, errors {self.errors}, timeouts {self.timeouts}, saves {self.saves}, "
                 f"runs {self.run_id}, kinds {json.dumps(self.err_kinds, sort_keys=True)}")
        # Ten minutes at rest at the end, then stop.
        self.phase = "idle"
        time.sleep(min(600, DRIVE_S / 6))
        self.recto("stop", quiet_errors=True)
        time.sleep(5)
        self.stop = True


def main():
    os.makedirs(DIR, exist_ok=True)
    try:
        os.setsid()  # Its own session, so the shell or agent session that started it ending doesn't take it along.
    except OSError:
        pass
    s = Soak()

    def on_term(*_):
        s.stop = True
        s.log("driver told to stop")
        try:
            subprocess.run([RECTO, "stop"], env=s.env, timeout=30, capture_output=True)
        except Exception:
            pass
        sys.exit(0)

    signal.signal(signal.SIGTERM, on_term)
    s.write_run()
    s.log(f"soak starting: {HOURS} h, bundle {BUNDLE}, corpus {CORPUS}")
    t = threading.Thread(target=s.sampler, daemon=True)
    t.start()
    try:
        s.drive()
    except Exception as e:
        import traceback
        s.log("driver crashed: " + traceback.format_exc())
        s.phase = "driver_crashed"
        # Keep sampling the app until its own limit ends it, so the memory record goes on.
        while s.alive():
            time.sleep(60)
    s.stop = True
    time.sleep(2)
    try:
        out = subprocess.run(["/usr/bin/python3", "-I", os.path.join(os.path.dirname(os.path.abspath(__file__)), "verdict.py"), CSV],
                             capture_output=True, text=True, timeout=120)
        with open(os.path.join(DIR, "verdict.txt"), "w") as f:
            f.write(out.stdout + out.stderr)
    except Exception as e:
        s.log(f"verdict failed: {e}")
    s.log("soak finished")


if __name__ == "__main__":
    main()
