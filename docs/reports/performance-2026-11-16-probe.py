#!/usr/bin/env python3
"""Probes for docs/reports/performance-2026-11-16.md: what `qa budget`'s cold start and sidebar times are made of.

`qa budget` times a cold start and the sidebar coming back to `qa wait-settled`, which also waits for the window's animations to end
(`app.animations`: the chrome's marks fading, the contents' fade and slide). These poll `qa state` instead, each call a `recto` process
(about 10 to 30 ms apart, so each time is to within one poll), and say when the page was drawn and when the last animation ended, apart.

    python3 -I performance-2026-11-16-probe.py coldstart RECTO APP PDF RUNS OUT.json
    python3 -I performance-2026-11-16-probe.py sidebar RECTO APP PDF RUNS OUT.json [TABS]     (TABS tabs of clones of PDF, 2 if left out; the sidebar hidden, then shown, RUNS times)

Each run's state folder is a fresh temp folder; RECTO_QA_OWNER_PID is this process, so a quiet run ends with it.
"""
import json, os, shutil, subprocess, sys, tempfile, time

mode, recto, app, pdf, runs, out = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], int(sys.argv[5]), sys.argv[6]
env = dict(os.environ, RECTO_QA_OWNER_PID=str(os.getpid()))


def call(*words):
    done = subprocess.run([recto, *words, "--json"], env=env, capture_output=True, text=True)
    try:
        reply = json.loads(done.stdout)
    except ValueError:
        return None
    return reply.get("data") if reply.get("ok") else None


def look():
    data = call("qa", "state")
    if data is None:
        return None
    reader = data.get("reader") or {}
    return {
        "drawn": bool(reader.get("settled")) and (reader.get("tiles") or {}).get("attached", 0) > 0,
        "animations": (data.get("app") or {}).get("animations") or [],
        "first_shown_ms_from_start": ((data.get("record") or {}).get("first_shown") or {}).get("ms_from_start"),
    }


def follow(began, limit=8.0):
    """Polls until the page is drawn and nothing is animating; when each first held, and every animation seen."""
    drawn = quiet = None
    seen = set()
    first_shown = None
    while time.monotonic() - began < limit:
        now = look()
        at = (time.monotonic() - began) * 1000
        if now is None:
            continue
        seen.update(now["animations"])
        first_shown = now["first_shown_ms_from_start"] or first_shown
        if drawn is None and now["drawn"]:
            drawn = at
        if drawn is not None and not now["animations"]:
            quiet = at
            break
    return {"drawn_ms": drawn, "drawn_and_still_ms": quiet, "animations_seen": sorted(seen), "first_shown_ms_from_start": first_shown}


results = []
if mode == "coldstart":
    for _ in range(runs):
        root = tempfile.mkdtemp()
        env["RECTO_STATE_DIR"] = os.path.join(root, "s")
        copy = os.path.join(root, "doc.pdf")
        shutil.copyfile(pdf, copy)
        # The state folder made as `qa start` makes it (the level, the folders), and the app ended again, so the launch below is the app's first with a file.
        call("qa", "start", "--app", app)
        call("qa", "quit", "now")
        time.sleep(1)
        began = time.monotonic()
        # As `qa budget` starts it (`launch` in recto-cli's qa.rs): the app with --qa and the file, as a double click opens it, and its ready line on stdout.
        child = subprocess.Popen(
            [app, "--qa", "--qa-owner-pid", str(os.getpid()), copy],
            env=dict(env, MallocSpaceEfficient="1", MallocMaxMagazines="1"),
            cwd=env["RECTO_STATE_DIR"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        child.stdout.readline()
        ready = (time.monotonic() - began) * 1000
        one = follow(began)
        settle = call("qa", "wait-settled", "--timeout", "30s")
        one.update({"answering_ms": ready, "wait_settled_after": settle})
        results.append(one)
        call("qa", "quit", "now")
        try:
            child.wait(timeout=30)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
        shutil.rmtree(root, ignore_errors=True)
elif mode == "sidebar":
    root = tempfile.mkdtemp()
    env["RECTO_STATE_DIR"] = os.path.join(root, "s")
    call("qa", "start", "--app", app)
    tabs = int(sys.argv[7]) if len(sys.argv) > 7 else 2
    for at in range(tabs):
        copy = os.path.join(root, f"t{at}.pdf")
        subprocess.run(["cp", "-c", pdf, copy], check=True)
        call("open", copy)
    call("qa", "wait-settled", "--timeout", "60s")
    for _ in range(runs):
        call("view", "--sidebar", "off")
        call("qa", "wait-settled", "--timeout", "60s")
        began = time.monotonic()
        call("view", "--sidebar", "on")
        answered = (time.monotonic() - began) * 1000
        one = follow(began)
        one["command_answered_ms"] = answered
        one["tabs_open"] = len(((call("qa", "state") or {}).get("session") or {}).get("tabs") or [])
        results.append(one)
    call("qa", "quit", "now")
    shutil.rmtree(root, ignore_errors=True)
elif mode == "restore":
    # A start from the record with PDF's tab in the middle of the file, with an edit (a page turned) or not, as `qa budget`'s hand-off has it:
    # `restore RECTO APP PDF RUNS OUT.json edit|plain [SAMPLE_OUT]`. The first run is sampled (`sample`, 3 s from 0.3 s after the launch) if asked.
    # `edit` or `plain` ends the first app with a quit; `handoff-edit` with the hand-off an update makes (`qa handoff now`), after `qa scroll 120`, as `qa budget` does.
    edit = sys.argv[7].endswith("edit")
    handoff = sys.argv[7].startswith("handoff")
    sample_out = sys.argv[8] if len(sys.argv) > 8 else None
    for run in range(runs):
        root = tempfile.mkdtemp()
        env["RECTO_STATE_DIR"] = os.path.join(root, "s")
        copy = os.path.join(root, "doc.pdf")
        subprocess.run(["cp", "-c", pdf, copy], check=True)
        call("qa", "start", "--app", app)
        call("open", copy)
        call("qa", "wait-settled", "--timeout", "60s")
        pages = (call("qa", "state") or {}).get("reader", {}).get("pages", 2)
        call("goto", str(max(pages // 2, 1)))
        call("qa", "wait-settled", "--timeout", "60s")
        if handoff:
            call("qa", "scroll", "120")
            call("qa", "wait-settled", "--timeout", "60s")
        if edit:
            call("qa", "as-person", "page rotate 1 --by 90")
            call("qa", "wait-settled", "--timeout", "60s")
        # `qa handoff now` is what `qa budget` sends (flush, read back, quit); the app is waited out before the next starts.
        call("qa", "handoff" if handoff else "quit", "now")
        gone = time.monotonic()
        while call("qa", "state") is not None and time.monotonic() - gone < 15:
            time.sleep(0.05)
        time.sleep(0.5)
        began = time.monotonic()
        child = subprocess.Popen(
            [app, "--qa", "--qa-owner-pid", str(os.getpid())],
            env=dict(env, MallocSpaceEfficient="1", MallocMaxMagazines="1"),
            cwd=env["RECTO_STATE_DIR"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        sampler = None
        if sample_out and run == 0:
            time.sleep(0.3)
            sampler = subprocess.Popen(["sample", str(child.pid), "3", "1", "-file", sample_out], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        child.stdout.readline()
        ready = (time.monotonic() - began) * 1000
        one = follow(began, limit=20.0)
        # Then `qa wait-settled`, as `qa budget` waits, in rounds of half a second, keeping what each round that ran out said was still under way.
        waits = []
        while time.monotonic() - began < 20:
            done = subprocess.run([recto, "qa", "wait-settled", "--timeout", "500ms", "--json"], env=env, capture_output=True, text=True)
            reply = json.loads(done.stdout or "{}")
            at = round((time.monotonic() - began) * 1000)
            if reply.get("ok"):
                waits.append({"at_ms": at, "settled": reply.get("data")})
                break
            memory = (call("qa", "state") or {}).get("memory") or {}
            waits.append({"at_ms": at, "said": (reply.get("error") or {}).get("message"), "app_mib": round(memory.get("footprint_bytes", 0) / 1048576, 1), "threads": memory.get("threads")})
        one["wait_settled"] = waits
        state = call("qa", "state") or {}
        one.update({"answering_ms": ready, "edit": edit, "handoff": handoff, "pages": pages, "memory": state.get("memory"), "last_wake": (state.get("reader") or {}).get("last_wake")})
        results.append(one)
        if sampler:
            sampler.wait()
        call("qa", "quit", "now")
        try:
            child.wait(timeout=30)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
        shutil.rmtree(root, ignore_errors=True)
json.dump(results, open(out, "w"), indent=1)
