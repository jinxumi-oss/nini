#!/usr/bin/env python3
"""工具对标（WF-BUILTIN）的判据看板：把计划里的退出判据变成一次运行。

为什么要有这个
--------------
计划里的退出判据如果只写在 Markdown 里，就只能靠人记着去做、去验。
本脚本把它们变成**一次运行的红绿看板**：每一行 = 一个工作项的退出判据，
绿色 = 已达成。

为什么判据都是 `cargo test` 而不是「看输出里有没有 ✗」
------------------------------------------------------
第一版这么做过，而审查指出那是**不可靠的解析**：输出格式一变就可能
「什么都没匹配上」→ 空列表 → 判定为「全绿」。一个**匹配不到东西**的
判据不是判据，是装饰。所以现在每一行都是**一条命令的退出码**。

用法
----
    python3 scripts/tool-parity-check.py            # 看板（会跑 cargo）
    python3 scripts/tool-parity-check.py --json     # 机读
    python3 scripts/tool-parity-check.py --quick    # 只跑不编译的那些

退出码：0 = 全部达成；1 = 还有未达成项（开工前的正常状态）。
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


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


def ls_registered() -> tuple[bool, str]:
    """T2 的静态部分：ls 工具存在、导出、注册。"""
    src = REPO / "crates" / "nini-tools" / "src" / "ls.rs"
    if not src.is_file():
        return False, "crates/nini-tools/src/ls.rs 不存在"
    lib = (REPO / "crates" / "nini-tools" / "src" / "lib.rs").read_text(encoding="utf-8")
    if "pub use ls::LsTool;" not in lib:
        return False, "ls 没有在 nini-tools/src/lib.rs 里导出"
    reg = (REPO / "crates" / "nini-cli" / "src" / "tool_registry.rs").read_text(encoding="utf-8")
    if "LsTool::new()" not in reg:
        return False, "ls 没有注册进 tool_registry"
    return True, "ls 工具已实现、导出并注册"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--quick", action="store_true", help="跳过要跑 cargo 的判据")
    args = ap.parse_args()

    rows: list[dict] = []

    def add(item: str, crit: str, ok: bool, detail: str) -> None:
        rows.append({"item": item, "criterion": crit, "ok": bool(ok), "detail": detail})

    # T1 契约：七个工具的描述/参数/snippet/guidelines 与 0.85.1 逐条一致
    if args.quick:
        add("T1", "cargo test -p nini-tools --test tool_contract_parity（跳过：--quick）",
            False, "quick 模式不跑 cargo")
    else:
        ok, detail = cargo_test("nini-tools", "tool_contract_parity")
        add("T1", "cargo test -p nini-tools --test tool_contract_parity", ok, detail)

    # T2 ls 工具
    ok, detail = ls_registered()
    add("T2", "ls 工具已实现、导出并注册（静态判定）", ok, detail)
    if not args.quick:
        ok2, detail2 = cargo_test("nini-tools", "tool_contract_parity")
        add("T2", "cargo test -p nini-tools --test tool_contract_parity（ls 的契约也对齐）",
            ok2, detail2)

    if not args.quick:
        # T3 prompt 联动
        ok, detail = cargo_test("nini-core", "prompt_tool_linkage")
        add("T3", "cargo test -p nini-core --test prompt_tool_linkage", ok, detail)
        # T4 截断
        ok, detail = cargo_test("nini-tools", "truncation_parity")
        add("T4", "cargo test -p nini-tools --test truncation_parity", ok, detail)

    done = sum(1 for r in rows if r["ok"])
    if args.json:
        print(json.dumps({"done": done, "total": len(rows), "rows": rows},
                         ensure_ascii=False, indent=2))
    else:
        print("工具对标（WF-BUILTIN）判据看板")
        for r in rows:
            print(f"{r['item']:<4}{'✓' if r['ok'] else '✗':<6}{r['criterion']}")
            if not r["ok"] and r["detail"]:
                print(f"{'':<10}└ {r['detail'][:200]}")
        print(f"\n{done}/{len(rows)} 条判据达成")
    return 0 if done == len(rows) else 1


if __name__ == "__main__":
    raise SystemExit(main())
