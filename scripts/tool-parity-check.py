#!/usr/bin/env python3
"""工具对标（WF-BUILTIN）的判据看板：把计划里的退出判据变成一次运行。

为什么要有这个
--------------
计划里的退出判据如果只写在 Markdown 里，就只能靠人记着去做、去验。
本脚本把它们变成**一次运行的红绿看板**：
每一行 = 一个工作项的退出判据，绿色 = 已达成。

用法
----
    python3 scripts/tool-parity-check.py            # 看板
    python3 scripts/tool-parity-check.py --json     # 机读
    python3 scripts/tool-parity-check.py --quick    # 跳过 cargo 的那几行

退出码：0 = 全部达成；1 = 还有未达成项（这是**开工前**的正常状态）。
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


def sh(cmd: list[str], timeout: int = 900) -> tuple[int, str]:
    try:
        r = subprocess.run(cmd, cwd=REPO, capture_output=True, text=True, timeout=timeout)
        out = (r.stdout or "") + (r.stderr or "")
        return r.returncode, out
    except subprocess.TimeoutExpired:
        return 1, "timeout"


def contracts_compare() -> tuple[bool, str]:
    """T1：nini 的工具描述/参数是否与 0.85.1 一致。"""
    code, out = sh([sys.executable, "scripts/pi-tool-contracts.py", "--compare"])
    lines = [l for l in out.splitlines() if l.strip()]
    # 脚本在有差异时打印「⚠ 与 0.85.1 不一致」；更稳的判据是**逐条**看 ✗
    bad = [l for l in lines if l.startswith(("bash ", "edit ", "find ", "grep ", "ls ", "read ", "write "))
           and "✗" in l]
    if bad:
        return False, f"{len(bad)} 个工具与 0.85.1 不一致：" + " ".join(
            l.split()[0] for l in bad)
    if code != 0:
        return False, "pi-tool-contracts.py 退出非 0"
    return True, "全部工具的描述与 0.85.1 一致"


def ls_tool_present() -> tuple[bool, str]:
    """T2：nini 有没有 ls 工具（且注册了）。"""
    src = REPO / "crates" / "nini-tools" / "src" / "ls.rs"
    if not src.is_file():
        return False, "crates/nini-tools/src/ls.rs 不存在"
    lib = (REPO / "crates" / "nini-tools" / "src" / "lib.rs").read_text(encoding="utf-8")
    if "ls" not in lib:
        return False, "ls 没有在 nini-tools/src/lib.rs 里导出"
    reg = (REPO / "crates" / "nini-cli" / "src" / "tool_registry.rs").read_text(encoding="utf-8")
    if '"ls"' not in reg:
        return False, "ls 没有注册进 tool_registry"
    return True, "ls 工具已实现并注册"


def cargo_test(pkg: str, test: str) -> tuple[bool, str]:
    code, out = sh(["cargo", "test", "-p", pkg, "--test", test], timeout=1800)
    if code == 0:
        return True, f"{pkg}/{test} 全绿"
    tail = "\n".join(out.splitlines()[-6:])
    return False, f"{pkg}/{test} 失败：{tail[:300]}"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--quick", action="store_true", help="跳过要跑 cargo 的判据")
    args = ap.parse_args()

    rows: list[dict] = []

    def add(item: str, crit: str, ok: bool, detail: str) -> None:
        rows.append({"item": item, "criterion": crit, "ok": bool(ok), "detail": detail})

    # T1 契约
    ok, detail = contracts_compare()
    add("T1", "pi-tool-contracts.py --compare：7 个工具的描述与 0.85.1 逐条一致", ok, detail)

    # T2 ls 工具
    ok, detail = ls_tool_present()
    add("T2", "nini-tools 有 ls 工具且已注册", ok, detail)

    if not args.quick:
        # T3 prompt 联动
        ok, detail = cargo_test("nini-core", "prompt_tool_linkage")
        add("T3", "cargo test -p nini-core --test prompt_tool_linkage：工具集→prompt 的四种组合", ok, detail)
        # T4 截断
        ok, detail = cargo_test("nini-tools", "truncation_parity")
        add("T4", "cargo test -p nini-tools --test truncation_parity：截断语义与 Pi 的 50KB/行宽一致", ok, detail)

    done = sum(1 for r in rows if r["ok"])
    if args.json:
        print(json.dumps({"done": done, "total": len(rows), "rows": rows},
                         ensure_ascii=False, indent=2))
    else:
        print("工具对标（WF-BUILTIN）判据看板")
        print(f"{'项':<4}{'达成':<6}判据")
        for r in rows:
            print(f"{r['item']:<4}{'✓' if r['ok'] else '✗':<6}{r['criterion']}")
            if not r["ok"] and r["detail"]:
                print(f"{'':<10}└ {r['detail'][:150]}")
        print(f"\n{done}/{len(rows)} 条判据达成")
    return 0 if done == len(rows) else 1


if __name__ == "__main__":
    raise SystemExit(main())
