#!/usr/bin/env python3
"""v0.11 计划（`docs/plans/next-parity.md`）的判据看板。

为什么要有这个
--------------
计划里的退出判据如果只写在 Markdown 里，就只能靠人记着去做、去验。
本脚本把它们变成**一次运行的红绿看板**：每一行 = 一个工作项的退出判据，
绿色 = 已达成。

为什么判据都是 `cargo test` / `python3` 的退出码
------------------------------------------------
判据写成「输出里有没有 ✗」是不可靠的解析：输出格式一变，就可能
「什么都没匹配上」→ 空列表 → 判定为「全绿」。一个**匹配不到东西**的
判据不是判据，是装饰。所以每一行都是**一条命令的退出码**。

用法
----
    python3 scripts/next-parity-check.py            # 看板（会跑 cargo）
    python3 scripts/next-parity-check.py --json     # 机读
    python3 scripts/next-parity-check.py --quick    # 只跑不编译的那些

退出码：0 = 全部达成；1 = 还有未达成项（开工前的正常状态）。
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# 本轮**顺延**的工作项：它们不算「未达成」，但也不能假装不存在。
#
# （v0.11 的 A4 双队列已在 2026-10-27 做完，顺延清单因此清空。）
DEFERRED: list[tuple[str, str]] = []


def sh(cmd: list[str], timeout: int = 2400) -> tuple[int, str]:
    try:
        r = subprocess.run(cmd, cwd=REPO, capture_output=True, text=True, timeout=timeout)
        return r.returncode, (r.stdout or "") + (r.stderr or "")
    except subprocess.TimeoutExpired:
        return 1, "timeout"


def cargo_test(pkg: str, test: str) -> tuple[bool, str]:
    """一条判据 = 一条命令的退出码。"""
    code, out = sh(["cargo", "test", "-p", pkg, "--test", test])
    if code == 0:
        return True, f"cargo test -p {pkg} --test {test} 退出 0"
    tail = "\n".join(out.splitlines()[-8:])
    return False, f"cargo test -p {pkg} --test {test} 失败：\n{tail[:400]}"


def script_ok(args: list[str], label: str) -> tuple[bool, str]:
    code, out = sh(args)
    if code == 0:
        return True, f"{label} 退出 0"
    tail = "\n".join(out.splitlines()[-6:])
    return False, f"{label} 失败：\n{tail[:400]}"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--quick", action="store_true", help="跳过要跑 cargo 的判据")
    args = ap.parse_args()

    rows: list[dict] = []

    def add(item: str, crit: str, ok: bool, detail: str) -> None:
        rows.append({"item": item, "criterion": crit, "ok": bool(ok), "detail": detail})

    # N0 —— 对照物进快照：引用必须可解析
    ok, detail = script_ok(
        ["python3", "scripts/pi-oracle-lock.py", "--check"], "pi-oracle-lock --check"
    )
    add("N0", "python3 scripts/pi-oracle-lock.py --check", ok, detail)

    ok, detail = script_ok(
        ["python3", "scripts/pi-oracle-lock.py", "--verify-refs"], "pi-oracle-lock --verify-refs"
    )
    add("N0", "python3 scripts/pi-oracle-lock.py --verify-refs（drift=0）", ok, detail)

    if not args.quick:
        ok, detail = cargo_test("nini-conformance", "citations_resolve")
        add("N0", "cargo test -p nini-conformance --test citations_resolve", ok, detail)

        # N1 —— A2b：Esc 在压缩期间只中止压缩
        # N5 —— A7：中止信号真的有消费者（同一批 oracle）
        ok, detail = cargo_test("nini-tui", "compaction_escape")
        add("N1", "cargo test -p nini-tui --test compaction_escape", ok, detail)
        add("N5", "cargo test -p nini-tui --test compaction_escape（含 A7 的信号断言）", ok, detail)

        # N2 —— A4：steering / followUp 两个队列
        ok, detail = cargo_test("nini-tui", "queue_routing")
        add("N2", "cargo test -p nini-tui --test queue_routing", ok, detail)

        # N3 —— B5：double-ESC
        ok, detail = cargo_test("nini-tui", "double_escape")
        add("N3", "cargo test -p nini-tui --test double_escape", ok, detail)

        # N4 —— B3：/reload 的范围
        ok, detail = cargo_test("nini-tui", "reload_scope")
        add("N4", "cargo test -p nini-tui --test reload_scope", ok, detail)
    else:
        for item, crit in (
            ("N1", "cargo test -p nini-tui --test compaction_escape"),
            ("N5", "cargo test -p nini-tui --test compaction_escape"),
            ("N2", "cargo test -p nini-tui --test queue_routing"),
            ("N3", "cargo test -p nini-tui --test double_escape"),
            ("N4", "cargo test -p nini-tui --test reload_scope"),
        ):
            add(item, f"{crit}（跳过：--quick）", False, "quick 模式不跑 cargo")

    done = sum(1 for r in rows if r["ok"])
    if args.json:
        print(json.dumps({"done": done, "total": len(rows), "rows": rows,
                          "deferred": DEFERRED},
                         ensure_ascii=False, indent=2))
    else:
        print("v0.11 计划（next-parity）判据看板")
        for r in rows:
            print(f"{r['item']:<4}{'✓' if r['ok'] else '✗':<6}{r['criterion']}")
            if not r["ok"] and r["detail"]:
                print(f"{'':<10}└ {r['detail'][:200]}")
        for item, why in DEFERRED:
            print(f"{item:<4}–     {why}")
        print(f"\n{done}/{len(rows)} 条判据达成")
    return 0 if done == len(rows) else 1


if __name__ == "__main__":
    raise SystemExit(main())
