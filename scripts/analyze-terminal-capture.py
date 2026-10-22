#!/usr/bin/env python3
"""分析一次**真实终端**的原始输出流：把流重放成屏幕，然后回答"屏幕为什么坏了"。

## 为什么不能用 tmux 的 capture-pane

`capture-pane` 给的是**网格**：终端已经替应用折了行、跳过了宽字符的续格、
处理了样式之后的结果。网格可以看起来干净，而终端里已经乱了。

## 这个脚本做什么

把原始字节流按终端的规则重放（光标定位、清屏、行/列擦除、SGR 样式、
可打印文本），得到一张屏幕，并检查三件事：

1. **应用写到了终端宽度之外**（列 > cols）→ 终端折行 → 之后每一行都下移
   一格 → 整屏错位。
2. **宽字符落在最后一列** → 终端在这个字符之前就折行 → 那一行错位。
3. **流结束时还有样式没被复位**（下划线/反显/粗体）→ 终端把后面所有字都
   画成那个样式 → 满屏"下划线"。

外加：非法 UTF-8 字节（半个字 → 替换字符）。

## 用法

    script -q -c nini /tmp/nini-screen.log           # 在**你的**终端里录一次
    python3 scripts/analyze-terminal-capture.py /tmp/nini-screen.log [cols] [rows]

`cols`/`rows` 省略时按 80x24。退出码 0 = 没发现问题。
"""
from __future__ import annotations

import re
import sys
import unicodedata

CSI = re.compile(r"\x1b\[([0-9;?]*)([@-~])")
OSC = re.compile(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")

STYLE_NAMES = {
    "1": "粗体",
    "3": "斜体",
    "4": "下划线",
    "7": "反显",
    "9": "删除线",
    "5": "闪烁",
}


def char_width(ch: str) -> int:
    if unicodedata.combining(ch):
        return 0
    return 2 if unicodedata.east_asian_width(ch) in "WF" else 1


class Replay:
    """把一个终端流重放成屏幕。"""

    def __init__(self, cols: int, rows: int) -> None:
        self.cols = cols
        self.rows = rows
        self.grid = [[" "] * cols for _ in range(rows)]
        self.styles = [[""] * cols for _ in range(rows)]
        self.row = 0
        self.col = 0
        self.style: list[str] = []
        self.problems: list[str] = []
        self.beyond_width = 0
        self.wide_at_edge = 0
        self.written = 0

    # -- 样式 -------------------------------------------------------------
    def _sgr(self, params: str) -> None:
        codes = [c for c in params.split(";") if c != ""] or ["0"]
        for c in codes:
            if c == "0":
                self.style.clear()
            elif c in STYLE_NAMES:
                if c not in self.style:
                    self.style.append(c)
            elif c in ("22", "23", "24", "27", "29"):
                for name in STYLE_NAMES:
                    if name == {"22": "1", "23": "3", "24": "4", "27": "7", "29": "9"}.get(c, ""):
                        if name in self.style:
                            self.style.remove(name)

    def _style_of(self) -> str:
        return ",".join(sorted(self.style))

    # -- 写入 -------------------------------------------------------------
    def _put(self, ch: str) -> None:
        w = char_width(ch)
        if w == 0:
            return
        if self.col + w > self.cols:
            # 放不下了：终端会折行，整屏从此错位。
            if w == 2:
                # 宽字符在最后一列只放得下半格 —— 终端在它**之前**就折行，
                # 这个字落在下一行的开头。
                self.wide_at_edge += 1
                if self.wide_at_edge == 1:
                    self.problems.append(
                        f"宽字符落在最后一列（列 {self.col}，宽 {w}）：终端在它之前折行，"
                        "那一行之后全部错位"
                    )
            else:
                self.beyond_width += 1
                if self.beyond_width == 1:
                    self.problems.append(
                        f"内容比终端宽（写到列 {self.col}，终端只有 {self.cols} 列）："
                        "终端折行 → 之后每一行下移一格"
                    )
            self.row = min(self.rows - 1, self.row + 1)
            self.col = 0
        if self.col >= self.cols:
            self.beyond_width += 1
            if self.beyond_width == 1:
                self.problems.append(
                    f"应用把内容写到了终端宽度之外：列 {self.col} > {self.cols} → "
                    "终端折行 → 之后每一行下移一格"
                )
            return
        if 0 <= self.row < self.rows:
            self.grid[self.row][self.col] = ch
            self.styles[self.row][self.col] = self._style_of()
            if w == 2 and self.col + 1 < self.cols:
                self.grid[self.row][self.col + 1] = ""
                self.styles[self.row][self.col + 1] = self._style_of()
        self.written += 1
        self.col += w

    # -- 主循环 -----------------------------------------------------------
    def feed(self, text: str) -> None:
        i = 0
        n = len(text)
        while i < n:
            ch = text[i]
            if ch == "\x1b":
                m = CSI.match(text, i)
                if m:
                    params, final = m.group(1), m.group(2)
                    if final == "H":
                        parts = [p for p in params.split(";") if p != ""] or ["1", "1"]
                        r = int(parts[0]) if parts[0].isdigit() else 1
                        c = int(parts[1]) if len(parts) > 1 and parts[1].isdigit() else 1
                        self.row, self.col = max(0, r - 1), max(0, c - 1)
                    elif final == "K":
                        for x in range(self.col, self.cols):
                            if 0 <= self.row < self.rows:
                                self.grid[self.row][x] = " "
                                self.styles[self.row][x] = ""
                    elif final == "J":
                        self.grid = [[" "] * self.cols for _ in range(self.rows)]
                        self.styles = [[""] * self.cols for _ in range(self.rows)]
                    elif final == "m":
                        self._sgr(params)
                    i = m.end()
                    continue
                m = OSC.match(text, i)
                if m:
                    i = m.end()
                    continue
                i += 1
                continue
            if ch == "\n":
                self.row = min(self.rows - 1, self.row + 1)
                self.col = 0
            elif ch == "\r":
                self.col = 0
            elif ch == "\b":
                self.col = max(0, self.col - 1)
            elif ch == "\t":
                self.col = min(self.cols - 1, (self.col // 8 + 1) * 8)
            elif ord(ch) < 32:
                pass
            else:
                self._put(ch)
            i += 1

    def screen(self) -> list[str]:
        return ["".join(row).rstrip() for row in self.grid]


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    path = sys.argv[1]
    cols = int(sys.argv[2]) if len(sys.argv) > 2 else 80
    rows = int(sys.argv[3]) if len(sys.argv) > 3 else 24
    raw = open(path, "rb").read()

    problems: list[str] = []
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as e:
        problems.append(f"流里有非法 UTF-8 字节（{e}）—— 半个字符，终端会画成替换字符")
        text = raw.decode("utf-8", errors="replace")

    r = Replay(cols, rows)
    r.feed(text)

    for p in r.problems:
        problems.append(p)

    # 流结束时的样式：终端会把后面的字都画成那个样式
    if r.style:
        problems.append(
            "流结束时还有样式没被复位："
            + "、".join(STYLE_NAMES.get(a, a) for a in r.style)
            + " → 终端会把后面所有字都画成那个样式"
        )

    print(f"捕获：{path}")
    print(
        f"字节数：{len(raw)}  终端：{cols}x{rows}  写入字符数：{r.written}  "
        f"写到宽度外：{r.beyond_width}  宽字符顶到边上：{r.wide_at_edge}"
    )
    if problems:
        print("FAIL")
        for p in problems:
            print("  -", p)
        print("--- 屏幕（重放后） ---")
        for line in r.screen():
            print(f"  |{line}")
        return 1
    print("PASS：没有写到宽度之外的内容、没有漏掉的样式复位、没有非法 UTF-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
