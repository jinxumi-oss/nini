#!/usr/bin/env python3
"""Machine-checkable ledger of the nini ↔ Pi parity gaps.

The gap table in ``docs/plans/parity-with-pi.md`` §2 used to be hand-maintained
Markdown, which made every count in the document a claim instead of a fact.
This script turns that table into an enumerable ledger (``scripts/parity-gaps.json``)
and cross-checks the ledger back against the plan document.

Usage
-----
    python3 scripts/parity-gap.py --json                    # machine readable
    python3 scripts/parity-gap.py                          # human readable table
    python3 scripts/parity-gap.py --check-plan docs/plans/parity-with-pi.md
    python3 scripts/parity-gap.py --check-plan              # default plan path

Exit codes
----------
    0  the ledger is self-consistent (and, with --check-plan, agrees with the plan)
    1  cross-check failure: the plan document and the ledger disagree
    2  the ledger file itself is broken (bad JSON, missing field, duplicate id,
       value outside the whitelist)
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import Counter
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
LEDGER_FILE = Path(
    os.environ.get("NINI_PARITY_GAPS", Path(__file__).resolve().parent / "parity-gaps.json")
)
DEFAULT_PLAN = REPO_ROOT / "docs" / "plans" / "parity-with-pi.md"

SCHEMA_VERSION = 1

# Whitelists. Anything outside these is a data error, not a judgement call.
CLASSES = ("A", "B", "C", "D")
SEVERITIES = ("P0", "P1", "P2", "P3")
STATUSES = ("open", "fixed", "blocked-owner", "blocked-external")
ROUNDS = ("W0", "W1-A", "W1-B", "W1-C", "W2", "W3", "W4", "W5", "—",
          # 工具对标计划的四个工作项（docs/plans/tool-parity.md）
          "T1", "T2", "T3", "T4", "T2+T3",
          # v0.11 计划（docs/plans/next-parity.md）：本轮关掉的几条
          # 差距，以及本轮**执行中才发现**的那条（中止信号没有消费者）
          "v0.11",
          # v0.12 计划（docs/plans/next-parity.md）：长任务的轮数上限
          # （A8）——一条 nini 独有、Pi 完全没有的截断。
          "v0.12")
DECISIONS = ("R4", "R5", "R9", "R10", "R11")

# Every field a gap record must carry, with the type it must have.
REQUIRED_FIELDS = {
    "id": str,
    "class": str,
    "severity": str,
    "title": str,
    "pi_basis": str,
    "nini_state": str,
    "oracle": str,
    "status": str,
    "blocking_decision": (str, type(None)),
    "round": str,
    "exit_criterion": str,
    "machine_checkable": bool,
}

# Statuses that mean "the gap is still open for business".
OPEN_STATUSES = ("open", "blocked-owner", "blocked-external")

# §2 of the plan lists C1b as a second row under C1. It is the same
# meta-problem (hand-verification is unreliable), so the ledger folds it into
# C1 instead of counting it twice. The mapping is explicit, never silent.
DOC_ID_ALIASES = {"C1b": "C1"}

# A table cell that holds nothing but a gap id: | **A1** |  or  | `B1b` |
# The class letter is restricted to A-D on purpose: the severity cells of the
# same table look like | **P0** | and must not be mistaken for gap ids.
ID_CELL_RE = re.compile(r"\|\s*\*{0,2}`?([A-D]\d+[a-z]?)`?\*{0,2}\s*\|")
# A metrics row in §5: | A 类未闭 | §2 A 表 | 4 |
METRIC_ROW_RE = re.compile(r"^\|(?P<label>[^|]+)\|(?P<method>[^|]+)\|(?P<value>[^|]+)\|\s*$")


class LedgerError(Exception):
    """The ledger file itself is broken."""


def load_ledger(path: Path = LEDGER_FILE) -> dict:
    if not path.is_file():
        raise LedgerError(f"ledger not found: {path}")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise LedgerError(f"{path}: invalid JSON: {exc}") from exc
    if not isinstance(data, dict):
        raise LedgerError(f"{path}: top level must be an object")
    return data


def validate(data: dict) -> list[str]:
    """Return a list of data errors (empty means the ledger is self-consistent)."""
    errors: list[str] = []

    if data.get("schema") != SCHEMA_VERSION:
        errors.append(f"schema must be {SCHEMA_VERSION}, got {data.get('schema')!r}")
    for key in ("updated", "authority", "detail"):
        if not str(data.get(key, "")).strip():
            errors.append(f"top-level field {key!r} is missing or empty")

    gaps = data.get("gaps")
    if not isinstance(gaps, list) or not gaps:
        errors.append("'gaps' must be a non-empty list")
        return errors

    seen_ids: dict[str, int] = {}
    for index, gap in enumerate(gaps):
        where = f"gaps[{index}]"
        if not isinstance(gap, dict):
            errors.append(f"{where}: must be an object")
            continue
        gid = gap.get("id")
        if isinstance(gid, str) and gid:
            where = f"gaps[{index}] (id={gid})"
            if gid in seen_ids:
                errors.append(f"{where}: duplicate id (also at gaps[{seen_ids[gid]}])")
            else:
                seen_ids[gid] = index

        for field, expected in REQUIRED_FIELDS.items():
            if field not in gap:
                errors.append(f"{where}: missing field {field!r}")
                continue
            value = gap[field]
            if expected is str:
                if not isinstance(value, str) or not value.strip():
                    errors.append(f"{where}: field {field!r} must be a non-empty string")
            elif expected is bool:
                if not isinstance(value, bool):
                    errors.append(f"{where}: field {field!r} must be a boolean")
            elif isinstance(expected, tuple):
                if not isinstance(value, expected):
                    errors.append(f"{where}: field {field!r} must be {expected} , got {type(value).__name__}")

        cls = gap.get("class")
        if isinstance(cls, str) and cls not in CLASSES:
            errors.append(f"{where}: class {cls!r} not in {list(CLASSES)}")
        sev = gap.get("severity")
        if isinstance(sev, str) and sev not in SEVERITIES:
            errors.append(f"{where}: severity {sev!r} not in {list(SEVERITIES)}")
        status = gap.get("status")
        if isinstance(status, str) and status not in STATUSES:
            errors.append(f"{where}: status {status!r} not in {list(STATUSES)}")
        rnd = gap.get("round")
        if isinstance(rnd, str) and rnd not in ROUNDS:
            errors.append(f"{where}: round {rnd!r} not in {list(ROUNDS)}")
        decision = gap.get("blocking_decision")
        if isinstance(decision, str) and decision not in DECISIONS:
            errors.append(f"{where}: blocking_decision {decision!r} not in {list(DECISIONS)}")
        if status in ("blocked-owner", "blocked-external") and not decision:
            errors.append(f"{where}: status {status!r} requires a blocking_decision")

    return errors


def summarize(data: dict) -> dict:
    gaps = data["gaps"]
    by_class = Counter(g["class"] for g in gaps)
    by_status = Counter(g["status"] for g in gaps)
    by_round = Counter(g["round"] for g in gaps)
    decisions = Counter(g["blocking_decision"] for g in gaps if g.get("blocking_decision"))
    machine = sum(1 for g in gaps if g["machine_checkable"])
    return {
        "schema": data.get("schema", SCHEMA_VERSION),
        "total": len(gaps),
        "by_class": {c: by_class.get(c, 0) for c in CLASSES},
        "by_status": {s: by_status.get(s, 0) for s in STATUSES if by_status.get(s)},
        "by_round": {r: by_round.get(r, 0) for r in ROUNDS if by_round.get(r)},
        "machine_checkable": machine,
        "not_machine_checkable": len(gaps) - machine,
        "decisions_blocking": {d: decisions.get(d, 0) for d in DECISIONS},
        "gaps": gaps,
    }


def _section(text: str, number: int) -> str:
    """Return the body of the '## N.' section of a Markdown document."""
    pattern = re.compile(rf"^##\s+{number}\..*?$", re.MULTILINE)
    match = pattern.search(text)
    if not match:
        return ""
    rest = text[match.end():]
    nxt = re.search(r"^##\s+\d+\.", rest, re.MULTILINE)
    return rest[: nxt.start()] if nxt else rest


def check_plan(data: dict, plan_path: Path) -> tuple[int, list[str]]:
    """Cross-check the ledger against the plan document. Returns (code, notes)."""
    notes: list[str] = []
    failures: list[str] = []

    if not plan_path.is_file():
        return 1, [f"plan document not found: {plan_path}"]

    text = plan_path.read_text(encoding="utf-8")
    gaps = data["gaps"]
    ledger_ids = {g["id"] for g in gaps}

    # ---- 1. every gap id in the plan's §2 table must exist in the ledger ----
    section2 = _section(text, 2)
    doc_ids: list[str] = []
    for line in section2.splitlines():
        if not line.strip().startswith("|"):
            continue
        for m in ID_CELL_RE.finditer(line):
            gid = m.group(1)
            if gid not in doc_ids:
                doc_ids.append(gid)

    for gid in doc_ids:
        target = DOC_ID_ALIASES.get(gid, gid)
        if target not in ledger_ids:
            failures.append(f"plan §2 lists gap {gid} but the ledger has no such id")
        elif target != gid:
            notes.append(f"plan §2 gap {gid} is folded into ledger id {target} (see DOC_ID_ALIASES)")
    notes.append(f"plan §2 table: {len(doc_ids)} gap id(s) checked against the ledger")

    # ---- 2. the counts the plan declares must match the ledger ----
    section5 = _section(text, 5)
    declared: dict[str, int] = {}
    excluded: set[str] = set()
    for line in section5.splitlines():
        m = METRIC_ROW_RE.match(line)
        if not m:
            continue
        label = m.group("label").strip()
        method = m.group("method").strip()
        value = m.group("value").strip()
        if "未闭" not in label:
            continue
        num = re.search(r"\d+", value)
        if not num:
            continue
        key = "A" if label.startswith("A") else ("B" if label.startswith("B") else label)
        declared[key] = int(num.group(0))
        excluded.update(re.findall(r"不含\s*([A-Z]\d+[a-z]?)", method))

    if not declared:
        notes.append("plan §5 declares no 「未闭」 counts — nothing to compare")

    for key, want in sorted(declared.items()):
        got = sum(
            1
            for g in gaps
            if g["class"] == key and g["status"] in OPEN_STATUSES and g["id"] not in excluded
        )
        if got != want:
            failures.append(
                f"plan §5 declares 「{key} 类未闭」 = {want}, ledger says {got}"
                + (f" (excluding {sorted(excluded)})" if excluded else "")
            )
        else:
            notes.append(f"plan §5 「{key} 类未闭」 = {want} matches the ledger")

    # ---- 3. the recorded defect count ----
    defects = sum(1 for g in gaps if "#[ignore]" in g.get("oracle", ""))
    for line in section5.splitlines():
        if "实测缺陷" not in line:
            continue
        m = METRIC_ROW_RE.match(line)
        if not m:
            continue
        num = re.search(r"\d+", m.group("value"))
        if num:
            want = int(num.group(0))
            if defects != want:
                failures.append(f"plan §5 declares 「实测缺陷」 = {want}, ledger says {defects}")
            else:
                notes.append(f"plan §5 「实测缺陷」 = {want} matches the ledger")

    # ---- 4. (v3) every gap id the plan MENTIONS must exist in the ledger ----
    #
    # v3 moved the gap table out of the document and into the ledger, so
    # the old "read the ids out of a Markdown table" check found nothing and
    # passed vacuously — a check that cannot fail is decoration. This one
    # works on the prose: the plan names gaps ("A1 是 P0", "A5 实测不落盘"),
    # and every name it uses must be a real ledger entry. A plan that
    # invents an id, or that still talks about a gap the ledger has
    # renamed, fails here.
    mentioned = []
    for m in re.finditer(r"\b([ABCD]\d+[a-z]?)\b", text):
        gid = m.group(1)
        if gid not in mentioned:
            mentioned.append(gid)
    if not mentioned:
        failures.append(
            "the plan mentions no gap id at all — it has drifted away from "
            "the ledger (and this check would otherwise pass vacuously)"
        )
    else:
        unknown = []
        for gid in mentioned:
            target = DOC_ID_ALIASES.get(gid, gid)
            if target not in ledger_ids:
                unknown.append(gid)
        if unknown:
            failures.append(
                f"the plan mentions gap id(s) {sorted(unknown)} that the ledger "
                "does not have"
            )
        else:
            notes.append(
                f"plan mentions {len(mentioned)} gap id(s); all of them exist "
                "in the ledger"
            )

    # ---- 5. (v3) a gap the ledger calls fixed must be recorded in the plan ----
    unrecorded = [
        g["id"] for g in gaps
        if g.get("status") == "fixed" and g["id"] not in mentioned
    ]
    if unrecorded:
        failures.append(
            f"the ledger says {unrecorded} are fixed, but the plan does not "
            "mention them — a fix that is not written down is a fix nobody "
            "can find"
        )
    else:
        fixed = [g["id"] for g in gaps if g.get("status") == "fixed"]
        notes.append(
            f"every fixed gap ({', '.join(fixed) if fixed else 'none'}) is "
            "recorded in the plan"
        )

    for note in notes:
        print(f"note     {note}")
    for failure in failures:
        print(f"FAIL     {failure}")
    if failures:
        print(f"\nplan cross-check FAILED: {len(failures)} disagreement(s)")
        return 1, notes
    print("\nplan cross-check OK: the ledger and the plan agree")
    return 0, notes


def _human(data: dict, summary: dict) -> None:
    print(f"parity gap ledger — {summary['total']} gaps, schema {summary['schema']}")
    print(f"by class:   {summary['by_class']}")
    print(f"by status:  {summary['by_status']}")
    print(f"by round:   {summary['by_round']}")
    print(
        f"machine-checkable: {summary['machine_checkable']}  "
        f"not machine-checkable: {summary['not_machine_checkable']}"
    )
    print(f"blocking decisions: {summary['decisions_blocking']}")
    print()
    header = f"{'id':<5} {'cls':<4} {'sev':<4} {'status':<16} {'round':<6} {'mc':<3} title"
    print(header)
    print("-" * len(header))
    for g in summary["gaps"]:
        mc = "yes" if g["machine_checkable"] else "no"
        decision = f" [{g['blocking_decision']}]" if g.get("blocking_decision") else ""
        print(f"{g['id']:<5} {g['class']:<4} {g['severity']:<4} {g['status']:<16} {g['round']:<6} {mc:<3} {g['title']}{decision}")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="nini ↔ Pi parity gap ledger.")
    ap.add_argument("--json", action="store_true", help="machine readable output")
    ap.add_argument(
        "--check-plan",
        nargs="?",
        const=str(DEFAULT_PLAN),
        default=None,
        metavar="PATH",
        help="cross-check the ledger against the plan document",
    )
    args = ap.parse_args(argv)

    try:
        data = load_ledger()
    except LedgerError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    errors = validate(data)
    if errors:
        print("ledger data errors:", file=sys.stderr)
        for err in errors:
            print(f"  - {err}", file=sys.stderr)
        return 2

    summary = summarize(data)

    if args.json:
        print(json.dumps(summary, ensure_ascii=False, indent=2))
    else:
        _human(data, summary)

    if args.check_plan:
        code, _ = check_plan(data, Path(args.check_plan))
        return code

    return 0


if __name__ == "__main__":
    sys.exit(main())
