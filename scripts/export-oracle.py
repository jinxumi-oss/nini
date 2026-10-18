#!/usr/bin/env python3
"""导出物对照 oracle：比**内容保全**与**结构保留**，不比像素。

## 为什么不比字节

Pi 279 KB、nini 815 字节——差 342 倍。字节 diff 全是噪声：Pi 内嵌
marked + highlight.js，nini 是内联 minimal。

但体积差不是结论，是**症状**。挖开看，真相是架构级的：

* Pi 的 `/export` 是**自包含单文件 HTML**：`entries` 以 base64(JSON) 内嵌在
  `<script>` 里，由客户端 marked + highlight.js 渲染。**结构化数据完整保留。**
* nini 的 `/export` 是 `render_transcript_html(&[TranscriptLine])`
  （`crates/nini-tui/src/commands/html_export.rs`）——**已经把结构渲染没了，
  只留极简 HTML**。

所以对照的正确形态是：解出 Pi 的结构化 entries，与 nini 的扁平 HTML 比
**内容保全**与**结构保留**，不比像素。

## 剧本里应该出现什么（`fixtures/tui/02-conversation.json`，已知）

| 内容 | 类别 |
|---|---|
| `run the probe` | 用户消息 |
| `Let me think about the export path.` | **thinking** |
| `echo nini-export-probe` | tool 参数 |
| `nini-export-probe` | tool 结果 |
| `The command printed nini-export-probe.` | assistant 文本 |
| `and again` | 用户消息（第二轮） |

任何一条在导出物里找不到，就是一条用户可见的数据丢失。

## 附带发现（已实跑确认）

`nini --export <path>` **静默丢弃路径参数**：`main.rs` 里是 `export: _`，
`dispatch.rs` 里 `cmd_export(_args)`。实测 `nini --export /tmp/x.html -p hello`
退出码 0，而 `/tmp/x.html` 不存在，文件落到了 `~/.nini/agent/exports/`。
这同时是 `--help` 与实现不符（T4）与 headless 导出不可用。

## 用法

    python3 scripts/export-oracle.py [--scenario 03-export] [--json]
"""

from __future__ import annotations

import argparse
import base64
import html
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CAP = ROOT / "captures" / "tui"

# mock 剧本里出现过、因此**应当**出现在导出物里的内容。
# 顺序即对话顺序，重复内容按出现次数计。
EXPECTED: list[tuple[str, str]] = [
    ("run the probe", "user"),
    ("Let me think about the export path.", "thinking"),
    ("echo nini-export-probe", "tool-args"),
    ("nini-export-probe", "tool-result"),
    ("The command printed nini-export-probe.", "assistant"),
    ("and again", "user"),
]

TAG = re.compile(r"<[^>]+>")
B64_SCRIPT = re.compile(r"<script[^>]*>\s*([A-Za-z0-9+/=]{200,})\s*</script>")


def decode_structured(raw: str) -> dict | None:
    """Pi 的导出把 session 数据以 base64(JSON) 内嵌在 `<script>` 里。

    这就是"绕开像素"的正确入口：它给的是结构化 entries，不是画面。
    """
    for blob in B64_SCRIPT.findall(raw):
        try:
            data = json.loads(base64.b64decode(blob))
        except Exception:
            continue
        if isinstance(data, dict) and "entries" in data:
            return data
    return None


def text_of(raw: str) -> str:
    """去标签 + 解实体 + 压空白，供内容查找。"""
    stripped = TAG.sub(" ", raw)
    return re.sub(r"\s+", " ", html.unescape(stripped)).strip()


def probe(html_path: Path) -> dict:
    raw = html_path.read_text(errors="replace")
    structured = decode_structured(raw)
    visible = text_of(raw)

    # 内容查找的草料：结构化优先（能看见被 base64 掉的内容），
    # 实在没有结构就退到可见文本。
    haystack = json.dumps(structured, ensure_ascii=False) if structured else visible

    present = {label: (needle in haystack) for needle, label in EXPECTED}

    blocks = []
    if structured:
        for e in structured.get("entries", []):
            blocks.append(e.get("type"))
            msg = e.get("message") or {}
            for part in msg.get("content", []) or []:
                if isinstance(part, dict) and part.get("type"):
                    blocks.append(part["type"])

    return {
        "file": html_path.name,
        "bytes": html_path.stat().st_size,
        "has_structured_entries": structured is not None,
        "entry_count": len(structured.get("entries", [])) if structured else 0,
        "block_types": sorted({b for b in blocks if b}),
        "present": present,
        "missing": [lab for lab, ok in present.items() if not ok],
        "classes": sorted(set(re.findall(r'class="([^"]+)"', raw)))[:24],
        "has_code_block": "<pre" in raw or "<code" in raw,
        "has_syntax_spans": bool(
            re.findall(r'class="[^"]*(?:hljs|token|keyword)[^"]*"', raw)
        ),
        "has_inline_css": "<style" in raw,
        "has_external_css": bool(re.findall(r"<link[^>]+stylesheet", raw)),
        "script_tags": len(re.findall(r"<script", raw)),
        "elided_markers": len(re.findall(r"elided|omitted", raw)),
    }


GOLDEN = ROOT / "fixtures" / "tui" / "golden"


def find_export(scenario: str, side: str) -> Path | None:
    """优先读入库的结构化 golden，其次读本机抓到的 HTML。"""
    g = GOLDEN / f"{scenario}.{side}.json"
    if g.exists():
        return g
    d = CAP / scenario / side / "exports"
    files = sorted(d.glob("*.html")) if d.exists() else []
    return files[0] if files else None


def save_golden(scenario: str) -> None:
    """把 Pi 导出里解出的结构化 entries 存成 golden。

    不存那 279 KB HTML——它九成是 Pi 自带的 marked + highlight.js，
    对照用不上，而且每跑一次时间戳和 id 都不同，存进来只会制造噪声 diff。
    真正需要固定下来的是**结构与内容**。
    """
    GOLDEN.mkdir(parents=True, exist_ok=True)
    src = CAP / scenario / "pi" / "exports"
    files = sorted(src.glob("*.html")) if src.exists() else []
    for f in files:
        structured = decode_structured(f.read_text(errors="replace"))
        if structured:
            out = GOLDEN / f"{scenario}.pi.json"
            out.write_text(
                json.dumps(structured, ensure_ascii=False, indent=2), encoding="utf-8"
            )
            print(f"golden 已存 {out} ({out.stat().st_size} 字节, "
                  f"{len(structured.get('entries', []))} entries)")
            return
    print("未找到可解码的 Pi 导出；先跑 capture-tui.sh --bin pi")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--scenario", default="03-export")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--save-golden", action="store_true")
    args = ap.parse_args()

    if args.save_golden:
        save_golden(args.scenario)
        return 0

    base = CAP / args.scenario
    sides = {}
    for side in ("pi", "nini"):
        f = find_export(args.scenario, side)
        if not f:
            sides[side] = None
            continue
        if f.suffix == ".json":
            structured = json.loads(f.read_text(errors="replace"))
            hay = json.dumps(structured, ensure_ascii=False)
            sides[side] = {
                "file": f.name,
                "bytes": f.stat().st_size,
                "has_structured_entries": True,
                "entry_count": len(structured.get("entries", [])),
                "block_types": sorted({
                    p.get("type")
                    for e in structured.get("entries", [])
                    for p in ((e.get("message") or {}).get("content") or [])
                    if isinstance(p, dict) and p.get("type")
                } | {e.get("type") for e in structured.get("entries", []) if e.get("type")}),
                "present": {lab: (needle in hay) for needle, lab in EXPECTED},
                "missing": [lab for lab, ok in {
                    lab: (needle in hay) for needle, lab in EXPECTED}.items() if not ok],
                "classes": [], "has_code_block": None, "has_syntax_spans": None,
                "has_inline_css": None, "has_external_css": None,
                "script_tags": None, "elided_markers": None,
            }
        else:
            sides[side] = probe(f)

    if args.json:
        json.dump(sides, sys.stdout, ensure_ascii=False, indent=2)
        print()
        return 0

    print(f"# 导出物内容保全对照（{args.scenario}）\n")
    print(f"剧本里发生过 {len(EXPECTED)} 件事。导出物里还在几件：\n")

    header = f"| {'内容':<10} | Pi 0.84.2 | nini |"
    print(header)
    print("|---|---|---|")
    missing_total = 0
    for needle, label in EXPECTED:
        p = sides["pi"]["present"].get(label) if sides["pi"] else None
        n = sides["nini"]["present"].get(label) if sides["nini"] else None
        if n is False:
            missing_total += 1
        print(f"| {label:<10} | {'在' if p else '**丢**'} | {'在' if n else '**丢**'} |")

    print()
    for side in ("pi", "nini"):
        s = sides.get(side)
        if not s:
            print(f"## {side}：无导出物")
            continue
        print(f"## {side}  {s['bytes']:>7} 字节  {s['file']}")
        print(f"  丢失内容: {s['missing'] or '无'}")
        print(
            f"  结构化 entries: {s['entry_count'] if s['has_structured_entries'] else '**无（已渲染成 HTML）**'}"
        )
        print(f"  块类型: {', '.join(s['block_types']) or '—'}")
        print(f"  代码块: {s['has_code_block']}   语法着色: {s['has_syntax_spans']}")
        print(f"  内联 CSS: {s['has_inline_css']}   外链 CSS: {s['has_external_css']}")
        print(f"  <script> 标签: {s['script_tags']}   省略标记: {s['elided_markers']}")
        print(f"  class: {', '.join(s['classes'][:12])}")
        print()

    return 1 if missing_total else 0


if __name__ == "__main__":
    raise SystemExit(main())
