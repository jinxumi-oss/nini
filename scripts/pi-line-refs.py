#!/usr/bin/env python3
"""Enumerate every Pi source line-number reference embedded in nini's Rust code.

Why this exists
---------------
nini's oracle comments cite Pi internals as ``agent-loop.js:160`` /
``keybindings.ts:208``.  Those line numbers are *not* checked by anything:
when Pi is upgraded the compiled bundle is rewritten and every citation
silently drifts to point at the wrong statement.  Nothing turns red.

This script turns those citations into an enumerable ledger so that
``scripts/pi-oracle-lock.py`` can re-resolve each one against the frozen
Pi 0.84.2 snapshot in ``references/pi-0.84.2/``.

Usage
-----
    python3 scripts/pi-line-refs.py --json    # machine readable
    python3 scripts/pi-line-refs.py           # human readable table

Exit codes
----------
    0  scan completed (the script's job is enumeration, not judgement)
    2  the repository layout is not what this script expects
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path

# The citation shape used in nini's comments.
#
# 两种形态，都必须有：
#   行号引用   agent-loop.js:160        —— 脆：Pi 一升级就漂
#   **符号引用** agent-loop.js#shouldStopAfterTurn —— 稳：含义被钉住，
#               行号由脚本在目标版本里解析出来。升级只会让脚本报告
#               「符号从 160 挪到了 172」，而不会让引用悄悄指错。
#
# 2026-10-23 实测：0.84.2 → 0.85.1 之间，90 处行号引用只有 3 处还指向
# 同一行。所以目标统一到单一版本之后，**行号引用必须能被机器复核**。
REF_RE = re.compile(
    r"(agent-loop|interactive-mode|agent-session|keybindings|agent-model|tui-alt-screen"
    r"|fuzzy|model-search|settings-manager)\.(js|ts)"
    r"(?::(\d+)|#([A-Za-z_$][A-Za-z0-9_$]*))"
)

# How much of the citing line is kept in the ledger record.
TEXT_LIMIT = 80

REPO_ROOT = Path(__file__).resolve().parent.parent
CRATES_DIR = REPO_ROOT / "crates"


def _iter_rust_files(root: Path):
    """Yield every .rs file under root, skipping build artefacts."""
    for path in sorted(root.rglob("*.rs")):
        parts = set(path.parts)
        if "target" in parts or "node_modules" in parts:
            continue
        yield path


def scan(root: Path = CRATES_DIR) -> list[dict]:
    """Return one record per Pi line-number reference found in the tree."""
    records: list[dict] = []
    for path in _iter_rust_files(root):
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        rel = path.relative_to(REPO_ROOT).as_posix()
        for lineno, line in enumerate(text.splitlines(), start=1):
            for m in REF_RE.finditer(line):
                excerpt = line.strip()
                if len(excerpt) > TEXT_LIMIT:
                    excerpt = excerpt[:TEXT_LIMIT] + "…"
                records.append(
                    {
                        "file": rel,
                        "line": lineno,
                        "doc": f"{m.group(1)}.{m.group(2)}",
                        "kind": "symbol" if m.group(4) else "line",
                        "symbol": m.group(4),
                        "doc_line": int(m.group(3)) if m.group(3) else None,
                        "text": excerpt,
                    }
                )
    return records


def _summary(records: list[dict]) -> dict:
    by_doc = Counter(r["doc"] for r in records)
    by_kind = Counter(r.get("kind", "line") for r in records)
    by_file = Counter(r["file"] for r in records)
    return {
        "total": len(records),
        "by_kind": dict(sorted(by_kind.items())),
        "by_doc": dict(sorted(by_doc.items())),
        "by_file": dict(sorted(by_file.items())),
        "refs": records,
    }


def _human(records: list[dict]) -> None:
    by_doc = Counter(r["doc"] for r in records)
    by_kind = Counter(r.get("kind", "line") for r in records)
    by_file = Counter(r["file"] for r in records)
    print(f"Pi line-number references in crates/: {len(records)}")
    print()
    print("by referenced document:")
    for doc, n in sorted(by_doc.items()):
        print(f"  {doc:<24} {n}")
    print()
    print(f"by citing file ({len(by_file)} files):")
    for f, n in sorted(by_file.items()):
        print(f"  {f:<56} {n}")
    print()
    print("records:")
    for r in records:
        print(f"  {r['file']}:{r['line']} -> {r['doc']}:{r['doc_line']}  | {r['text']}")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Enumerate Pi line-number references in crates/.")
    ap.add_argument("--json", action="store_true", help="machine readable output")
    args = ap.parse_args(argv)

    if not CRATES_DIR.is_dir():
        print(f"error: {CRATES_DIR} not found — run from the nini repository", file=sys.stderr)
        return 2

    records = scan()
    if args.json:
        print(json.dumps(_summary(records), ensure_ascii=False, indent=2))
    else:
        _human(records)
    return 0


if __name__ == "__main__":
    sys.exit(main())
