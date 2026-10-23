#!/usr/bin/env python3
"""把一个 Pi 版本的**机制面源码**快照进仓库，并加 sha256 锁。

为什么要有这个
--------------
nini 复刻 Pi 的行为层，全部证据都是「Pi 的某个文件第 N 行」。而 Pi 会升级：
行号会漂，漂了没有任何东西会红。v0.10.8 的审查实测：树里 90 处行号引用里
已经有一处越界（`agent-loop.js:582` 实际是 `.ts` 的行号）。

所以：**对照物必须进仓库、带哈希、被脚本检查**。这不是"复制别人的代码"，
是把"我们对照的是哪一个版本的哪一行"变成可复算的事实。

快照三层内容
------------
1. ``dist/``  —— npm 包里实际发布的 .js（运行的真相）
2. ``src/``   —— 从 ``.js.map`` 的 ``sourcesContent`` 逐字还原的 .ts（可读的真相）
3. ``maps/``  —— 原始 .js.map（还原过程的证据，可复算）

用法
----
    python3 scripts/pi-snapshot.py --version 0.85.1 \\
        --root /tmp/pi-0.85.1/node_modules/@earendil-works/pi-coding-agent \\
        --out references/pi-0.85.1

    python3 scripts/pi-snapshot.py --version 0.85.1 --root ... --out ... --check
        # 只校验哈希，不写盘
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# 要进快照的机制面文件。分组对应 nini 的复刻分层：
#   loop     —— agent loop（纯循环）
#   session  —— 会话状态
#   ui       —— 交互层
#   tools-t1 —— 基本工具（harness 层：read/write/edit/bash）
#   tools-t2 —— coding 工具（grep/find/ls + 截断 + 变更队列）
#   ext      —— 扩展工具契约
#   provider —— provider 目录与协议层
WANTED = {
    "loop": [
        "node_modules/@earendil-works/pi-agent-core/dist/agent-loop.js",
    ],
    "session": [
        "dist/core/agent-session.js",
    ],
    "ui": [
        "dist/modes/interactive/interactive-mode.js",
        "dist/core/keybindings.js",
    ],
    # pi-tui 是**独立的包**，0.85.1 把相当一部分机制沉到了这里：
    # search-match 的样式（0.84.2 时还在 coding-agent 的 interactive-mode 里）
    # 以及 alt-screen 的键位。nini 用 ratatui 自己重写了这一层，
    # 所以 nini-tui 是 pi-tui 的 3.2 倍——那不是"nini 更细"，那是"没有这一层"。
    "tui": [
        "node_modules/@earendil-works/pi-tui/dist/tui-alt-screen.js",
        "node_modules/@earendil-works/pi-tui/dist/keybindings.js",
    ],
    "tools-t1": [
        "node_modules/@earendil-works/pi-agent-core/dist/harness/tools/bash.js",
        "node_modules/@earendil-works/pi-agent-core/dist/harness/tools/read.js",
        "node_modules/@earendil-works/pi-agent-core/dist/harness/tools/write.js",
        "node_modules/@earendil-works/pi-agent-core/dist/harness/tools/edit.js",
        "node_modules/@earendil-works/pi-agent-core/dist/harness/tools/index.js",
    ],
    "tools-t2": [
        "dist/core/tools/index.js",
        "dist/core/tools/grep.js",
        "dist/core/tools/find.js",
        "dist/core/tools/ls.js",
        "dist/core/tools/truncate.js",
        "dist/core/tools/file-mutation-queue.js",
        "dist/core/tools/output-accumulator.js",
    ],
    "ext": [
        "dist/core/extensions/types.d.ts",
        "dist/core/sdk.d.ts",
    ],
    "provider": [
        "node_modules/@earendil-works/pi-ai/dist/providers/all.js",
    ],
}

# 通配的目录（协议 dialect 层、provider 目录层）。这两个是 nini 欠配最狠的
# 两层（pi-ai 0.21x），快照里必须有它们，"差分"才有意义。
GLOBS = {
    "api": [
        "node_modules/@earendil-works/pi-ai/dist/api/*.js",
    ],
    "providers": [
        "node_modules/@earendil-works/pi-ai/dist/providers/*.js",
    ],
}


# 按**内容标记**发现文件，而不是按固定路径。
#
# 为什么要这样：0.84.2 → 0.85.1 之间 Pi 把 `pi-ai/dist/providers/` **扁平化**了
# （provider 文件挪到 dist 顶层，providers/ 只剩 images/ 与 data/）。
# 固定路径的快照脚本在版本间会静默地少抓文件——那正是"对照物悄悄变了"。
DISCOVER = {
    # provider 注册表：里面有全部内置 provider 的构造
    "provider": ["builtinProviders"],
    # 工具注册表：内置工具名集合
    "tools-t2": ["allToolNames"],
    # 默认 system prompt：工具集 → guideline 的联动就在这里
    "system-prompt": ["Available tools:"],
}

# 这些文件在包里可能不存在（版本间会挪位置），缺失不致命，但要在报告里说。
# 关键文件（loop / tools）缺失才致命。
CRITICAL_GROUPS = {"loop", "tools-t1", "tools-t2"}


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    h.update(path.read_bytes())
    return h.hexdigest()


def restore_ts_from_map(map_path: Path) -> tuple[str | None, str | None]:
    """从 .js.map 的 sourcesContent 里还原 .ts 源码。"""
    try:
        data = json.loads(map_path.read_text(encoding="utf-8", errors="replace"))
    except Exception:
        return None, None
    sources = data.get("sources") or []
    contents = data.get("sourcesContent") or []
    for src, content in zip(sources, contents):
        if content is None:
            continue
        if src.endswith(".ts") and not src.endswith(".d.ts"):
            name = Path(src).name
            return name, content
    return None, None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True)
    ap.add_argument("--root", required=True, help="pi-coding-agent 包的根目录")
    ap.add_argument("--out", required=True)
    ap.add_argument("--check", action="store_true", help="只校验，不写盘")
    args = ap.parse_args()

    root = Path(args.root).resolve()
    out = (REPO / args.out) if not Path(args.out).is_absolute() else Path(args.out)
    lock_path = out / "SNAPSHOT.lock"

    # --- 收集源文件 -------------------------------------------------------
    found: dict[str, Path] = {}
    missing: list[str] = []
    for group, rels in WANTED.items():
        for rel in rels:
            p = root / rel
            if p.is_file():
                found[rel] = p
            else:
                missing.append(f"{group}:{rel}")
    for group, pats in GLOBS.items():
        matched = False
        for pat in pats:
            for p in sorted(root.glob(pat)):
                if p.is_file():
                    found[str(p.relative_to(root))] = p
                    matched = True
        if not matched:
            missing.append(f"{group}:{'/'.join(pats)}")

    # 按内容标记发现（跨版本稳定）
    scan_roots = [root] + sorted((root / "node_modules" / "@earendil-works").glob("*")) \
        + sorted((root / "node_modules" / "@earendil-works").glob("*/node_modules/@earendil-works/*"))
    for group, markers in DISCOVER.items():
        for sr in scan_roots:
            if not sr.is_dir():
                continue
            for p in sorted(sr.rglob("*.js")):
                rel = str(p.relative_to(root))
                if rel in found or "node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-coding-agent" in rel:
                    continue
                try:
                    head = p.read_text(encoding="utf-8", errors="ignore")[:20000]
                except Exception:
                    continue
                if any(m in head for m in markers):
                    found[rel] = p
                    break
    missing_critical = [m for m in missing if m.split(":")[0] in CRITICAL_GROUPS]

    if missing_critical:
        print(f"关键文件缺失，不能快照：\n  " + "\n  ".join(missing_critical), file=sys.stderr)
        return 2

    if args.check:
        if not lock_path.is_file():
            print(f"没有 {lock_path}", file=sys.stderr)
            return 2
        lock = json.loads(lock_path.read_text(encoding="utf-8"))
        bad = []
        for rel, meta in lock["files"].items():
            p = out / rel
            if not p.is_file():
                bad.append(f"{rel}: 缺失")
            elif sha256(p) != meta["sha256"]:
                bad.append(f"{rel}: 哈希变了")
        if bad:
            print("快照校验失败：\n  " + "\n  ".join(bad), file=sys.stderr)
            return 1
        print(f"快照校验通过：{len(lock['files'])} 个文件")
        return 0

    # --- 写盘 -------------------------------------------------------------
    if out.exists():
        shutil.rmtree(out)
    (out / "dist").mkdir(parents=True)
    (out / "src").mkdir(parents=True)
    (out / "maps").mkdir(parents=True)

    files_meta: dict[str, dict] = {}
    ts_sources: dict[str, str] = {}

    group_of = {}
    for group, rels in WANTED.items():
        for rel in rels:
            group_of[rel] = group
    for group in GLOBS:
        for rel in list(found):
            if rel not in group_of:
                group_of[rel] = group

    for rel, src in sorted(found.items()):
        name = Path(rel).name
        # 组名进路径：harness 的 bash.js 与 core 的 bash.js 是两个不同的工具集，
        # 放在同一个目录里会互相覆盖——**快照被覆盖是最难发现的一类失真**。
        group = group_of[rel]
        dist = out / "dist" / group / name
        dist.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dist)
        files_meta[f"dist/{group}/{name}"] = {
            "sha256": sha256(dist),
            "bytes": dist.stat().st_size,
            "origin": rel,
            "group": group,
        }

        map_src = src.with_suffix(src.suffix + ".map")
        if map_src.is_file():
            maps = out / "maps" / group / map_src.name
            maps.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(map_src, maps)
            files_meta[f"maps/{group}/{map_src.name}"] = {
                "sha256": sha256(maps),
                "bytes": maps.stat().st_size,
                "origin": rel + ".map",
                "group": group,
            }
            ts_name, ts_content = restore_ts_from_map(map_src)
            if ts_name:
                ts_path = out / "src" / group / ts_name
                ts_path.parent.mkdir(parents=True, exist_ok=True)
                ts_path.write_text(ts_content, encoding="utf-8")
                files_meta[f"src/{group}/{ts_name}"] = {
                    "sha256": sha256(ts_path),
                    "bytes": ts_path.stat().st_size,
                    "origin": rel + ".map:sourceContent",
                    "group": group,
                }
                ts_sources[f"{group}/{name}"] = f"{group}/{ts_name}"

    lock = {
        "piVersion": args.version,
        "sourceRoot": str(root),
        "captured": "2026-10-23",
        "capturedBy": "scripts/pi-snapshot.py",
        "note": "dist/ = npm 实际发布物；src/ = 由 .js.map 的 sourcesContent 逐字还原；"
                "maps/ = 还原证据。三层都可复算。",
        "missing": missing,
        "files": files_meta,
    }
    lock_path.write_text(json.dumps(lock, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    (out / "README.md").write_text(
        f"""# Pi {args.version} 机制面快照

- 来源：`{root}`（npm 包 `@earendil-works/pi-coding-agent@{args.version}`）
- 生成：`python3 scripts/pi-snapshot.py --version {args.version} --root {root} --out {args.out}`
- 校验：`python3 scripts/pi-snapshot.py --version {args.version} --root {root} --out {args.out} --check`

`dist/` 是包里实际发布的 .js；`src/` 是从 `.js.map` 的 `sourcesContent` 逐字还原的 .ts；
`maps/` 是还原过程的证据。三个目录都在 `SNAPSHOT.lock` 的 sha256 锁里。

**这份快照是 nini 行为层对照物的唯一真相。** 行号引用必须能在
`dist/` 或 `src/` 里解析出来；`scripts/pi-oracle-lock.py --verify-refs` 会逐条检查，
解析不到就退出 1。
""",
        encoding="utf-8",
    )

    print(f"快照完成：{len(files_meta)} 个文件 → {out}")
    if missing:
        print(f"缺失（未致命）：{len(missing)} 个")
        for m in missing:
            print(f"  - {m}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
