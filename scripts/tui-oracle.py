#!/usr/bin/env python3
"""TUI 对照 oracle（**实验性，提取器尚未可信，不得当作断言**）

从真 Pi 与 nini 的**实际屏幕**里提取结构，比较结构。

## 状态：未达标

录屏链路（capture-tui.sh）已经可靠，但**本文件的提取器噪声太大**：
它会把表头边框 `├───┼───┤` 配成键位，会把描述换行后的单元格错位配对，
110 列下 Pi 侧只提到 11 个键（真实约 25 个）。**因此当前输出只能当人工
辅助，不能进 gate。** 真正的修法有两条（下一轮）：

1. 把 COLS 提到 Pi 的键位表不换行的宽度，再收紧列边界；或
2. 绕开屏幕 OCR——Pi 与 nini 都有 `/export`，导出的结构化数据比像素可靠。

先把它写下来，是为了让"录屏可用"与"提取可用"这两件事的边界是清楚的。

## 为什么不是 diff 两个 .txt

两个 TUI 的排版注定不同（版本号、命令数 23+5、渲染风格）。逐字节 diff
得到的是噪声，不是缺陷。但**键位语义**必须一致——`dc75bca` 那个真缺陷
（`/hotkeys` 显示 `Home ?`，因为 `action_label` 漏了新 action 落到
`other => "?"` 臂）就是语义层的事。

所以：从屏幕里抽出 `(键, 描述)`，比语义。像素不一样不算，语义不一样才算。

## 输入

    captures/tui/<scenario>/pi/<step>.txt
    captures/tui/<scenario>/nini/<step>.txt

两边都是 `scripts/capture-tui.sh` 在**同一 mock provider、同一隔离 HOME、
同一终端尺寸**下录的，所以上游与环境的差异被排除掉了。

## 用法

    python3 scripts/tui-oracle.py                 # 全部场景
    python3 scripts/tui-oracle.py 01-hotkeys      # 指定场景
    python3 scripts/tui-oracle.py --json
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CAP = ROOT / "captures" / "tui"

# 键名 token。刻意覆盖两代写法：Pi 写 "Ctrl+O"，有些地方写 "Ctrl+Shift+O"。
KEY = re.compile(
    r"\b(?:Ctrl|Shift|Alt|Esc|Enter|Tab|Backspace|Delete|PageUp|PageDown|"
    r"Left|Right|Up|Down|Home|End|Home|Insert|Space|F\d{1,2})"
    r"(?:\+(?:Ctrl|Shift|Alt))*\+?[A-Za-z0-9]?\b"
)
PLAIN_KEY = re.compile(
    r"\b(?:Home|End|Backspace|Delete|Tab|Enter|Esc|Insert|PageUp|PageDown)\b"
)

# Pi 的表格行： │ Ctrl+O                │ Toggle tool output             │
# 必须是**两列以上**的真表格行：行内至少 3 个 `│`。
# nini 的输入框是 `│ ❯ ... │`（2 个 │），不满足，因此不会被误当成键位表。
PI_ROW = re.compile(r"│\s*([^│]+?)\s*│\s*([^│]+?)\s*│[^│]*│")


def strip_box(line: str) -> str:
    return line.replace("│", " ").replace("┌", " ").replace("┐", " ")
    line = line.replace("├", " ").replace("┤", " ").replace("└", " ")
    return line.replace("┘", " ").replace("─", " ").strip()


def pi_bindings(text: str) -> dict[str, str]:
    out: dict[str, str] = {}
    for m in PI_ROW.finditer(text):
        key, desc = m.group(1), m.group(2)
        key = key.strip()
        if not key or key in {"─", "-"}:
            continue
        # 跳过表头行
        if desc.strip().lower() in {"description", "action", "key"}:
            continue
        out[key] = desc.strip()
    return out


def loose_bindings(text: str) -> dict[str, str]:
    """宽松提取：任何 `键 + 空白 + 描述` 的行内相邻对。

    nini 的 /hotkeys 是流式排版（没有框线），只能这样抓。代价是它会把
    footer 里的 "Enter send" 也算进来——所以下面会过滤。
    """
    flat = " ".join(strip_box(l) for l in text.splitlines())
    out: dict[str, str] = {}
    pat = re.compile(
        r"((?:Ctrl|Shift|Alt|F\d{1,2})(?:\+(?:Ctrl|Shift|Alt))*\+?[A-Za-z0-9]?"
        r"|Home|End|Backspace|Delete|Tab|Enter|Esc|Insert|PageUp|PageDown)\s+"
        r"([a-z][a-z0-9 ,'()/.-]{2,40}?)(?=\s{2,}|\s+(?:Ctrl|Shift|Alt|F\d|Esc|Home|End|Backspace|Delete|Tab|Enter|Insert|Page)|$)"
    )
    for m in pat.finditer(flat):
        out.setdefault(m.group(1).strip(), m.group(2).strip())
    return out


def bare_question_marks(text: str) -> list[str]:
    """`dc75bca` 的通用检测：某个绑定在表里显示成一个裸 `?`。

    出现 `?` 作为表格单元或 `键 ? 描述`，就是 action_label 漏了新 action。
    """
    hits = []
    for line in text.splitlines():
        for m in PI_ROW.finditer(line):
            if m.group(1).strip() == "?":
                hits.append(line.strip())
        if re.search(r"\b\w[\w+]*\s+\?\s+\S", line):
            hits.append(line.strip())
    return hits


def render_forms(text: str) -> set[str]:
    return {
        "boxed-table" if PI_ROW.search(text) else "flowing-text",
    }


def analyse(scenario: str) -> dict:
    pi_dir = CAP / scenario / "pi"
    ni_dir = CAP / scenario / "nini"
    if not pi_dir.exists():
        return {"scenario": scenario, "error": "缺少 pi 侧录屏"}

    results = []
    for pi_file in sorted(pi_dir.glob("*.txt")):
        ni_file = ni_dir / pi_file.name
        text = pi_file.read_text(errors="replace")
        rec: dict = {"step": pi_file.stem}
        rec["pi_render"] = sorted(render_forms(text))
        rec["pi_bare_question"] = bare_question_marks(text)
        pb = pi_bindings(text) if PI_ROW.search(text) else loose_bindings(text)
        rec["pi_bindings"] = len(pb)
        rec["pi_bindings_sample"] = dict(list(pb.items())[:6])

        if ni_file.exists():
            ntext = ni_file.read_text(errors="replace")
            rec["nini_render"] = sorted(render_forms(ntext))
            rec["nini_bare_question"] = bare_question_marks(ntext)
            nb = (
                pi_bindings(ntext) if PI_ROW.search(ntext) else loose_bindings(ntext)
            )
            rec["nini_bindings"] = len(nb)
            only_pi = {k: v for k, v in pb.items() if k not in nb}
            only_ni = {k: v for k, v in nb.items() if k not in pb}
            rec["only_in_pi"] = dict(list(only_pi.items())[:20])
            rec["only_in_nini"] = dict(list(only_ni.items())[:20])
            rec["render_differs"] = rec["pi_render"] != rec["nini_render"]
            rec["missing_count"] = len(only_pi)
        else:
            rec["error"] = "缺少 nini 侧录屏"
        results.append(rec)

    return {"scenario": scenario, "steps": results}


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    as_json = "--json" in sys.argv

    scenarios = args or sorted(
        p.name for p in CAP.iterdir() if p.is_dir()
    ) if CAP.exists() else []

    reports = [analyse(s) for s in scenarios]

    if as_json:
        json.dump(reports, sys.stdout, ensure_ascii=False, indent=2)
        print()
        return 0

    problems = 0
    for rep in reports:
        if rep.get("error"):
            print(f"\n## {rep['scenario']}：{rep['error']}")
            problems += 1
            continue
        print(f"\n## {rep['scenario']}")
        for st in rep["steps"]:
            flag = ""
            if st.get("error"):
                print(f"  {st['step']}: {st['error']}")
                problems += 1
                continue
            if st.get("render_differs"):
                flag += " [渲染形态不同]"
            if st.get("missing_count"):
                flag += f" [{st['missing_count']} 个键位 Pi 有 nini 无]"
            print(
                f"  {st['step']}: pi={st['pi_render'][0]} nini={st.get('nini_render',['?'])[0]}"
                f" 绑定 {st['pi_bindings']} vs {st.get('nini_bindings','?')}{flag}"
            )
            if st.get("pi_bare_question") or st.get("nini_bare_question"):
                print("      裸 `?` 标签（dc75bca 类缺陷）:")
                for h in (st["pi_bare_question"] + st.get("nini_bare_question", [])):
                    print(f"        {h}")
                problems += 1
            if st.get("only_in_pi"):
                print("      Pi 有而 nini 无:")
                for k, v in list(st["only_in_pi"].items())[:12]:
                    print(f"        {k:<18} {v}")
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
