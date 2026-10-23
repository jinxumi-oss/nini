#!/usr/bin/env python3
"""Behaviour-layer version lock for the Pi oracle.

The *format* layer is frozen in ``references/spec-v0.85.1/`` (sha256 per file).
The *behaviour* layer is frozen in ``references/pi-0.85.1/`` (411 files, sha256).
Both are the **same version** — 0.85.1 — because a split target is not a
target: nini's comments cite ``agent-loop.js#shouldStopAfterTurn`` and
friends, and nothing re-resolved those citations, so a Pi upgrade silently
moved every one of them.

This script closes that hole against the frozen snapshot in
``references/pi-0.85.1/`` (the single target version):

  --check         recompute the sha256 of every snapshotted file and compare
                  with ``SNAPSHOT.lock``.  0 = untouched, 1 = something moved.

  --verify-refs   re-resolve every line-number citation found by
                  ``scripts/pi-line-refs.py`` against the snapshot and print
                  the referenced line.  Citations whose line number is beyond
                  the end of the file are *drift* and fail the run; citations
                  whose document is not in the snapshot are reported as
                  ``missing-in-snapshot`` and also fail.

Usage
-----
    python3 scripts/pi-oracle-lock.py --check
    python3 scripts/pi-oracle-lock.py --verify-refs
    python3 scripts/pi-oracle-lock.py --check --verify-refs

Exit codes
----------
    0  everything holds
    1  at least one check failed (drift, missing file, or missing snapshot)
    2  the snapshot itself is unusable (no lock file / broken layout)
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
# **唯一目标版本**。
#
# v0.10.8 之前，nini 的格式层锁 0.85.1、行为层只能对 0.84.2（本机装着的
# 那个）——那不是"分层"，是**目标不唯一**：同一段代码两个版本可能给出两种
# 答案。实测：90 处行号引用里只有 3 处在这两个版本之间还指向同一行。
# 现在两个版本都进了仓库（references/pi-0.84.2 与 references/pi-0.85.1），
# 目标统一为 0.85.1，0.84.2 保留为**历史对照**。
DEFAULT_SNAPSHOT = "pi-0.85.1"
SNAPSHOT_DIR = REPO_ROOT / "references" / DEFAULT_SNAPSHOT
LOCK_FILE = SNAPSHOT_DIR / "SNAPSHOT.lock"
REF_SCRIPT = Path(__file__).resolve().parent / "pi-line-refs.py"

# 别名表只有一份（scripts/pi_refs.py）——三个脚本各抄一份就会重演
# 「同一份失真有三份副本」。
sys.path.insert(0, str(Path(__file__).resolve().parent))
from pi_refs import DOC_ALIASES  # noqa: E402

# Files that live in the snapshot directory but are not part of the lock.
LOCK_EXCLUDE = {"SNAPSHOT.lock", "README.md"}

RED = "\033[31m"
GREEN = "\033[32m"
YELLOW = "\033[33m"
RESET = "\033[0m"


def _color(text: str, code: str, stream) -> str:
    if getattr(stream, "isatty", lambda: False)():
        return f"{code}{text}{RESET}"
    return text


def load_lock() -> dict:
    if not LOCK_FILE.is_file():
        raise FileNotFoundError(f"snapshot lock not found: {LOCK_FILE}")
    return json.loads(LOCK_FILE.read_text(encoding="utf-8"))


def sha256_of(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check_snapshot(lock: dict) -> int:
    """Compare every locked file against the bytes on disk."""
    failures = 0
    files = lock.get("files", {})
    if not files:
        print("error: SNAPSHOT.lock lists no files", file=sys.stderr)
        return 2

    for rel in sorted(files):
        path = SNAPSHOT_DIR / rel
        if not path.is_file():
            print(f"MISSING  {rel} — listed in SNAPSHOT.lock but absent from the snapshot")
            failures += 1
            continue
        want = files[rel].get("sha256")
        got = sha256_of(path)
        size = path.stat().st_size
        want_bytes = files[rel].get("bytes")
        if got != want or (want_bytes is not None and size != want_bytes):
            print(f"CHANGED  {rel}")
            print(f"         lock sha256 {want} bytes {want_bytes}")
            print(f"         now  sha256 {got} bytes {size}")
            failures += 1
        else:
            print(f"ok       {rel}  sha256={got[:12]}…  bytes={size}")

    # Files that appeared in the snapshot without being locked are tampering.
    for path in sorted(SNAPSHOT_DIR.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(SNAPSHOT_DIR).as_posix()
        if rel in files or rel in LOCK_EXCLUDE:
            continue
        print(f"EXTRA    {rel} — present in the snapshot but not in SNAPSHOT.lock")
        failures += 1

    if failures:
        print(f"\nsnapshot check FAILED: {failures} problem(s)")
    else:
        print(f"\nsnapshot check OK: {len(files)} file(s) match SNAPSHOT.lock")
    return 1 if failures else 0


def _load_refs_module():
    spec = importlib.util.spec_from_file_location("pi_line_refs", REF_SCRIPT)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {REF_SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _snapshot_path(doc: str) -> Path | None:  # noqa: D401
    """Map a cited document name onto the snapshot file that backs it.

    ``*.js`` citations refer to the compiled bundle shipped in Pi's ``dist/``;
    ``*.ts`` citations refer to the TypeScript sources, which the npm package
    does not ship directly but which are recoverable verbatim from the
    ``sourcesContent`` of the matching ``.js.map``.
    """
    from pi_refs import resolve as _resolve
    return _resolve(SNAPSHOT_DIR, doc)


def verify_refs(lock: dict) -> int:
    """Re-resolve every Pi line-number citation against the snapshot."""
    try:
        module = _load_refs_module()
    except Exception as exc:  # pragma: no cover - defensive
        print(f"error: cannot load {REF_SCRIPT}: {exc}", file=sys.stderr)
        return 2

    records = module.scan(module.CRATES_DIR)
    if not records:
        print("error: no Pi line-number references found — did the scan scope change?", file=sys.stderr)
        return 2

    cache: dict[str, list[str] | None] = {}
    drift = 0
    missing = 0
    resolved = 0

    for rec in records:
        doc = rec["doc"]
        if doc not in cache:
            path = _snapshot_path(doc)
            if path is None:
                cache[doc] = None
            else:
                cache[doc] = path.read_text(encoding="utf-8", errors="replace").splitlines()
        lines = cache[doc]
        where = f"{rec['file']}:{rec['line']} -> {doc}:{rec['doc_line']}"

        # 符号引用：把含义钉住，行号由快照解析。
        #
        # 规则是"这个 token 在目标版本里还在不在、第一次出现在哪一行"。
        # 它不判断那是定义还是调用点——引用者写下的**含义**由代码评审负责，
        # 脚本负责的是「它没有消失」。token 消失 = 漂移 = 红灯。
        if rec.get("kind") == "symbol":
            sym = rec.get("symbol") or ""
            if lines is None:
                print(_color(f"MISSING-IN-SNAPSHOT  {where} (#{sym})", RED, sys.stdout))
                missing += 1
                continue
            pat = re.compile(r"\b" + re.escape(sym) + r"\b")
            hit = None
            for i, ln in enumerate(lines, start=1):
                if pat.search(ln):
                    hit = (i, ln.strip())
                    break
            if hit is None:
                print(_color(f"DRIFT    {where} (#{sym})  —— 该符号在目标版本里不存在", RED, sys.stdout))
                drift += 1
            else:
                print(_color(f"ok       {where} (#{sym}) -> {doc}:{hit[0]}  | {hit[1][:90]}", GREEN, sys.stdout))
                resolved += 1
            continue

        if lines is None:
            print(_color(f"MISSING-IN-SNAPSHOT  {where}", RED, sys.stdout))
            missing += 1
            continue

        n = rec["doc_line"]
        if n < 1 or n > len(lines):
            print(_color(f"DRIFT    {where}  (file has {len(lines)} lines)", RED, sys.stdout))
            drift += 1
            continue

        content = lines[n - 1].strip()[:100]
        print(f"ok       {where}  | {content}")
        resolved += 1

    print()
    print(f"resolved={resolved}  drift={drift}  missing-in-snapshot={missing}  total={len(records)}")
    if drift or missing:
        print(_color(f"verify-refs FAILED: {drift} drifted, {missing} missing", RED, sys.stdout))
        return 1
    print(f"verify-refs OK: every citation resolves inside the {SNAPSHOT_DIR.name} snapshot")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Pi behaviour-layer version lock.")
    ap.add_argument("--check", action="store_true", help="verify snapshot sha256 against SNAPSHOT.lock")
    ap.add_argument("--verify-refs", action="store_true", help="re-resolve every Pi line-number citation")
    args = ap.parse_args(argv)

    if not args.check and not args.verify_refs:
        ap.print_help()
        return 2

    try:
        lock = load_lock()
    except (FileNotFoundError, json.JSONDecodeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    code = 0
    if args.check:
        code += check_snapshot(lock)
    if args.verify_refs:
        code += verify_refs(lock)
    return code


if __name__ == "__main__":
    sys.exit(main())
