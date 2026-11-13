"""The ratchet. See tools/ratchet.sh. Heuristic, not a Rust parser: comments, strings and chars are masked, then braces are matched."""
import bisect, os, re, subprocess, sys

def env_int(name, default):
    return int(os.environ.get(name, default))

BUDGET = {
    "file": env_int("RATCHET_FILE_LINES", 1500),
    "fn": env_int("RATCHET_FN_LINES", 150),
    "struct": env_int("RATCHET_STRUCT_FIELDS", 15),
    "enum": env_int("RATCHET_ENUM_VARIANTS", 40),
    "impl": env_int("RATCHET_IMPL_METHODS", 40),
    "type": env_int("RATCHET_TYPE_METHODS", 60),
}
WHAT = {
    "file": ("production lines", "Split the file by concern into a module of its own (guide 2.8)."),
    "fn": ("lines", "Split the function by step into helpers in the feature's own module (guide 2.8)."),
    "struct": ("fields", "Group related fields into a struct of their own, owned by the feature (guide 2.8)."),
    "enum": ("variants", "Split the enum by feature, each feature owning its variants (guide 2.8)."),
    "impl": ("methods", "Move the methods of one concern into a type or an impl in a module of its own (guide 2.8)."),
    "type": ("methods in all its inherent impl blocks", "Cut the type into components that own their fields and methods; an `impl` block in another file is the same type (guide 2.8)."),
}
# type key -> {file: methods}: filled by measure(), read by compare() to name the files of a type that is over
TYPE_FILES = {}
HUBS = [
    "crates/recto-session/src/lib.rs", "crates/recto-agent/src/lib.rs", "crates/recto-agent/src/table.rs",
    "crates/recto-agent/src/human.rs", "crates/recto-ffi/src/lib.rs", "crates/recto-ffi/src/qa.rs",
    "crates/recto-ffi/src/menus.rs", "app/Sources/RectoPDF/MainWindow.swift", "app/Sources/RectoPDF/QAChannel.swift",
    "crates/recto-cli/tests/app.rs", "crates/recto-cli/tests/restore.rs",
]
POLL = "ratchet: poll"
FFI_DIR = "crates/recto-ffi/src"
# The test binaries that launch the real app, the most expensive tests there are: 75% of all test time for 10% of the tests (2026-11-12). Their count
# only goes down: a rule Rust decides is tested in process, and what only the real app shows joins a launch that is already there.
APPTEST_DIR = "crates/recto-cli/tests"
APPTEST_BINARIES = ("app", "restore", "compare", "evidence", "long_session")
TEST_ATTR = re.compile(r'^\s*#\[\s*test\s*\]', re.M)
# The hub types whose size is more than their methods: every function in their crate's src/ that takes the whole thing is counted too, as a method moved
# out to a free function that still takes all of it is the same hub by another name (R3.7: `Inner`'s 113 methods and about 84 such functions). Only down.
WHOLE = {"crates/recto-app/src": "Inner"}
FN_HEAD = re.compile(r'\bfn\s+\w+')

# ---- Rust masking -------------------------------------------------------------------------------------------------------------------------
MASK = re.compile(
    r'(?<![\w])b?r(#*)"(?:.*?)"\1'                       # raw string
    r'|b?"(?:\\.|[^"\\])*"'                              # string
    r"|'(?:\\(?:u\{[^}]*\}|x[0-9a-fA-F]{2}|.)|[^\\'])'"  # char (a lifetime has no closing quote)
    r'|//[^\n]*'                                         # line comment
    r'|/\*.*?\*/',                                       # block comment (not nested: a blind spot)
    re.S)

def blank(s):
    return re.sub(r'[^\n]', ' ', s)

def mask(text):
    return MASK.sub(lambda m: blank(m.group(0)), text)

def match_brace(t, i):
    """Index of the } closing the { at i (len(t)-1 when unbalanced)."""
    d = 0
    for m in re.finditer(r'[{}]', t[i:]):
        d += 1 if m.group(0) == '{' else -1
        if d == 0:
            return i + m.start()
    return len(t) - 1

TESTATTR = re.compile(r'#\[cfg\(test\)\]')

def strip_tests(t):
    """(text with cfg(test) items blanked, lines removed, names of out-of-line test modules)."""
    out, removed, outline, pos = [], 0, [], 0
    while True:
        m = TESTATTR.search(t, pos)
        if not m:
            break
        j = m.end()
        while True:
            a = re.compile(r'\s*#\[[^\]]*\]').match(t, j)
            if not a:
                break
            j = a.end()
        k = j
        while k < len(t) and t[k] not in '{;':
            k += 1
        if k >= len(t):
            break
        if t[k] == '{':
            end = match_brace(t, k) + 1
        else:
            end = k + 1
            mm = re.match(r'\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*$', t[j:k])
            if mm:
                outline.append(mm.group(1))
        removed += t.count('\n', m.start(), end) + 1
        out.append(t[pos:m.start()])
        out.append(blank(t[m.start():end]))
        pos = end
    out.append(t[pos:])
    return ''.join(out), removed, outline

# ---- Rust item scan -----------------------------------------------------------------------------------------------------------------------
TOK = re.compile(
    r'(?P<fn>\bfn\s+(?P<fname>\w+))|(?P<struct>\bstruct\s+(?P<sname>\w+))|(?P<enum>\benum\s+(?P<ename>\w+))'
    r'|(?P<impl>(?m:^[ \t]*)(?:pub(?:\([^)]*\))?[ \t]+)?(?:unsafe[ \t]+)?impl\b)|(?P<trait>(?m:^[ \t]*)(?:pub(?:\([^)]*\))?[ \t]+)?trait[ \t]+(?P<tname>\w+))'
    r'|(?P<open>\{)|(?P<close>\})|(?P<po>[(\[])|(?P<pc>[)\]])|(?P<semi>;)')

def impl_type(header):
    """(is_inherent, type name) of the text between `impl` and `{`."""
    h = header.split(' where ')[0]
    h = re.sub(r'->', '  ', h)
    d, buf = 0, []
    for ch in h:                      # drop generic argument lists
        if ch == '<':
            d += 1
        elif ch == '>':
            d = max(0, d - 1)
        elif d == 0:
            buf.append(ch)
    h = ''.join(buf)
    parts = re.split(r'\bfor\b', h)
    name = re.findall(r'\w+', parts[-1])
    return len(parts) == 1, (name[-1] if name else '?')

def split_top(body):
    body = body.replace('->', '  ')
    parts, d, cur = [], 0, []
    for ch in body:
        if ch in '([{<':
            d += 1
        elif ch in ')]}>':
            d -= 1
        if ch == ',' and d == 0:
            parts.append(''.join(cur)); cur = []
        else:
            cur.append(ch)
    parts.append(''.join(cur))
    return parts

def strip_attrs(s):
    return re.sub(r'#\[[^\]]*\]', '', s)

REMOTE = re.compile(r'#\[\s*uniffi\s*::\s*remote\b')

def is_remote(t, at):
    """Whether the struct or enum whose keyword is at `at` has `#[uniffi::remote(..)]` among its own attributes: the text since the item before it ended."""
    before = max(t.rfind(c, 0, at) for c in ';{}')
    return bool(REMOTE.search(t, before + 1, at))

def scan_items(t, lineof):
    """Yield ('fn'|'struct'|'enum'|'impl', key, number) from masked, test-stripped text. A struct or enum declared under `#[uniffi::remote(..)]` restates a
    type that lives, and is counted, in another crate, so it's left out of the field and variant budgets; its lines still count toward its file's and the
    bridge crate's."""
    stack = []          # (kind, name, depth_of_open_brace, start_pos)
    pend = None         # (kind, name, start_pos, paren_depth)
    remote = set()      # start positions of remote declarations
    pd = 0
    bd = 0
    fns, impls = {}, {}
    res = []
    for m in TOK.finditer(t):
        g = m.lastgroup
        if m.group('fn'):
            pend = ('fn', m.group('fname'), m.start(), pd)
        elif m.group('struct'):
            pend = ('struct', m.group('sname'), m.end(), pd)
            if is_remote(t, m.start()):
                remote.add(m.end())
        elif m.group('enum'):
            pend = ('enum', m.group('ename'), m.end(), pd)
            if is_remote(t, m.start()):
                remote.add(m.end())
        elif m.group('impl'):
            pend = ('impl', None, m.end(), pd)
        elif m.group('trait'):
            pend = ('trait', m.group('tname'), m.end(), pd)
        elif m.group('po'):
            pd += 1
        elif m.group('pc'):
            pd -= 1
        elif m.group('semi'):
            if pend and pend[3] == pd:
                pend = None
        elif m.group('open'):
            bd += 1
            if pend and pend[3] == pd:
                kind, name, start, _ = pend
                if kind == 'impl':
                    inherent, name = impl_type(t[start:m.start()])
                    kind = 'impl' if inherent else 'traitimpl'
                stack.append((kind, name, bd, start, m.start()))
                pend = None
            else:
                stack.append(('blk', None, bd, m.start(), m.start()))
        elif m.group('close'):
            if stack and stack[-1][2] == bd:
                kind, name, _, start, op = stack.pop()
                if kind == 'fn':
                    owner = next((s for s in reversed(stack) if s[0] in ('fn', 'impl', 'traitimpl', 'trait')), None)
                    pre = ''
                    if owner:
                        pre = owner[1] + '::'
                        if owner[0] == 'fn':
                            pre = owner[1] + '/'
                    key = pre + name
                    n = lineof(m.start()) - lineof(start) + 1
                    fns[key] = max(fns.get(key, 0), n)
                    if owner and owner[0] == 'impl' and stack[-1] is owner:
                        impls[owner[1]] = impls.get(owner[1], 0) + 1
                elif kind in ('struct', 'enum') and start in remote:
                    pass
                elif kind == 'struct':
                    n = sum(1 for p in split_top(strip_attrs(t[op + 1:m.start()])) if ':' in p)
                    res.append(('struct', name, n))
                elif kind == 'enum':
                    n = sum(1 for p in split_top(strip_attrs(t[op + 1:m.start()])) if p.strip())
                    res.append(('enum', name, n))
            bd -= 1
    for k, v in fns.items():
        res.append(('fn', k, v))
    for k, v in impls.items():
        res.append(('impl', k, v))
    return res

def in_test_folder(rel, test_mods):
    """Whether a file is under the folder of a module declared `#[cfg(test)]`: `core/headless/x.rs` under `mod headless;`."""
    d = os.path.dirname(rel)
    while d and d != os.path.dirname(d):
        if os.path.join(d, 'mod.rs') in test_mods or d + '.rs' in test_mods:
            return True
        d = os.path.dirname(d)
    return False

def whole_fns(t, name):
    """How many functions in masked, test-stripped text take `name` in a parameter: `fn f(inner: &Inner)`, `fn g<'a>(_: &'a Arc<super::Inner>)`."""
    word = re.compile(r'\b' + re.escape(name) + r'\b')
    n = 0
    for m in FN_HEAD.finditer(t):
        i, depth = m.end(), 0
        while i < len(t) and (depth or t[i] != '('):      # past the generics, to the parameters
            if t[i] == '<':
                depth += 1
            elif t[i] == '>':
                depth -= 1
            elif t[i] in '{;':
                break
            i += 1
        if i >= len(t) or t[i] != '(':
            continue
        j, depth = i, 0
        while j < len(t):
            if t[j] == '(':
                depth += 1
            elif t[j] == ')':
                depth -= 1
                if not depth:
                    break
            j += 1
        if word.search(t[i:j]):
            n += 1
    return n

# ---- Measuring ----------------------------------------------------------------------------------------------------------------------------
def walk(root, sub, exts, skip=()):
    base = os.path.join(root, sub)
    for d, dirs, files in os.walk(base):
        dirs[:] = sorted(x for x in dirs if x not in ('target', '.build') and x not in skip)
        for f in sorted(files):
            if f.endswith(exts):
                yield os.path.relpath(os.path.join(d, f), root)

def rust_files(root):
    crates = os.path.join(root, 'crates')
    if not os.path.isdir(crates):
        return
    for c in sorted(os.listdir(crates)):
        for sub in ('src', 'tests', 'benches', 'examples'):
            yield from walk(root, os.path.join('crates', c, sub), ('.rs',))

def measure(root):
    """dict: (kind, key) -> number. Kinds file, fn, struct, enum, impl, type, sleep, ffi, apptest, whole."""
    m = {}
    TYPE_FILES.clear()
    apptests = 0
    srcs, test_mods = {}, set()
    for rel in rust_files(root):
        with open(os.path.join(root, rel), encoding='utf-8', errors='replace') as f:
            raw = f.read()
        # sleeps count in every Rust file, tests and sources alike; a comment line or the poll marker is not a fixed wait
        n = 0
        for line in raw.split('\n'):
            s = line.lstrip()
            if s.startswith('//') or POLL in line:
                continue
            if re.search(r'\bsleep\(', line) and not re.search(r'\bfn\s+sleep\b', line):
                n += 1
        if n:
            m[('sleep', rel)] = n
        parts = rel.split(os.sep)
        if rel.startswith(APPTEST_DIR + '/') and parts[3].split('.')[0] in APPTEST_BINARIES:
            apptests += len(TEST_ATTR.findall(mask(raw)))
        if len(parts) > 3 and parts[2] == 'src':
            srcs[rel] = raw
    for rel, raw in srcs.items():
        t, _, outline = strip_tests(mask(raw))
        d, base = os.path.dirname(rel), os.path.basename(rel)
        sub = d if base in ('mod.rs', 'lib.rs', 'main.rs') else os.path.join(d, base[:-3])
        for name in outline:
            test_mods.add(os.path.join(sub, name + '.rs'))
            test_mods.add(os.path.join(sub, name, 'mod.rs'))
    ffi_total = 0
    for rel, raw in srcs.items():
        if rel in test_mods or '#![cfg(test)]' in raw:
            continue
        t, removed, _ = strip_tests(mask(raw))
        nl = [i for i, ch in enumerate(t) if ch == '\n']
        lineof = lambda pos, nl=nl: bisect.bisect_left(nl, pos) + 1
        lines = raw.count('\n') + (0 if raw.endswith('\n') or not raw else 1) - removed
        m[('file', rel)] = lines
        if rel.startswith(FFI_DIR + '/'):
            ffi_total += lines
        for crate, name in WHOLE.items():
            # A test module's folder holds tests only, whatever its files say (`#[cfg(test)] mod headless;` and every file under headless/).
            if rel.startswith(crate + '/') and not in_test_folder(rel, test_mods):
                k = ('whole', crate + '::' + name)
                m[k] = m.get(k, 0) + whole_fns(t, name)
        for kind, key, n in scan_items(t, lineof):
            k = (kind, rel + '::' + key)
            m[k] = max(m.get(k, 0), n)
            if kind == 'impl':
                # the same type across files: every inherent impl block of a name in a crate's src/ is one type (a name defined twice in one crate is a blind spot)
                tk = '/'.join(rel.split('/')[:3]) + '::' + key
                TYPE_FILES.setdefault(tk, {})[rel] = n
    for tk, files in TYPE_FILES.items():
        m[('type', tk)] = sum(files.values())
    m[('ffi', FFI_DIR)] = ffi_total
    if apptests:
        m[('apptest', APPTEST_DIR)] = apptests
    for rel in walk(root, 'app/Sources', ('.swift',), skip=('Generated',)):
        with open(os.path.join(root, rel), encoding='utf-8', errors='replace') as f:
            raw = f.read()
        m[('file', rel)] = raw.count('\n') + (0 if raw.endswith('\n') or not raw else 1)
    for sub in ('app/qa',):
        for rel in walk(root, sub, ('.sh', '.qa')):
            n = 0
            with open(os.path.join(root, rel), encoding='utf-8', errors='replace') as f:
                for line in f:
                    s = line.strip()
                    if s.startswith('#') or POLL in line:
                        continue
                    if re.match(r'(?:.*[;&|]\s*|\s*)sleep\s', line):
                        n += 1
            if n:
                m[('sleep', rel)] = n
    return m

# ---- Baseline -----------------------------------------------------------------------------------------------------------------------------
def read_baseline(path):
    base, problems, prev = {}, [], None
    if not os.path.exists(path):
        return base, ['baseline file %s is missing' % path]
    with open(path) as f:
        for ln, line in enumerate(f.read().split('\n'), 1):
            if not line:
                continue
            parts = line.split(' ')
            if len(parts) != 3 or not parts[2].isdigit():
                problems.append('baseline line %d is not "kind key number": %r' % (ln, line))
                continue
            if prev is not None and line <= prev:
                problems.append('baseline line %d is %s: keep the file sorted, one entry per line (sort -u)'
                                % (ln, 'a duplicate' if line == prev else 'out of order'))
            if (parts[0], parts[1]) in base:
                problems.append('baseline line %d repeats %s %s: one entry per key' % (ln, parts[0], parts[1]))
            prev = line
            base[(parts[0], parts[1])] = int(parts[2])
    return base, problems

def limit(kind, key, base):
    if (kind, key) in base:
        return base[(kind, key)]
    return 0 if kind == 'sleep' else BUDGET.get(kind)

def compare(cur, base, bootstrap_ffi=False):
    fails, shrunk = [], []
    for (kind, key), n in sorted(cur.items()):
        if kind == 'ffi':
            if ('ffi', key) not in base and bootstrap_ffi:
                pass
            elif ('ffi', key) not in base:
                fails.append('ffi %s: no baseline entry (add "ffi %s %d" by hand)' % (key, key, n))
            elif n > base[('ffi', key)]:
                fails.append('the binding crate %s grew: %d production lines, baseline %d. Move the new code out of recto-ffi '
                             'into the feature crate; the bridge only forwards (guide 2.8).' % (key, n, base[('ffi', key)]))
            elif n < base[('ffi', key)]:
                shrunk.append(('ffi', key, base[('ffi', key)], n))
            continue
        if kind == 'whole':
            if (kind, key) not in base:
                if not bootstrap_ffi:
                    fails.append('whole %s: no baseline entry (add "whole %s %d" by hand)' % (key, key, n))
            elif n > base[(kind, key)]:
                fails.append('functions that take the whole %s grew: %d in %s, baseline %d. Pass a function the parts it reads, or make it the method of '
                             'the part whose state it is, not a free function over all of it (guide 2.8, R3.7).' % (key.split('::')[-1], n, key, base[(kind, key)]))
            elif n < base[(kind, key)]:
                shrunk.append((kind, key, base[(kind, key)], n))
            continue
        if kind == 'apptest':
            if (kind, key) not in base:
                if not bootstrap_ffi:
                    fails.append('apptest %s: no baseline entry (add "apptest %s %d" by hand)' % (key, key, n))
            elif n > base[(kind, key)]:
                fails.append('tests that launch the app grew: %d in %s (%s), baseline %d. Test what Rust decides in process, with the headless app '
                             '(the testkit in recto-app); what only the real app shows joins a launch that is already there, or replaces one (AGENTS.md, '
                             '"CI does not get slower").' % (n, key, ', '.join(APPTEST_BINARIES), base[(kind, key)]))
            elif n < base[(kind, key)]:
                shrunk.append((kind, key, base[(kind, key)], n))
            continue
        lim = limit(kind, key, base)
        inb = (kind, key) in base
        if kind == 'sleep':
            if n > lim:
                fails.append('%s: %d fixed sleeps, %s%d. Wait for the condition with a bounded poll (worker-testing.md rule 5); '
                             'a genuine poll interval inside a wait helper takes the marker "// %s".'
                             % (key, n, 'baseline ' if inb else 'was none, budget ', lim, POLL))
            elif inb and n < lim:
                shrunk.append((kind, key, lim, n))
            continue
        what, todo = WHAT[kind]
        if kind == 'type':
            todo = '%s Its impl blocks: %s.' % (todo, ', '.join('%s (%d)' % f for f in sorted(TYPE_FILES.get(key, {}).items())))
        if n > lim:
            if inb:
                fails.append('%s %s grew: %d %s, baseline %d, budget %d. %s' % (kind, key, n, what, lim, BUDGET[kind], todo))
            else:
                fails.append('%s %s is a new offender: %d %s, budget %d. %s' % (kind, key, n, what, lim, todo))
        elif inb and n < lim:
            shrunk.append((kind, key, lim, n))
    for (kind, key), v in sorted(base.items()):
        if (kind, key) not in cur and kind != 'ffi':
            shrunk.append((kind, key, v, 0))
    return fails, shrunk

def wanted(cur):
    """Entries the baseline should hold: everything over its budget, every fixed-sleep file, and the binding total."""
    out = {}
    for (kind, key), n in cur.items():
        if kind in ('ffi', 'sleep', 'apptest', 'whole') or n > BUDGET[kind]:
            out[(kind, key)] = n
    return out

def write_baseline(path, entries):
    lines = sorted('%s %s %d' % (k, key, n) for (k, key), n in entries.items())
    os.makedirs(os.path.dirname(path) or '.', exist_ok=True)
    with open(path, 'w') as f:
        f.write('\n'.join(lines) + '\n')

# ---- A move ------------------------------------------------------------------------------------------------------------------------------
def moved(root, bpath, old, new, kind=None):
    """Renames the baseline entries of code that moved unchanged, from `old` to `new`: a file's path (every entry under it: the file, its sleeps, its
    functions, structs, enums and impls) or one item's key (`path::Name`). Refused unless the old one is gone from the tree (for a path: or no longer
    measures over its budget there, for every entry's kind, as when a small new file takes its name), the new one is there, and no number measured at
    the new place is higher than its entry. It never raises a number; --update lowers it later as usual. A `type` entry is per type, not per file, so a
    move leaves it alone; a renamed type or crate is renamed in the baseline by hand (a lower or equal number; --update drops the stale entry otherwise)."""
    base, problems = read_baseline(bpath)
    if problems:
        print('ratchet --moved: the baseline has problems, fix them first:')
        for p in problems:
            print('  - ' + p)
        return 1
    cur = measure(root)
    item = '::' in old or '::' in new
    under = (lambda key: key == old) if item else (lambda key: key == old or key.startswith(old + '::'))
    picked = sorted((k, key) for (k, key) in base if k not in ('ffi', 'apptest', 'type', 'whole') and under(key) and kind in (None, k))
    if not picked:
        print('ratchet --moved: refused: the baseline has no %sentry for %s' % (kind + ' ' if kind else '', old))
        return 1
    if item:
        if any((k, old) in cur for k, _ in picked):
            print('ratchet --moved: refused: %s is still in the tree, so it was copied, not moved' % old)
            return 1
        if not all((k, new) in cur for k, _ in picked):
            print('ratchet --moved: refused: nothing at %s to take the entry' % new)
            return 1
    else:
        # A new file may take the old path (a bridge file keeping a module's name, say): that's still a move, as long as the old path no longer
        # holds the offender. One that still measures over its budget, for any entry's kind, is a copy.
        if os.path.exists(os.path.join(root, old)):
            still = ['%s %s measures %d' % (k, key, cur[(k, key)]) for k, key in picked if cur.get((k, key), 0) > limit(k, key, {})]
            if still:
                print('ratchet --moved: refused: %s still exists and still holds the offender (%s), so it was copied, not moved' % (old, '; '.join(still)))
                return 1
        if not os.path.exists(os.path.join(root, new)):
            print('ratchet --moved: refused: %s does not exist' % new)
            return 1
    renamed, refused = [], []
    for k, key in picked:
        nkey = new + key[len(old):]
        n = cur.get((k, nkey), 0)
        if (k, nkey) in base:
            refused.append('%s %s already has an entry' % (k, nkey))
        elif n > base[(k, key)]:
            refused.append('%s %s measures %d, more than the entry %d it would take: a move is not a way to grow' % (k, nkey, n, base[(k, key)]))
        else:
            renamed.append((k, key, nkey))
    if refused:
        print('ratchet --moved: refused, nothing renamed:')
        for r in refused:
            print('  - ' + r)
        return 1
    for k, key, nkey in renamed:
        base[(k, nkey)] = base.pop((k, key))
        print('ratchet --moved: %s %s -> %s (%d)' % (k, key, nkey, base[(k, nkey)]))
    write_baseline(bpath, base)
    return 0

# ---- Report -------------------------------------------------------------------------------------------------------------------------------
def report(root, ref):
    r = subprocess.run(['git', '-C', root, 'diff', '--numstat', ref + '...HEAD'], capture_output=True, text=True)
    if r.returncode != 0:
        r = subprocess.run(['git', '-C', root, 'diff', '--numstat', ref], capture_output=True, text=True)
    if r.returncode != 0:
        print('ratchet --report: git could not diff against %s: %s' % (ref, r.stderr.strip()))
        return 0
    added = {}
    for line in r.stdout.splitlines():
        a, d, p = line.split('\t', 2)
        if p in HUBS:
            added[p] = (int(a) if a.isdigit() else 0, int(d) if d.isdigit() else 0)
    print('Hub files touched since %s (shared files take a one-line registration, guide 2.8):' % ref)
    if not added:
        print('  none')
    for p in HUBS:
        if p in added:
            print('  %-48s +%d -%d' % (p, added[p][0], added[p][1]))
    if added:
        print('  total added to hubs: %d' % sum(a for a, _ in added.values()))
    return 0

# ---- A merge of two baselines ---------------------------------------------------------------------------------------------------------------
def parse_side(text, stage):
    """{(kind, key): n} of one version of the baseline, or a message naming the first line that is not "kind key number"."""
    d = {}
    for ln, line in enumerate(text.split('\n'), 1):
        if not line:
            continue
        parts = line.split(' ')
        if len(parts) != 3 or not parts[2].isdigit():
            return None, 'stage %d (%s) line %d is not "kind key number": %r' % (stage, STAGES[stage], ln, line)
        d[(parts[0], parts[1])] = int(parts[2])
    return d, None

STAGES = {1: 'base', 2: 'ours', 3: 'theirs'}

def merge_sides(base, ours, theirs):
    """The merged baseline and the lines saying where the two sides differed. A key both have: the lower number. A key one side added: kept.
    A key either side dropped: gone. A key both added with two numbers: the lower, and said to need the check."""
    merged, said = {}, []
    for k in sorted(set(ours) | set(theirs) | set(base)):
        name = '%s %s' % k
        o, t, b = ours.get(k), theirs.get(k), base.get(k)
        if o is not None and t is not None:
            merged[k] = min(o, t)
            if o != t:
                said.append('%s: ours %d, theirs %d -> %d%s' % (name, o, t, min(o, t),
                            '  (both sides added it: needs the check)' if b is None else ''))
        elif o is not None or t is not None:
            who, n = ('ours', o) if o is not None else ('theirs', t)
            if b is None:
                merged[k] = n
                said.append('%s: added by %s at %d -> kept' % (name, who, n))
            else:
                said.append('%s: dropped by %s -> gone' % (name, 'theirs' if who == 'ours' else 'ours'))
    return merged, said

def git_out(root, *args):
    r = subprocess.run(['git', '-C', root] + list(args), capture_output=True, text=True)
    return r.returncode, r.stdout, r.stderr

def resolve(root, bpath):
    rel = os.path.relpath(os.path.abspath(bpath), root)
    rc, out, err = git_out(root, 'ls-files', '-u', '--', rel)
    if rc != 0:
        print('ratchet --resolve: git failed: %s' % err.strip()); return 1
    stages = {int(l.split()[2]) for l in out.splitlines() if len(l.split()) >= 3}
    if not stages:
        print('ratchet --resolve: %s is not in conflict (no merge under way, or it merged cleanly). Nothing changed.' % rel); return 1
    if 2 not in stages or 3 not in stages:
        print('ratchet --resolve: %s was deleted on one side; decide that by hand. Nothing changed.' % rel); return 1
    sides = {}
    for n in (1, 2, 3):
        if n in stages:
            rc, text, err = git_out(root, 'show', ':%d:%s' % (n, rel))
            if rc != 0:
                print('ratchet --resolve: cannot read stage %d: %s' % (n, err.strip())); return 1
        else:
            text = ''  # added on both sides: no common ancestor version
        sides[n], bad = parse_side(text, n)
        if bad:
            print('ratchet --resolve: %s. Nothing changed; fix the line by hand.' % bad); return 1
    merged, said = merge_sides(sides[1], sides[2], sides[3])
    write_baseline(bpath, merged)
    print('ratchet --resolve: wrote %s with %d entries (not added to git).' % (rel, len(merged)))
    for s in said:
        print('  ' + s)
    if not said:
        print('  the two sides agree on every key.')
    print('Now run tools/ratchet.sh on the merged tree: the true number can be higher than either side\'s when both branches added to the same thing '
          '(raise it only by a hand edit a reviewer sees). When it passes: git add %s' % rel)
    return 0

# ---- Main ---------------------------------------------------------------------------------------------------------------------------------
def main(argv):
    root = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
    bpath, mode, ref, kind = None, 'check', None, None
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == '--root':
            i += 1; root = os.path.abspath(argv[i])
        elif a == '--baseline':
            i += 1; bpath = argv[i]
        elif a == '--update':
            mode = 'update'
        elif a == '--resolve':
            mode = 'resolve'
        elif a == '--moved':
            mode = 'moved'
            if i + 2 >= len(argv):
                print('usage: tools/ratchet.sh --moved <old path or path::Item> <new path or path::Item>'); return 2
            ref = (argv[i + 1], argv[i + 2]); i += 2
        elif a == '--kind':
            i += 1; kind = argv[i] if i < len(argv) else None
            if kind not in BUDGET and kind != 'sleep':
                print('usage: --kind is one of %s' % ', '.join(sorted(list(BUDGET) + ['sleep']))); return 2
        elif a == '--report':
            mode = 'report'; i += 1; ref = argv[i] if i < len(argv) else None
            if ref is None:
                print('usage: tools/ratchet.sh --report <base-ref>'); return 2
        else:
            print(__doc__); print('usage: tools/ratchet.sh [--update | --resolve | --report <base-ref> | --moved <old> <new> [--kind K]] [--root DIR] [--baseline FILE]'); return 2
        i += 1
    bpath = bpath or os.path.join(root, 'tools', 'ratchet', 'baseline.txt')
    if mode == 'resolve':
        return resolve(root, bpath)
    if mode == 'report':
        return report(root, ref)
    if mode == 'moved':
        return moved(root, bpath, ref[0], ref[1], kind)
    cur = measure(root)
    base, problems = read_baseline(bpath)
    if os.environ.get('RATCHET_BOOTSTRAP') and not os.path.exists(bpath):
        write_baseline(bpath, wanted(cur)); print('ratchet: wrote a first baseline with %d entries' % len(wanted(cur))); return 0
    fails, shrunk = compare(cur, base, mode == 'update')
    for p in problems:
        fails.append(p)
    if fails:
        print('ratchet: FAILED (%d)' % len(fails))
        for f in fails:
            print('  - ' + f)
        print('A baseline number is raised only by a hand edit of %s, which a reviewer sees in the diff.' % os.path.relpath(bpath, root))
        return 1
    if mode == 'update':
        write_baseline(bpath, wanted(cur))
        print('ratchet: baseline rewritten, %d entries, %d lowered or dropped' % (len(wanted(cur)), len(shrunk)))
        return 0
    if shrunk:
        print('ratchet: ok, and %d entries are now smaller than the baseline. Lock it in: tools/ratchet.sh --update' % len(shrunk))
        for kind, key, old, new in shrunk[:8]:
            print('  %s %s: %d -> %d' % (kind, key, old, new))
        if len(shrunk) > 8:
            print('  ... and %d more' % (len(shrunk) - 8))
    else:
        print('ratchet: ok')
    return 0

if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
