#!/usr/bin/env bash
# move_check.sh <base> <head> <old-file> [<old-file>...] [--into <path>]...
# Proves a split commit only moved code: the lines of the old files at
# <base> are the lines of the files that received them at <head>.
#
# Receiving files, read at both revisions where they exist: each old file,
# every .rs file under the module directory it becomes (`nav.rs` and
# `nav/**/*.rs`, `foo/mod.rs` and `foo/**/*.rs`), and each `--into` path
# (a file or a directory of .rs files). All old files and receivers form
# one set, so code moved between two old files passes when both are named.
#
# Both sides are rustfmt'd (edition 2024, max_width 1000, so a signature
# that a longer visibility or a deeper indent would rewrap stays on one
# line), then blank lines, comment-only
# lines and leading and trailing whitespace are dropped, and the multisets
# of lines are compared. Allowed differences:
#   - `use` and `mod` items (multi-line `use` trees included)
#   - visibility (`pub`, `pub(crate)`, `pub(super)`, `pub(in ..)`) at the
#     start of a line
#   - `impl ... {` and `mod ... {` header lines, with one closing `}` for
#     each such header added or removed
#   - `#[cfg(test)]`, which each test module that moves into its own file
#     carries
# Every other added (+) or removed (-) line is printed, with its count, and
# the exit status is 1. Exit 2: bad usage, or a file rustfmt cannot parse.
# A path that moved one directory deeper (`include_bytes!("../x")`) shows up
# as a difference; read it and judge.
set -euo pipefail
command -v rustfmt > /dev/null || { echo "move_check: rustfmt not found" >&2; exit 2; }
cd "$(git rev-parse --show-toplevel)"
exec python3 -I - "$@" << 'EOF'
import re
import subprocess
import sys
from collections import Counter

USAGE = "usage: move_check.sh <base> <head> <old-file> [<old-file>...] [--into <path>]..."


def die(msg):
    print(f"move_check: {msg}", file=sys.stderr)
    sys.exit(2)


def git(*args):
    r = subprocess.run(["git", *args], capture_output=True, text=True)
    if r.returncode != 0:
        die(f"git {' '.join(args)}: {r.stderr.strip()}")
    return r.stdout


def parse_args(argv):
    pos, into = [], []
    i = 0
    while i < len(argv):
        if argv[i] == "--into":
            if i + 1 >= len(argv):
                die(USAGE)
            into.append(argv[i + 1].rstrip("/"))
            i += 2
        else:
            pos.append(argv[i])
            i += 1
    if len(pos) < 3:
        die(USAGE)
    return pos[0], pos[1], pos[2:], into


def tree_files(rev, prefix):
    """The .rs files under directory `prefix` at `rev`."""
    out = git("ls-tree", "-r", "--name-only", rev, "--", prefix + "/")
    return {p for p in out.splitlines() if p.endswith(".rs")}


def exists(rev, path):
    r = subprocess.run(["git", "cat-file", "-e", f"{rev}:{path}"], capture_output=True)
    return r.returncode == 0


def receivers(rev, olds, into):
    paths = set()
    for old in olds:
        paths.add(old)
        stem = old[: -len(".rs")]
        moddir = stem[: -len("/mod")] if stem.endswith("/mod") else stem
        paths |= tree_files(rev, moddir)
    for p in into:
        if p.endswith(".rs"):
            paths.add(p)
        else:
            paths |= tree_files(rev, p)
    return {p for p in paths if exists(rev, p)}


def fmt(path, rev, text):
    r = subprocess.run(
        ["rustfmt", "--edition", "2024", "--config", "max_width=1000"], input=text, capture_output=True, text=True
    )
    if r.returncode != 0:
        die(f"rustfmt failed on {rev}:{path}:\n{r.stderr.strip()}")
    return r.stdout


VIS = re.compile(r"^pub(\((crate|super|self|in [^)]*)\))?\s+")
USE_OR_MOD = re.compile(r"^(use|mod)\b")
BLOCK_HEADER = re.compile(r"^((unsafe|default)\s+)*(impl\b.*|mod\s+\w+\s*)\{$")


def norm_lines(text):
    """Code lines with comments, blanks, edge whitespace and visibility gone;
    `use`/`mod` items are returned apart."""
    code, skipped = [], []
    in_block = False
    in_use = False
    for raw in text.splitlines():
        line = raw.strip()
        if in_block:
            if "*/" in line:
                in_block = False
            continue
        if not line or line.startswith("//"):
            continue
        if line.startswith("/*"):
            if "*/" not in line:
                in_block = True
            continue
        line = VIS.sub("", line)
        if in_use:
            skipped.append(line)
            if line.endswith(";"):
                in_use = False
            continue
        if line == "#[cfg(test)]":
            skipped.append(line)
            continue
        if USE_OR_MOD.match(line) and not BLOCK_HEADER.match(line):
            skipped.append(line)
            if not line.endswith(";"):
                in_use = True
            continue
        code.append(line)
    return code, skipped


def side(rev, paths):
    lines = Counter()
    for p in sorted(paths):
        code, _ = norm_lines(fmt(p, rev, git("show", f"{rev}:{p}")))
        lines.update(code)
    return lines


def main():
    base, head, olds, into = parse_args(sys.argv[1:])
    for old in olds:
        if not old.endswith(".rs"):
            die(f"{old}: not a .rs file")
        if not exists(base, old):
            die(f"{old} does not exist at {base}")
    base_paths = receivers(base, olds, into)
    head_paths = receivers(head, olds, into)
    print(f"base {base}: {' '.join(sorted(base_paths))}")
    print(f"head {head}: {' '.join(sorted(head_paths))}")
    a = side(base, base_paths)
    b = side(head, head_paths)
    added = b - a
    removed = a - b

    headers = 0
    bad = []
    for sign, diff in (("+", added), ("-", removed)):
        for line, n in sorted(diff.items()):
            if BLOCK_HEADER.match(line):
                headers += n if sign == "+" else -n
            elif line != "}":
                bad.append((sign, line, n))
    braces = added["}"] - removed["}"]
    if braces != headers:
        bad.append(("+" if braces > headers else "-", "}", abs(braces - headers)))

    total = sum(a.values())
    if bad:
        for sign, line, n in bad:
            print(f"{sign} {line}" + (f"  (x{n})" if n > 1 else ""))
        print(f"move_check: {len(bad)} line(s) differ beyond a move ({total} lines at base)")
        sys.exit(1)
    print(f"move_check: ok, {total} lines at base, a move only")


main()
EOF
