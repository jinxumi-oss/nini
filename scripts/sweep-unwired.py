#!/usr/bin/env python3
"""Triage scan for the R2c/R2d/R2e pattern.

A field that exists, is assigned, and has no production reader - so its
value can never be anything real. Rust's dead_code lint cannot see these:
everything here is `pub` in a library crate, and clippy is clean.

This produces a *candidate list*, not findings. Every entry has to be
confirmed by reading the code. The grep counts are a way of deciding what
to read, not what is true.
"""

import os
import re
import subprocess
from collections import defaultdict

# Relative to this script, not to whoever happens to run it. The old
# value pointed at one developer's checkout, so the tool could only be
# run from that machine - the same machine-specific coupling R3
# removed from the Rust sources.
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATES = ["nini-core", "nini-tui", "nini-ai", "nini-session",
          "nini-tools", "nini-cli", "nini-ext", "nini-conformance"]

def rust_files():
    for base, dirs, files in os.walk(os.path.join(ROOT, "crates")):
        dirs[:] = [d for d in dirs if d not in ("target", ".git")]
        for f in files:
            if f.endswith(".rs"):
                yield os.path.join(base, f)

def is_test(path):
    return "/tests/" in path or path.endswith("/tests.rs")


def blank_test_modules(src):
    """Blank out `#[cfg(test)]` blocks, keeping the line count.

    The sweep reports things unwired *in production*. A `#[cfg(test)] mod
    tests` inside `src/` is not production. Reporting its contents buries
    the real findings under noise: one run listed six `_name` parameters
    from agent.rs that were all test helpers, and three of the `let _x =`
    sites were guard bindings whose entire purpose is to be named.

    Lines are replaced rather than deleted so reported line numbers still
    point at the right source.
    """
    lines = src.split("\n")
    out = list(lines)
    i = 0
    while i < len(lines):
        if lines[i].strip().startswith("#[cfg(test)]"):
            depth = 0
            started = False
            j = i
            while j < len(lines):
                depth += lines[j].count("{") - lines[j].count("}")
                if "{" in lines[j]:
                    started = True
                out[j] = ""
                if started and depth <= 0:
                    break
                j += 1
            i = j + 1
            continue
        i += 1
    return "\n".join(out)


def load_prod(path):
    return blank_test_modules(open(path, errors="ignore").read())


def is_guard_binding(line):
    """`let _guard = MUTEX.lock()...` holds its value alive on purpose; it
    is not a discarded result."""
    return re.search(r"\blet\s+_\w*\b.*\.(lock|read|write)\(", line) is not None

files = list(rust_files())
prod = [f for f in files if not is_test(f)]
blob = "\n".join(open(f, errors="ignore").read() for f in prod)

# ---------- 1. pub struct fields with no production reader ----------
FIELD = re.compile(r"^\s*pub\s+(\w+)\s*:\s*[^,{(]+,?\s*(?://.*)?$")
struct_re = re.compile(r"(?:pub\s+)?struct\s+(\w+)\s*\{")

candidates = []
for path in prod:
    src = load_prod(path)
    cur_struct = None
    depth = 0
    for i, line in enumerate(src.split("\n"), 1):
        m = struct_re.search(line)
        if m:
            cur_struct = m.group(1)
            depth = line.count("{") - line.count("}")
            continue
        if cur_struct:
            depth += line.count("{") - line.count("}")
            f = FIELD.match(line)
            if f and depth >= 1:
                candidates.append((cur_struct, f.group(1), path, i))
            if depth <= 0:
                cur_struct = None

def prod_refs(name):
    """How many production lines mention `.name` or `name:`."""
    pat = re.compile(r"\.\s*" + re.escape(name) + r"\b")
    n = 0
    for src in blob.split("\n"):
        if pat.search(src):
            n += 1
    return n

print("=" * 78)
print("1. pub struct fields with ZERO production `.field` reads")
print("=" * 78)
rows = []
for struct, field, path, line in candidates:
    n = prod_refs(field)
    if n == 0:
        rel = os.path.relpath(path, ROOT)
        rows.append((rel, line, struct, field))
for rel, line, struct, field in sorted(rows):
    print(f"  {rel}:{line:<5} {struct}.{field}")
print(f"\n  ({len(rows)} candidates - confirm by reading before believing)")

# ---------- 2. explicitly discarded results: `let _x = ...` ----------
print()
print("=" * 78)
print("2. results explicitly discarded (`let _x = ...`) - a producer that")
print("   computes something and throws it away")
print("=" * 78)
discard = re.compile(r"\blet\s+_(?:mut\s+)?\w+\s*=")
hits = defaultdict(list)
for path in prod:
    for i, line in enumerate(load_prod(path).split("\n"), 1):
        if discard.search(line) and not is_guard_binding(line):
            hits[os.path.relpath(path, ROOT)].append((i, line.strip()))
n = 0
for f, items in sorted(hits.items()):
    for i, line in items:
        print(f"  {f}:{i:<5} {line[:96]}")
        n += 1
print(f"\n  ({n} sites)")

# ---------- 3. functions with no production caller ----------
print()
print("=" * 78)
print("3. pub fns defined but never called in production")
print("=" * 78)
FN = re.compile(r"^\s*pub(?:\([a-z]+\)\s+)?fn\s+(\w+)\s*[(<]")
defined = []
for path in prod:
    for i, line in enumerate(load_prod(path).split("\n"), 1):
        m = FN.match(line)
        if m:
            defined.append((m.group(1), os.path.relpath(path, ROOT), i))
unused = []
for name, f, i in defined:
    pat = re.compile(r"\b" + re.escape(name) + r"\s*\(")
    uses = 0
    for src in blob.split("\n"):
        if pat.search(src):
            uses += 1
    # a definition plus at most one trait-impl mention
    if uses <= 2:
        unused.append((f, i, name, uses))
for f, i, name, uses in sorted(unused):
    print(f"  {f}:{i:<5} fn {name}()   (refs in production: {uses})")
print(f"\n  ({len(unused)} candidates)")

# ---------- 4. underscore-prefixed parameters: accepted and ignored ----------
print()
print("=" * 78)
print("4. parameters accepted and discarded (`_name`) in pub fns")
print("=" * 78)
UND = re.compile(r"\b_[a-z]\w*\s*:\s*")
hits2 = defaultdict(list)
for path in prod:
    for i, line in enumerate(load_prod(path).split("\n"), 1):
        if UND.search(line) and "fn " in line:
            hits2[os.path.relpath(path, ROOT)].append((i, line.strip()))
n = 0
for f, items in sorted(hits2.items()):
    for i, line in items:
        print(f"  {f}:{i:<5} {line[:96]}")
        n += 1
print(f"\n  ({n} sites - these are signatures that promise something and")
print( "   take nothing; R2d found two that lied outright)")