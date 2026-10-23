#!/usr/bin/env python3
"""把行号引用重新锚定到目标版本。

为什么需要它
------------
nini 的行为层证据是「Pi 某个文件第 N 行」。实测：0.84.2 → 0.85.1 之间，
树里 90 处引用只有 **3 处**仍然指向同一行。目标统一到 0.85.1 之后，
这些引用必须逐条重锚——否则它们会**静默地指向别的代码**，
而这正是"行号式 oracle"的全部弱点。

本脚本做的是**机械但可核对**的那部分：
  1. 从引用所在的那一行，取出被引用的那行代码的**原文**（在旧版本快照里解析）；
  2. 在目标版本快照里找**内容完全相同**的行；
  3. 找到唯一一处 → 直接给出新行号（可自动改写）；
     找到多处 / 找不到 → 报告出来，交人工判断（不猜）。

用法
----
    python3 scripts/pi-reanchor.py --from references/pi-0.84.2 --to references/pi-0.85.1
    python3 scripts/pi-reanchor.py ... --json
    python3 scripts/pi-reanchor.py ... --apply     # 只改**唯一匹配**的引用
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent



sys.path.insert(0, str(Path(__file__).resolve().parent))
from pi_refs import resolve  # noqa: E402




def norm(s: str) -> str:
    return re.sub(r"\s+", " ", s.strip())


def is_comment(line: str) -> bool:
    t = line.strip()
    return t.startswith("//") or t.startswith("/*") or t.startswith("*") or t.startswith("*/")


def first_code_line(lines: list[str], idx: int) -> int:
    """从 idx 起找第一行**代码**（跳过注释/空行）。

    为什么：引用必须指向**行为所在的那一行**。指向注释行是"看起来对"——
    注释会随代码移动，而行为不会。审查实测过一处：引用写着
    `interactive-mode.js:2491`，那一行是注释，真正的行为在 2495。
    """
    for i in range(idx, min(len(lines), idx + 12)):
        if lines[i].strip() and not is_comment(lines[i]):
            return i + 1
    return idx + 1


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--from", dest="frm", required=True)
    ap.add_argument("--to", dest="to", required=True)
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--apply", action="store_true", help="自动改写唯一匹配的引用")
    args = ap.parse_args()

    a = (REPO / args.frm if not Path(args.frm).is_absolute() else Path(args.frm))
    b = (REPO / args.to if not Path(args.to).is_absolute() else Path(args.to))

    r = subprocess.run([sys.executable, "scripts/pi-line-refs.py", "--json"],
                       cwd=REPO, capture_output=True, text=True)
    refs = json.loads(r.stdout).get("refs", [])

    # 目标快照里每个文档的行索引
    target_lines: dict[str, list[str]] = {}
    target_paths: dict[str, Path] = {}
    for doc in {x["doc"] for x in refs}:
        p = resolve(b, doc)
        if p:
            target_paths[doc] = p
            target_lines[doc] = p.read_text(encoding="utf-8", errors="replace").splitlines()

    def window_match(doc: str, old_file_lines: list[str], idx: int, radius: int = 3) -> list[int]:
        """用**上下文窗口**消歧：同一行文本出现多次时，用它前后几行来定位。"""
        if doc not in target_lines:
            return []
        lo, hi = max(0, idx - radius), min(len(old_file_lines), idx + radius + 1)
        want = [norm(x) for x in old_file_lines[lo:hi]]
        hits = []
        tgt = target_lines[doc]
        for start in range(0, max(0, len(tgt) - len(want) + 1)):
            if [norm(x) for x in tgt[start:start + len(want)]] == want:
                hits.append(start + (idx - lo) + 1)
        return hits

    out = []
    for ref in refs:
        doc, ln = ref["doc"], ref["doc_line"]
        src = resolve(a, doc)
        old_text = None
        old_lines: list[str] = []
        if src:
            old_lines = src.read_text(encoding="utf-8", errors="replace").splitlines()
            if 1 <= ln <= len(old_lines):
                old_text = old_lines[ln - 1].strip()
        cands = []
        how = ""
        if old_text and doc in target_lines:
            n = norm(old_text)
            for i, line in enumerate(target_lines[doc], start=1):
                if norm(line) == n:
                    cands.append(i)
            if len(cands) > 1:
                # 同一行文本出现多次 → 用上下文窗口消歧
                w = window_match(doc, old_lines, ln - 1)
                if w:
                    cands = w
                    how = "window"
        if not cands:
            # 该行在目标版本里不存在：可能是**引用写错了扩展名**（.js ↔ .ts）。
            # npm 包不发 .ts，作者当年对着还原出的 TS 抄行号 —— 于是引用写着
            # .js，行号却是 .ts 的。这种错位必须被**纠正**，不是被容忍。
            sib = doc[:-3] + ".ts" if doc.endswith(".js") else doc[:-3] + ".js"
            if sib in target_lines and old_text:
                n = norm(old_text)
                for i, line in enumerate(target_lines[sib], start=1):
                    if norm(line) == n:
                        cands = [i]
                        how = f"sibling:{sib}"
                        break
                if cands:
                    out.append({
                        "file": ref["file"], "line": ref["line"], "doc": doc,
                        "old_line": ln, "old_text": old_text,
                        "new_candidates": cands, "status": "unique",
                        "new_doc": sib, "how": how,
                    })
                    continue
        status = ("unique" if len(cands) == 1 else
                  "ambiguous" if len(cands) > 1 else
                  "not-found")
        # 锚点落在注释行 → 落到下一个代码行
        if len(cands) == 1 and doc in target_lines:
            tl = target_lines[doc]
            c = cands[0]
            if 1 <= c <= len(tl) and is_comment(tl[c - 1]):
                nc = first_code_line(tl, c - 1)
                if nc != c:
                    how = (how + "," if how else "") + "comment->code"
                    cands = [nc]
        out.append({
            "file": ref["file"], "line": ref["line"], "doc": doc,
            "old_line": ln, "old_text": old_text,
            "new_candidates": cands, "status": status, "how": how,
        })

    if args.apply:
        # 只改唯一匹配且行号不同的；逐文件重写
        by_file: dict[str, list[dict]] = {}
        for o in out:
            if o["status"] == "unique" and o["new_candidates"][0] != o["old_line"]:
                by_file.setdefault(o["file"], []).append(o)
        changed_files = 0
        for path, items in by_file.items():
            p = REPO / path
            text = p.read_text(encoding="utf-8")
            lines = text.splitlines(keepends=True)
            # 从后往前改，避免行号位移
            for it in sorted(items, key=lambda x: x["line"], reverse=True):
                idx = it["line"] - 1
                if idx >= len(lines):
                    continue
                old_ref = f"{it['doc']}:{it['old_line']}"
                new_doc = it.get("new_doc") or it["doc"]
                new_ref = f"{new_doc}:{it['new_candidates'][0]}"
                if old_ref not in lines[idx]:
                    continue
                lines[idx] = lines[idx].replace(old_ref, new_ref)
            p.write_text("".join(lines), encoding="utf-8")
            changed_files += 1
        print(f"自动改写 {changed_files} 个文件")

    if args.json:
        print(json.dumps(out, ensure_ascii=False, indent=2))
    else:
        c = {}
        for o in out:
            c[o["status"]] = c.get(o["status"], 0) + 1
        print(f"状态分布：{c}")
        for o in out:
            if o["status"] != "unique":
                print(f"  ? {o['file']}:{o['line']}  {o['doc']}:{o['old_line']}  "
                      f"[{o['status']}]  {str(o['old_text'])[:70]}")
        print("\n（--apply 已改写唯一匹配的行）" if args.apply else "")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
