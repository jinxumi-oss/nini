#!/usr/bin/env python3
"""Pi 引用（citation）的公共解析层：别名表 + 快照定位。

为什么单独抽出来
----------------
`pi-line-refs.py`、`pi-oracle-lock.py`、`pi-reanchor.py`、`pi-version-delta.py`
都需要"把一个被引用的文档名解析到快照里的具体文件"。如果每个脚本各抄一份
别名表，就会出现**同一份失真有三份副本**——这正是 nini 自己的 TUI 键位表
踩过的坑（footer / `/hotkeys` / README 三份不一致），教训刚写进 CHANGELOG。
所以别名表只有这一份。

引用形态
--------
    agent-loop.js:160                  行号引用（脆：Pi 一升级就漂）
    agent-loop.js#shouldStopAfterTurn  符号引用（稳：含义被钉住，行号由脚本解析）

行号引用在 0.84.2 → 0.85.1 之间实测：90 处里只有 3 处还指向同一行。
"""

from __future__ import annotations

from pathlib import Path

# 被引用的文档名 → 快照里的相对路径（按优先级）。
#
# 分组名来自 scripts/pi-snapshot.py 的 WANTED：
#   loop / session / ui / tui / tools-t1 / tools-t2 / ext / provider / api / providers
#
# 注意 "keybindings.ts"：0.84.2 时它在 pi-tui 里（`tui.altScreen.top`），
# 0.85.1 的 coding-agent 也有一个同名文件（app 级键位）。两个都叫
# keybindings.ts——**同名不同物**，所以别名表必须写清优先级，
# 否则解析到哪个全看运气。
DOC_ALIASES = {
    "agent-loop.js": ["loop/agent-loop.js"],
    "agent-loop.ts": ["loop/agent-loop.ts"],
    "agent-session.js": ["session/agent-session.js"],
    "agent-session.ts": ["session/agent-session.ts"],
    "interactive-mode.js": ["ui/interactive-mode.js"],
    "interactive-mode.ts": ["ui/interactive-mode.ts"],
    "keybindings.ts": ["tui/keybindings.ts", "ui/keybindings.ts"],
    "keybindings.js": ["tui/keybindings.js", "ui/keybindings.js"],
    "tui-alt-screen.ts": ["tui/tui-alt-screen.ts"],
    "tui-alt-screen.js": ["tui/tui-alt-screen.js"],
}


def snapshot_dirs(repo_root: Path) -> dict[str, Path]:
    """返回 {版本号: 快照目录}，按 references/pi-* 目录名。"""
    refs = repo_root / "references"
    out = {}
    if refs.is_dir():
        for d in sorted(refs.glob("pi-*")):
            if (d / "SNAPSHOT.lock").is_file():
                out[d.name.replace("pi-", "")] = d
    return out


def resolve(snapshot: Path, doc: str) -> Path | None:
    """把被引用的文档名解析到快照里的文件（找不到返回 None）。"""
    for cand in DOC_ALIASES.get(doc, []):
        for sub in ("dist", "src"):
            p = snapshot / sub / cand
            if p.is_file():
                return p
    # 兜底：按文件名在快照里找（同名多命中时取排序第一个，并让调用方知道）
    for sub in ("dist", "src"):
        hits = sorted((snapshot / sub).rglob(doc))
        if hits:
            return hits[0]
    return None


def resolve_all(snapshot: Path, doc: str) -> list[Path]:
    """同名文档的全部候选（用于提示"同名不同物"）。"""
    out = []
    for cand in DOC_ALIASES.get(doc, []):
        for sub in ("dist", "src"):
            p = snapshot / sub / cand
            if p.is_file():
                out.append(p)
    if not out:
        for sub in ("dist", "src"):
            out.extend(sorted((snapshot / sub).rglob(doc)))
    return out
