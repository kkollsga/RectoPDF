"""Retire this carry check's private apps before forgetting or removing them.

Application PIDs are observations only. Quit uses the existing Run.raw client.
The only child processes started here query the OS; none launches an app.
"""
import ctypes
import errno
import json
import os
from pathlib import Path
import plistlib
import selectors
import shutil
import subprocess
import sys
import time

LSREGISTER = '/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister'
MAX_PIDS = 8192
MAX_PATH = 4096
MAX_OUTPUT = 8192


def describe(work, apps):
    root = Path(work).resolve()
    bundles = []
    if len(apps) > 2:
        raise RuntimeError('carry cleanup expects at most two private apps')
    paths = []
    for app in apps:
        paths.append(Path(app))
        helper = Path(app) / 'Contents/Helpers/RectoRender.app'
        if helper.is_dir():
            paths.append(helper)
    for app in paths:  # At most four exact known URLs; never a recursive app sweep.
        path = Path(app).resolve()
        if not path.is_relative_to(root) or path == root:
            raise RuntimeError('cleanup bundle is outside this private work folder')
        plist = path / 'Contents/Info.plist'
        if plist.stat().st_size > 1024 * 1024:
            raise RuntimeError('cleanup bundle plist exceeds its bound')
        identifier = plistlib.loads(plist.read_bytes()).get('CFBundleIdentifier')
        if not isinstance(identifier, str) or not identifier or len(identifier.encode()) > 1024:
            raise RuntimeError('cleanup bundle has no bounded identifier')
        bundles.append({'path': str(path), 'id': identifier})
    return bundles


def process_vanished(code):
    return code == errno.ESRCH


def observe(bundles, known_pids):
    # SDK libproc.h / sys/proc_info.h: PROC_UID_ONLY=4; proc_listpids returns bytes.
    lib = ctypes.CDLL('/usr/lib/libproc.dylib', use_errno=True)
    lib.proc_listpids.argtypes = [ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_int]
    lib.proc_listpids.restype = ctypes.c_int
    lib.proc_pidpath.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
    lib.proc_pidpath.restype = ctypes.c_int
    pids = (ctypes.c_int * MAX_PIDS)()
    size = lib.proc_listpids(4, os.getuid(), pids, ctypes.sizeof(pids))
    if size <= 0 or size >= ctypes.sizeof(pids) or size % ctypes.sizeof(ctypes.c_int):
        raise RuntimeError('private-app process scan failed or exceeded its PID cap')
    prefixes = tuple(bundle['path'] + '/' for bundle in bundles)
    live, unknown = {}, []
    for pid in pids[:size // ctypes.sizeof(ctypes.c_int)]:
        if pid <= 1:
            continue
        buffer = ctypes.create_string_buffer(MAX_PATH)
        ctypes.set_errno(0)
        length = lib.proc_pidpath(pid, buffer, len(buffer))
        if length <= 0:
            if not process_vanished(ctypes.get_errno()):
                unknown.append(pid)
            continue
        if length >= len(buffer):
            unknown.append(pid)
            continue
        path = os.path.realpath(os.fsdecode(buffer.value))
        if path.startswith(prefixes):
            live[str(pid)] = path
        elif pid in known_pids:
            unknown.append(pid)  # A reused PID is not authority to touch its new process.
    return {'live': live, 'unknown': unknown}


def bounded_command(argv, timeout=5):
    """Cap both OS-query streams while running; timeout kills only our direct child."""
    child = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    selector = None
    output = {'stdout': bytearray(), 'stderr': bytearray()}
    reaped = False
    try:
        selector = selectors.DefaultSelector()
        for name, stream in [('stdout', child.stdout), ('stderr', child.stderr)]:
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ, name)
        deadline = time.monotonic() + timeout
        while selector.get_map():
            left = deadline - time.monotonic()
            if left <= 0:
                raise TimeoutError('OS-query command exceeded its time bound')
            for key, _ in selector.select(min(left, 0.05)):
                block = os.read(key.fileobj.fileno(), 4096)
                if not block:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    continue
                held = output[key.data]
                if len(held) + len(block) > MAX_OUTPUT:
                    raise RuntimeError('OS-query output exceeded its byte cap')
                held.extend(block)
        code = child.wait(timeout=max(0.001, deadline - time.monotonic()))
        reaped = True
        return code, bytes(output['stdout']), bytes(output['stderr'])
    finally:
        primary = sys.exc_info()[1]
        try:
            if not reaped:
                child.kill()  # Owned OS-query child, never a discovered application PID.
                child.wait(timeout=1)
        except Exception as error:
            if primary is None:
                raise
            raise primary from error
        finally:
            try:
                if selector is not None:
                    selector.close()
            finally:
                child.stdout.close()
                child.stderr.close()


def _osstatus_domain(cf, error):
    wanted = ctypes.c_void_p.in_dll(cf, 'kCFErrorDomainOSStatus').value
    domain = cf.CFErrorGetDomain(error)
    return bool(wanted and domain and cf.CFEqual(domain, wanted))


def registration_query(identifier, wanted):
    # This synchronous CF IPC runs only in the bounded child below, not the carry controller.
    cf = ctypes.CDLL('/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation')
    ls = ctypes.CDLL('/System/Library/Frameworks/CoreServices.framework/CoreServices')
    ptr = ctypes.c_void_p
    for name, arguments, result in [
        ('CFStringCreateWithCString', [ptr, ctypes.c_char_p, ctypes.c_uint32], ptr),
        ('CFArrayGetCount', [ptr], ctypes.c_long),
        ('CFArrayGetValueAtIndex', [ptr, ctypes.c_long], ptr),
        ('CFURLCopyFileSystemPath', [ptr, ctypes.c_int], ptr),
        ('CFStringGetCString', [ptr, ctypes.c_char_p, ctypes.c_long, ctypes.c_uint32], ctypes.c_bool),
        ('CFErrorGetCode', [ptr], ctypes.c_long), ('CFErrorGetDomain', [ptr], ptr),
        ('CFEqual', [ptr, ptr], ctypes.c_bool), ('CFRelease', [ptr], None),
    ]:
        function = getattr(cf, name)
        function.argtypes, function.restype = arguments, result
    ls.LSCopyApplicationURLsForBundleIdentifier.argtypes = [ptr, ctypes.POINTER(ptr)]
    ls.LSCopyApplicationURLsForBundleIdentifier.restype = ptr
    key = cf.CFStringCreateWithCString(None, identifier.encode(), 0x08000100)
    if not key:
        raise RuntimeError('bundle identifier could not be represented')
    error = ptr()
    array = ls.LSCopyApplicationURLsForBundleIdentifier(key, ctypes.byref(error))
    try:
        if error:
            if not array and _osstatus_domain(cf, error) and cf.CFErrorGetCode(error) == -10814:
                return False
            raise RuntimeError('registration lookup returned an error, so absence is unknown')
        if not array:
            raise RuntimeError('registration lookup returned no definite result')
        count = cf.CFArrayGetCount(array)
        if not 0 <= count <= 256:
            raise RuntimeError('registration lookup exceeded 256 URLs')
        found = False
        for index in range(count):
            text = cf.CFURLCopyFileSystemPath(cf.CFArrayGetValueAtIndex(array, index), 0)
            try:
                buffer = ctypes.create_string_buffer(MAX_PATH)
                if not text or not cf.CFStringGetCString(text, buffer, len(buffer), 0x08000100):
                    raise RuntimeError('registration path exceeded its bound')
                found |= os.path.realpath(os.fsdecode(buffer.value)) == wanted
            finally:
                if text:
                    cf.CFRelease(text)
        return found
    finally:
        if array:
            cf.CFRelease(array)
        if error:
            cf.CFRelease(error)
        cf.CFRelease(key)


def registered(bundle):
    code, raw, error = bounded_command([sys.executable, '-I', '-B', str(Path(__file__).resolve()),
                                       '--registration-query', bundle['id'], bundle['path']])
    if code != 0:
        raise RuntimeError(f'registration-query subprocess failed ({code}): {error.decode(errors="replace")[:1024]}')
    result = json.loads(raw)
    if not isinstance(result, bool):
        raise RuntimeError('registration-query result is not definite')
    return result


def finish(work, bundles, runs, probe, quit_run, forget, query, remove, now=time.monotonic, pause=time.sleep, limit=5):
    """Pure retirement policy: injected observations/waits make failure order testable."""
    report = {'complete': False, 'retained_work': work, 'quit': [], 'unregister': []}
    try:
        deadline = now() + limit
        seen = probe()
        report['processes'] = seen
        if seen['live'] or seen['unknown']:
            states = set()
            for run in reversed(runs):
                if run.state in states:
                    continue
                states.add(run.state)
                try:
                    reply = quit_run(run)
                    report['quit'].append({'state': run.state, 'exit_code': reply.returncode})
                except Exception as error:
                    report['quit'].append({'state': run.state, 'error': str(error)[:1024]})
            seen = probe()  # Quit may have ended the app even when its reply timed out.
            report['processes'] = seen
        while seen['live'] or seen['unknown']:
            left = deadline - now()
            if left <= 0:
                raise TimeoutError('app/helper exit was not established before cleanup deadline')
            pause(min(0.05, left))  # Bounded condition poll; unit tests inject a fake clock.
            seen = probe()
            report['processes'] = seen
        registration_deadline = now() + limit
        for bundle in bundles:
            attempt = {'path': bundle['path'], 'attempts': 0}
            report['unregister'].append(attempt)
            while True:
                if now() >= registration_deadline:
                    raise TimeoutError('exact bundle URL absence was not established')
                code = forget(bundle)
                attempt.update(exit_code=code, attempts=attempt['attempts'] + 1)
                present = query(bundle)
                attempt['present'] = present
                if present is False:
                    break  # Absence, not lsregister's return code, decides.
                if present is not True:
                    raise RuntimeError('exact bundle registration is unknown')
                left = registration_deadline - now()
                if left <= 0:
                    raise TimeoutError('exact bundle URL stayed registered')
                pause(min(0.05, left))
        remove()
        report['complete'] = True
        report['retained_work'] = None
    except Exception as error:
        report['error'] = type(error).__name__ + ': ' + str(error)[:1024]
    return report


def clean_up(work, bundles, runs):
    try:
        root = Path(work).resolve()
        paths = {bundle['path'] for bundle in bundles}
        if any(not Path(path).is_relative_to(root) or Path(path) == root for path in paths):
            raise RuntimeError('cleanup bundle escaped this private work folder')
        for run in runs:
            state = Path(run.state).resolve()
            if not state.is_relative_to(root) or state == root or os.path.realpath(run.app) not in paths:
                raise RuntimeError('cleanup run escaped this private work folder')
        known = {run.pid for run in runs if run.pid is not None}
        return finish(
            work, bundles, runs,
            probe=lambda: observe(bundles, known),
            quit_run=lambda run: run.raw('qa', 'quit', 'now', timeout=5),
            forget=lambda bundle: bounded_command([LSREGISTER, '-u', bundle['path']])[0],
            query=registered,
            remove=lambda: shutil.rmtree(work),
        )
    except Exception as error:
        return {'complete': False, 'retained_work': work, 'error': type(error).__name__ + ': ' + str(error)[:1024]}


if __name__ == '__main__':
    if len(sys.argv) != 4 or sys.argv[1] != '--registration-query':
        raise SystemExit('cleanup.py is an internal carry-check OS query')
    if not 0 < len(sys.argv[2].encode()) <= 1024 or len(os.fsencode(sys.argv[3])) >= MAX_PATH:
        raise SystemExit('registration query arguments exceed their bounds')
    print(json.dumps(registration_query(sys.argv[2], sys.argv[3])))
