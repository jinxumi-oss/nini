#!/usr/bin/env python3
"""从快照里提取 Pi 的**工具契约**（名称 / 描述 / 参数 schema），并与 nini 的比对。

为什么需要它
------------
工具的「契约」= 模型看到的那份描述 + 参数 schema。它直接决定模型怎么用工具，
所以它是**行为面**，不是实现细节。

nini 现在的工具描述是**从 Pi 0.84.2 捕获**的（crates/nini-tools/src/*.rs 里
逐字抄的）。目标统一到 0.85.1 之后，这些捕获就过期了——而"过期"不会自己报错：
模型照样按旧描述调用工具，差异只会表现为「成功率低了一点」。

本脚本把两侧的契约拉成机读 JSON，然后逐条比对：
  * `references/pi-0.85.1/tool-contracts.json` —— Pi 侧（真相）
  * nini 侧 —— 从 Rust 源码里抽出描述文本

用法
----
    python3 scripts/pi-tool-contracts.py --extract          # 只生成 Pi 侧契约
    python3 scripts/pi-tool-contracts.py --compare          # 与 nini 逐条比对
    python3 scripts/pi-tool-contracts.py --compare --json
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SNAPSHOT = REPO / "references" / "pi-0.85.1"

# 工具定义所在的快照文件（按 Pi 自己的分层：harness 层 / coding 层）
#
# Pi 的**扩展层**不在这里：Pi 默认**一个扩展工具都不带**（唯一内置扩展
# llama.cpp 是隐藏的，且不注册工具）。扩展工具的契约在
# `dist/core/extensions/types.d.ts` 的 `ToolDefinition`，与内置工具**不是**同一类东西：
# 它能自带 renderCall/renderResult、拿 onUpdate 与 signal、贡献 prompt 片段。
# 内置工具则是「进程内的函数 + schema」。两者不可混谈。
TOOL_FILES = {
    "read": "tools-t1/read.js",
    "write": "tools-t1/write.js",
    "edit": "tools-t1/edit.js",
    "bash": "tools-t1/bash.js",
    "grep": "tools-t2/grep.js",
    "find": "tools-t2/find.js",
    "ls": "tools-t2/ls.js",
}

# Pi 自己把 read/bash/edit/write 称为 "builtin"（sdk.d.ts 的 noTools: "builtin"），
# grep/find/ls/powershell 是 coding 层的内置工具。两者都是**原始自带**，
# 但归属不同：加一个品牌/工具时改的地方不同。
PI_BUILTIN = {"read", "bash", "edit", "write"}
NINI_FILES = {
    "read": "crates/nini-tools/src/read.rs",
    "write": "crates/nini-tools/src/write.rs",
    "edit": "crates/nini-tools/src/edit.rs",
    "bash": "crates/nini-tools/src/bash.rs",
    "grep": "crates/nini-tools/src/grep.rs",
    "find": "crates/nini-tools/src/find.rs",
}


def extract_description(text: str) -> str | None:
    """取工具级 description（最长的那个字符串字面量，通常就是它）。"""
    cands = re.findall(r'description:\s*("(?:[^"\\]|\\.){40,}"|`(?:[^`\\]|\\.){40,}`)', text, re.S)
    if not cands:
        return None
    best = max(cands, key=len)
    if best.startswith("`"):
        best = best[1:-1]
    else:
        best = best[1:-1]
    return best.replace("\\n", "\n").strip()


def extract_params(text: str) -> list[str]:
    """参数名：从 schema 的 properties 里取。"""
    m = re.search(r"properties:\s*\{(.*?)\n\s*\}", text, re.S)
    if not m:
        return []
    return sorted(set(re.findall(r"^\s*([A-Za-z_][A-Za-z0-9_]*):\s*Type\.", m.group(1), re.M)))


def nini_description(path: Path) -> str | None:
    text = path.read_text(encoding="utf-8", errors="replace")
    # nini 的描述通常在 ToolSpec 的 description 字段，或一段 doc 注释里
    m = re.search(r'description:\s*(r?"(?:[^"\\]|\\.){40,}"|r#"(?:[^"\\]|\\.){40,}")', text, re.S)
    if m:
        s = m.group(1)
        s = re.sub(r'^r?#?"', "", s)
        s = s.rstrip('"')
        return s.replace('\\n', '\n').strip()
    # 退化：取最长的行注释块
    blocks = re.findall(r'(?s)((?:\s*///.*\n){3,})', text)
    if blocks:
        b = max(blocks, key=len)
        lines = [re.sub(r'^\s*///\s?', '', l) for l in b.splitlines() if l.strip()]
        return "\n".join(lines).strip()
    return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--extract", action="store_true")
    ap.add_argument("--compare", action="store_true")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    contracts: dict[str, dict] = {}
    for name, rel in TOOL_FILES.items():
        p = SNAPSHOT / "dist" / rel
        if not p.is_file():
            continue
        text = p.read_text(encoding="utf-8", errors="replace")
        contracts[name] = {
            "description": extract_description(text),
            "params": extract_params(text),
            "source": rel,
            "layer": "builtin(harness)" if name in PI_BUILTIN else "builtin(coding)",
            "kind": "original-builtin",
        }

    out_path = SNAPSHOT / "tool-contracts.json"
    if args.extract or not args.compare:
        out_path.write_text(json.dumps(
            {
                "piVersion": "0.85.1",
                "layers": {
                    "original-builtin": sorted(contracts),
                    "extension": [],
                    "note": "Pi 默认不带任何扩展工具；扩展工具的契约见 "
                            "dist/core/extensions/types.d.ts 的 ToolDefinition，"
                            "与内置工具不是同一类东西。",
                },
                "tools": contracts,
            },
            ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8")
        print(f"Pi 侧工具契约 → {out_path}（{len(contracts)} 个工具）")

    if not args.compare:
        return 0

    rows = []
    for name, c in sorted(contracts.items()):
        nf = REPO / NINI_FILES.get(name, "")
        ndesc = nini_description(nf) if nf.is_file() else None
        same = (ndesc or "").strip() == (c["description"] or "").strip()
        rows.append({
            "tool": name,
            "pi_params": c["params"],
            "nini_params": None,
            "pi_description_len": len(c["description"] or ""),
            "nini_description_len": len(ndesc or ""),
            "identical": same,
            "pi_description": c["description"],
            "nini_description": ndesc,
        })

    if args.json:
        print(json.dumps(rows, ensure_ascii=False, indent=2))
    else:
        print(f"\n{'工具':<8}{'层':<20}{'一致':<6}{'Pi 描述字数':<12}{'nini 描述字数':<14}")
        for r in rows:
            layer = contracts[r['tool']]['layer']
            print(f"{r['tool']:<8}{layer:<20}{'✓' if r['identical'] else '✗':<6}"
                  f"{r['pi_description_len']:<12}{r['nini_description_len']:<14}")
        print("\n扩展层：Pi 默认 0 个扩展工具（唯一的内置扩展 llama.cpp 是隐藏的，且不注册工具）。")
        bad = [r["tool"] for r in rows if not r["identical"]]
        if bad:
            print(f"\n⚠ 与 0.85.1 不一致：{bad}")
            print("  （nini 的描述是从 Pi 0.84.2 捕获的；目标统一后需要重新捕获）")
        else:
            print("\n全部与 0.85.1 一致")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
