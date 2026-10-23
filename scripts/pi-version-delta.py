#!/usr/bin/env python3
"""两个 Pi 版本快照之间的差分：文件、指标、以及**行号引用的漂移**。

为什么需要它
------------
nini 的行为层证据全是「Pi 某个文件第 N 行」。Pi 一升级，行号就可能指向
别的东西。v0.10.8 的审查实测：树里 90 处引用已经有一处越界。

而 nini 曾经同时对着**两个** Pi 版本：格式层锁 0.85.1，行为层只能对
0.84.2（本机装着的那个）。这不是"分层"，是**目标不唯一**——同一段代码，
两个版本可能给出两种答案。

本脚本把"两个版本差在哪"变成可复算的事实：
  1. 文件层面：新增 / 删除 / 变更（sha256）
  2. 指标层面：provider 数、协议 dialect 数、工具名集合
  3. **引用层面**：树里每一处行号引用，在两个版本里分别解析成什么内容

用法
----
    python3 scripts/pi-version-delta.py \\
        --from references/pi-0.84.2 --to references/pi-0.85.1

    python3 scripts/pi-version-delta.py --from ... --to ... --json

退出码：0 = 全部引用在目标版本里仍能解析；1 = 有引用在目标版本里越界/失配。
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent



def load_lock(snap: Path) -> dict:
    return json.loads((snap / "SNAPSHOT.lock").read_text(encoding="utf-8"))


sys.path.insert(0, str(Path(__file__).resolve().parent))
from pi_refs import resolve  # noqa: E402




def line_at(path: Path, n: int) -> str | None:
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except Exception:
        return None
    if n < 1 or n > len(lines):
        return None
    return lines[n - 1].strip()


def metrics(snap: Path) -> dict:
    out: dict = {}
    lock = load_lock(snap)
    out["piVersion"] = lock.get("piVersion")
    out["files"] = len(lock.get("files", {}))

    # provider 数：找含 builtinProviders 的文件，数它 return 的构造调用
    prov = None
    for rel, meta in lock.get("files", {}).items():
        if meta.get("group") == "provider" and rel.endswith("all.js"):
            prov = snap / rel
    if prov is None:
        for p in (snap / "dist").rglob("*.js"):
            if "builtinProviders" in p.read_text(encoding="utf-8", errors="ignore"):
                prov = p
                break
    if prov:
        text = prov.read_text(encoding="utf-8", errors="ignore")
        m = re.search(r"export function builtinProviders\(\)\s*\{\s*return \[(.*?)\];", text, re.S)
        if m:
            out["providers"] = len(re.findall(r"(\w+)\(\)", m.group(1)))

    # 工具名集合
    tools = None
    for p in (snap / "dist").rglob("*.js"):
        if "allToolNames" in p.read_text(encoding="utf-8", errors="ignore"):
            tools = p
            break
    if tools:
        text = tools.read_text(encoding="utf-8", errors="ignore")
        m = re.search(r"allToolNames\s*=\s*new Set\(\[(.*?)\]\)", text, re.S)
        if m:
            out["toolNames"] = sorted(set(re.findall(r'"([^"]+)"', m.group(1))))

    out["apiDialects"] = len(list((snap / "dist" / "api").glob("*.js"))) if (snap / "dist" / "api").is_dir() else 0
    out["authFiles"] = len(list((snap / "dist" / "auth").rglob("*.js"))) if (snap / "dist" / "auth").is_dir() else 0
    return out


def file_delta(a: Path, b: Path) -> dict:
    la, lb = load_lock(a), load_lock(b)
    fa, fb = la.get("files", {}), lb.get("files", {})
    added = sorted(set(fb) - set(fa))
    removed = sorted(set(fa) - set(fb))
    changed = sorted(
        f for f in set(fa) & set(fb) if fa[f]["sha256"] != fb[f]["sha256"]
    )
    same = len(set(fa) & set(fb)) - len(changed)
    return {
        "added": added,
        "removed": removed,
        "changed": changed,
        "unchanged": same,
        "common": len(set(fa) & set(fb)),
    }


def refs() -> list[dict]:
    r = subprocess.run(
        [sys.executable, "scripts/pi-line-refs.py", "--json"],
        cwd=REPO, capture_output=True, text=True,
    )
    if r.returncode != 0:
        print(r.stderr, file=sys.stderr)
        return []
    return json.loads(r.stdout).get("refs", [])


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--from", dest="frm", required=True)
    ap.add_argument("--to", dest="to", required=True)
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    a, b = (REPO / args.frm if not Path(args.frm).is_absolute() else Path(args.frm)), \
           (REPO / args.to if not Path(args.to).is_absolute() else Path(args.to))

    ma, mb = metrics(a), metrics(b)
    fd = file_delta(a, b)

    # 引用漂移
    drift = []
    def first_with_symbol(path: Path | None, sym: str) -> tuple[int | None, str | None]:
        """符号引用：在文件里找它第一次出现的位置。"""
        if path is None or not sym:
            return None, None
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        pat = re.compile(r"\b" + re.escape(sym) + r"\b")
        for i, l in enumerate(lines, start=1):
            if pat.search(l):
                return i, l.strip()
        return None, None

    for ref in refs():
        doc, ln = ref.get("doc"), ref.get("doc_line")
        pa, pb = resolve(a, doc), resolve(b, doc)
        if ln is None:
            # 符号引用：解析成两个版本里各自的首次出现
            ia, ta = first_with_symbol(pa, ref.get("symbol") or "")
            ib, tb = first_with_symbol(pb, ref.get("symbol") or "")
            if ia and ib:
                status = "same" if ia == ib else "MOVED"
            elif ib is None:
                status = "SYMBOL-GONE-in-target"
            elif ia is None:
                status = "new-in-target"
            else:
                status = "unresolved-in-both"
            drift.append({
                "file": ref.get("file"), "line": ref.get("line"),
                "doc": doc, "doc_line": f"#{ref.get('symbol')}",
                "status": status, "from_text": ta, "to_text": tb,
                "from_line": ia, "to_line": ib,
            })
            continue
        la = line_at(pa, ln) if pa else None
        lb = line_at(pb, ln) if pb else None
        if la is None and lb is None:
            status = "unresolved-in-both"
        elif lb is None:
            status = "OUT-OF-RANGE-in-target"
        elif la is None:
            status = "new-in-target"
        elif la == lb:
            status = "same"
        else:
            status = "CONTENT-CHANGED"
        drift.append({
            "file": ref.get("file"), "line": ref.get("line"),
            "doc": doc, "doc_line": ln, "status": status,
            "from_text": la, "to_text": lb,
        })

    bad = [d for d in drift if d["status"] in (
        "OUT-OF-RANGE-in-target", "unresolved-in-both", "SYMBOL-GONE-in-target")]
    changed = [d for d in drift if d["status"] == "CONTENT-CHANGED"]

    result = {
        "from": ma, "to": mb,
        "files": {
            "added": len(fd["added"]), "removed": len(fd["removed"]),
            "changed": len(fd["changed"]), "unchanged": fd["unchanged"],
        },
        "metrics_changed": {
            k: {"from": ma.get(k), "to": mb.get(k)}
            for k in set(ma) | set(mb)
            if k != "files" and ma.get(k) != mb.get(k)
        },
        "refs": {
            "total": len(drift),
            "same": sum(1 for d in drift if d["status"] == "same"),
            "content_changed": len(changed),
            "out_of_range": len(bad),
            "details": drift,
        },
        "file_detail": {"added": fd["added"], "removed": fd["removed"], "changed": fd["changed"]},
    }

    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=2))
    else:
        print(f"=== 指标 {ma['piVersion']} → {mb['piVersion']}")
        for k in sorted(set(ma) | set(mb)):
            if k == "files":
                continue
            mark = "  " if ma.get(k) == mb.get(k) else "→ "
            print(f"  {mark}{k}: {ma.get(k)} → {mb.get(k)}")
        print(f"\n=== 文件：新增 {len(fd['added'])} / 删除 {len(fd['removed'])} / "
              f"变更 {len(fd['changed'])} / 未变 {fd['unchanged']}")
        print(f"=== 行号引用 {len(drift)} 处："
              f"未变 {result['refs']['same']} / 内容变了 {len(changed)} / 目标版本里越界 {len(bad)}")
        for d in changed[:20]:
            print(f"  ~ {d['file']}:{d['line']}  {d['doc']}:{d['doc_line']}")
            print(f"      旧: {d['from_text'][:90]}")
            print(f"      新: {d['to_text'][:90]}")
        for d in bad[:20]:
            print(f"  ✗ {d['file']}:{d['line']}  {d['doc']}:{d['doc_line']} —— {d['status']}")
    return 1 if bad else 0


if __name__ == "__main__":
    raise SystemExit(main())
