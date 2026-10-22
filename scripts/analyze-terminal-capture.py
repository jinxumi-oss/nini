#!/usr/bin/env python3
"""分析一次**真实终端**的原始输出流，回答"屏幕为什么坏了"。

## 为什么需要它

tmux 的 `capture-pane` 给的是**网格**：终端已经替 nini 折行、跳过宽字符的续格、
处理了样式之后的结果。网格看起来干净，终端里可能已经乱了 —— 折行会把下面每一行
往下推一格，于是整屏错位；没被复位��样式会让后面所有字都带下划线。

所以这个脚本读**原始字节流**（`script -q -c nini /tmp/log` 的产物），并且只
回答三件事：

1. **有没有一行比终端还宽？** 有 → 终端折行 → 下面全部错位（"乱码"）。
2. **流结束时还有没有样式没关？** 有 → 终端把后面的字都画成那个样式
   （"下划线"）。
3. **有没有非法 UTF-8 字节？** 有 → 半个字（"乱码"）。

## 用法

    script -q -c nini /tmp/nini-screen.log      # 在**你的**终端里录一次
    python3 scripts/analyze-terminal-capture.py /tmp/nini-screen.log [cols]

`cols` 省略时按 80 计算（终端宽度用 `tput cols` 查）。
退出码 0 = 没发现问题。
"""
from __future__ import annotations

import re
import sys
import unicodedata

CSI = re.compile(rb"\x1b\[([0-9;?]*)([@-~])")
OSC = re.compile(rb"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")


def display_width(text: str) -> int:
    """一行的显示宽度：宽字符 2 列，歧义字符按 2 列（最坏情况）。"""
    w = 0
    prev_wide = False
    for ch in text:
        if prev_wide and ch == " ":
            prev_wide = False
            continue
        cw = 2 if unicodedata.east_asian_width(ch) in "WF" else 1
        w += cw
        prev_wide = cw == 2
    return w


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    path = sys.argv[1]
    cols = int(sys.argv[2]) if len(sys.argv) > 2 else 80
    raw = open(path, "rb").read()

    problems: list[str] = []

    # 1) 非法 UTF-8
    try:
        raw.decode("utf-8")
    except UnicodeDecodeError as e:
        problems.append(
            f"流里有非法 UTF-8 字节（{e}）—— 半个字符，终端会画成替换字符"
        )

    # 2) 逐行看宽度：ANSI 序列不占列，但要跳过
    text = raw.decode("utf-8", errors="replace")
    text = OSC.sub("", text)
    widest = 0
    over: list[tuple[int, int, str]] = []
    for i, line in enumerate(text.split("\n")):
        plain = CSI.sub("", line)
        plain = plain.replace("\r", "")
        w = display_width(plain)
        widest = max(widest, w)
        if w > cols:
            over.append((i, w, plain[:70]))
    if over:
        problems.append(
            f"{len(over)} 行比终端宽（{cols} 列）→ 终端折行 → 下面每一行都下移一格："
        )
        for i, w, sample in over[:5]:
            problems.append(f"    第 {i} 行宽 {w}：{sample!r}")

    # 3) 流结束时还开着的样式
    active: set[str] = set()
    for m in CSI.finditer(raw):
        params, final = m.group(1), m.group(2)
        if final != b"m":
            continue
        codes = [c for c in params.decode().split(";") if c != ""] or ["0"]
        for c in codes:
            if c == "0":
                active.clear()
            elif c in ("4", "7", "1", "3", "9"):
                active.add(c)
            elif c in ("24", "27", "22", "23", "29"):
                active.discard(c)
    if active:
        names = {"4": "下划线", "7": "反显", "1": "粗体", "3": "斜体", "9": "删除线"}
        problems.append(
            "流结束时还有样式没被复位："
            + "、".join(names.get(a, a) for a in sorted(active))
            + " → 终端会把后面的字都画成那个样式"
        )

    print(f"捕获：{path}")
    print(f"字节数：{len(raw)}  最宽的一行：{widest} 列  终端宽度：{cols} 列")
    if problems:
        print("FAIL")
        for p in problems:
            print("  -", p)
        return 1
    print("PASS：没有超宽的行、没有漏掉的样式复位、没有非法 UTF-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
