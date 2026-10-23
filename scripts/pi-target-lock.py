#!/usr/bin/env python3
"""**对标目标版本冻结门禁**：nini 的 parity 工作锚定**一个**版本，直到 owner 宣布解除。

为什么要有这个门禁
------------------
owner 的长期指令（2026-10-23）：「以后的 nini 对标，统一锚定一个版本，直到我认为
完全没有问题为止」。

一条规矩如果只活在对话里，它会在下一次会话蒸发；而**没有门禁的规矩等于没有规矩**。
所以把它变成机器判定：违反即退出 1。

检查项
------
C1  **只有一个快照**：`references/` 下有且仅有一个 `pi-*` 快照目录。
    两个快照 = 两个对照物 = 同一段代码两种答案。
C2  **快照版本 == 目标版本**：快照的 `SNAPSHOT.lock` 里的 `piVersion` 必须等于
    `references/TARGET.json` 的 `targetVersion`。
C3  **格式层同版本**：`references/spec-v0.85.1/` 必须与目标版本同版本号。
C4  **本机 Pi == 目标版本**：装了 pi 就必须同版本；没装（CI 里）则跳过并说明。
    版本不一致意味着「我读的和别人读的不是同一份东西」。
C5  **引用只指向目标快照**：树里不得出现指向别的 Pi 版本的路径或注释式引用。
C6  **权威文档声明同一个目标**：`docs/GOALS.md` 与 `docs/plans/parity-with-pi.md`
    必须都指向 TARGET.json 里的版本与快照。
C7  **冻结状态为真**：`frozen` 必须为 true；解除冻结是 owner 的动作，不是脚本的。

用法
----
    python3 scripts/pi-target-lock.py
    python3 scripts/pi-target-lock.py --json

退出码：0 全部通过；1 有违反项（逐条给出该改哪里）。
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TARGET_FILE = REPO / "references" / "TARGET.json"
REFS = REPO / "references"


def load_target() -> dict:
    if not TARGET_FILE.is_file():
        raise SystemExit(f"缺少 {TARGET_FILE}——目标版本没有唯一来源，门禁无法判定")
    return json.loads(TARGET_FILE.read_text(encoding="utf-8"))


def installed_pi_version() -> str | None:
    exe = shutil.which("pi")
    if not exe:
        return None
    try:
        out = subprocess.run([exe, "--version"], capture_output=True, text=True, timeout=30)
        m = re.search(r"(\d+\.\d+\.\d+)", out.stdout + out.stderr)
        return m.group(1) if m else None
    except Exception:
        return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    t = load_target()
    target = t["targetVersion"]
    checks: list[dict] = []

    def add(name: str, ok: bool, detail: str = "") -> None:
        checks.append({"check": name, "ok": bool(ok), "detail": detail})

    # C1 只有一个快照
    snaps = sorted(p.name for p in REFS.glob("pi-*") if (p / "SNAPSHOT.lock").is_file())
    add("C1 references/ 下只有一个 Pi 快照", len(snaps) == 1,
        f"发现 {len(snaps)} 个：{snaps}（目标快照应为 {t['snapshot'].split('/')[-1]}）")

    # C2 快照版本 == 目标
    snap_dir = REPO / t["snapshot"]
    lock = snap_dir / "SNAPSHOT.lock"
    if lock.is_file():
        meta = json.loads(lock.read_text(encoding="utf-8"))
        add("C2 快照里的 piVersion == 目标版本", meta.get("piVersion") == target,
            f"快照说 {meta.get('piVersion')}，目标说 {target}")
    else:
        add("C2 快照里的 piVersion == 目标版本", False, f"缺少 {lock}")

    # C3 格式层同版本
    spec_dir = REPO / t["spec"]
    spec_lock = spec_dir / "SPEC.lock"
    if spec_lock.is_file():
        meta = json.loads(spec_lock.read_text(encoding="utf-8"))
        spec_tag = str(meta.get("specTag", "")).lstrip("v")
        add("C3 格式层 spec 与目标同版本", spec_tag == target,
            f"spec 是 {spec_tag}，目标是 {target}")
    else:
        add("C3 格式层 spec 与目标同版本", False, f"缺少 {spec_lock}")

    # C4 本机 Pi
    pi_ver = installed_pi_version()
    if pi_ver is None:
        add("C4 本机安装的 Pi == 目标版本", True,
            "本机没有 pi（CI 环境）——跳过；对照物在仓库快照里，仍然受锁")
    else:
        add("C4 本机安装的 Pi == 目标版本", pi_ver == target,
            f"本机 pi 是 {pi_ver}，目标是 {target}。装了别的版本 = 你读的和仓库里锁的不是同一份东西")

    # C5 引用只指向目标快照
    stray: list[str] = []
    for p in (REPO / "crates").rglob("*.rs"):
        if "target" in p.parts:
            continue
        try:
            text = p.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        for m in re.finditer(r"references/pi-(\d+\.\d+\.\d+)", text):
            if m.group(1) != target:
                stray.append(f"{p.relative_to(REPO)}: references/pi-{m.group(1)}")
    add("C5 代码里没有指向别的 Pi 版本的路径", not stray,
        "；".join(stray[:6]))

    # C6 权威文档指向同一目标
    docs = {
        "docs/GOALS.md": (REPO / "docs" / "GOALS.md"),
        "docs/plans/parity-with-pi.md": (REPO / "docs" / "plans" / "parity-with-pi.md"),
    }
    for name, path in docs.items():
        if not path.is_file():
            add(f"C6 {name} 声明同一个目标", False, "文件不存在")
            continue
        text = path.read_text(encoding="utf-8", errors="ignore")
        ok = t["snapshot"] in text and target in text
        add(f"C6 {name} 声明同一个目标", ok,
            "" if ok else f"没有提到 {target} 或 {t['snapshot']}")

    # C7 冻结状态
    add("C7 目标处于冻结状态（解除是 owner 的动作）", t.get("frozen") is True,
        f"frozen={t.get('frozen')}")

    ok = all(c["ok"] for c in checks)
    if args.json:
        print(json.dumps({"ok": ok, "target": target, "checks": checks},
                         ensure_ascii=False, indent=2))
    else:
        print(f"对标目标版本：{target}（冻结中）")
        for c in checks:
            mark = "✓" if c["ok"] else "✗"
            print(f"  {mark} {c['check']}" + (f"  —— {c['detail']}" if c["detail"] and not c["ok"] else ""))
        print(f"\n{'PASS' if ok else 'FAIL'}: {sum(c['ok'] for c in checks)}/{len(checks)} 项通过")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
