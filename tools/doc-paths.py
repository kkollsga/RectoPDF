"""Every path and name a how-to page names in backticks exists, so the page can't rot without a check failing (R4.2).

    tools/doc-paths.sh [FILE.md ...]      (default: the pages in DOC_PATH_FILES, tools/check-scope.sh)

A backticked token is checked as:
- a path, when it has a `/` or ends in a known extension: the file or folder must exist from the repository's top. `<name>` in it is a placeholder,
  so only the part before it is checked. In a token with spaces (a command line), only its first word is checked, if that is a path.
- a name, when it's an identifier, `a::b` or `NAME=value`: its last part must be a word in the sources (crates/, tools/, app/).
Anything else (flags, code with punctuation, prose in backticks) is left alone. Pure python, reads the tree, builds nothing, well under a second.
"""

import os
import re
import sys

EXTENSIONS = (".md", ".rs", ".sh", ".py", ".txt", ".swift", ".toml", ".json", ".yml")
SOURCE_DIRS = ("crates", "tools", "app")
SOURCE_EXTENSIONS = (".rs", ".sh", ".py", ".swift", ".txt", ".toml")
IDENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*$")


def words_in_sources(top):
    words = set()
    for folder in SOURCE_DIRS:
        for root, dirs, files in os.walk(os.path.join(top, folder)):
            dirs[:] = [d for d in dirs if d not in ("target", "build.noindex", ".build", "dev")]
            for name in files:
                if name.endswith(SOURCE_EXTENSIONS):
                    try:
                        with open(os.path.join(root, name), encoding="utf-8", errors="replace") as f:
                            words.update(re.findall(r"[A-Za-z_][A-Za-z0-9_]*", f.read()))
                    except OSError:
                        pass
    return words


def is_path(token):
    return "/" in token or token.endswith(EXTENSIONS)


def check_path(top, token):
    if "<" in token:
        # `crates/recto-agent/src/commands/<feature>.rs`: the folder it would be in.
        before = token.split("<", 1)[0]
        token = before if before.endswith("/") else os.path.dirname(before)
    return not token or os.path.exists(os.path.join(top, token))


def problems(top, page, words):
    out = []
    with open(page, encoding="utf-8") as f:
        text = f.read()
    for number, line in enumerate(text.splitlines(), 1):
        for token in re.findall(r"`([^`]+)`", line):
            token = token.strip()
            first = token.split()[0] if token else ""
            if " " in token:
                if is_path(first) and not first.startswith("-") and not check_path(top, first):
                    out.append(f"{page}:{number}: no such file: {first}")
                continue
            if is_path(token):
                if not token.startswith(("http", "~", "$")) and not check_path(top, token):
                    out.append(f"{page}:{number}: no such file: {token}")
                continue
            name = token.split("=", 1)[0] if "=" in token else token
            if IDENT.match(name):
                last = name.split("::")[-1]
                if last not in words:
                    out.append(f"{page}:{number}: no such name in the sources: {token}")
    return out


def main(argv):
    top = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
    pages = argv or []
    if not pages:
        print("doc-paths: name the pages to check", file=sys.stderr)
        return 2
    words = words_in_sources(top)
    found = []
    for page in pages:
        found.extend(problems(top, os.path.join(top, page) if not os.path.isabs(page) else page, words))
    if found:
        print("doc-paths: these pages name what isn't there (renamed or moved?). Fix the page:", file=sys.stderr)
        for problem in found:
            print("  " + os.path.relpath(problem.split(":", 1)[0], top) + ":" + problem.split(":", 1)[1], file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
