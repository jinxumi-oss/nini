#!/usr/bin/env python3
"""计划自检：计划文档与机器台账必须说同一件事。

为什么需要它：v2 的 §2 是 Markdown 表格、§5 的计数是手写的，于是
「A 类未闭 4 / B 类未闭 10」这种数字在文档里躺了很久，直到某天有人
手工核对才发现它和代码对不上。v3 把差距搬进 scripts/parity-gaps.json，
本脚本负责**不让文档把它说错**。

用法：
    python3 scripts/plan-selfcheck.py            # 全量检查，退出码 0 = 通过
    python3 scripts/plan-selfcheck.py --json     # 机读

检查项：
    S1  差距台账自洽（脚本自身退出 0）
    S2  计划文档的必备章节齐全（§0..§10）
    S3  计划文档与 GOALS 的排序规则冲突已消除（GOALS 指向本计划）
    S4  计划文档引用了台账脚本与 Pi 快照锁
    S5  台账里每条差距的必填字段非空、判据非空
    S6  blocked-owner 的条目必须挂在某个 owner 决策上
    S7  标成 fixed 的条目必须有 oracle（不能「修好了但没有断言」）
    S8  文档里不得出现已作废的数字（51 个 provider / 5 原生），
        除非它出现在「已被推翻」的历史表里
    S9  计划文档不得手写台账计数（计数归脚本所有）
    S14  台账新鲜度（fixed 的 oracle 文件都还在、open 条目要么有 oracle 要么明说人工）
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PLAN = REPO / "docs" / "plans" / "parity-with-pi.md"
GOALS = REPO / "docs" / "GOALS.md"
LEDGER = REPO / "scripts" / "parity-gaps.json"

REQUIRED_SECTIONS = [
    "## 0.", "## 1.", "## 2.", "## 3.", "## 4.", "## 5.", "## 6.",
    "## 7.", "## 8.", "## 9.", "## 10.",
]
REQUIRED_GAP_FIELDS = [
    "id", "class", "severity", "title", "pi_basis", "nini_state",
    "oracle", "status", "round", "exit_criterion", "machine_checkable",
]
def _owner_decisions() -> set:
    """决策号白名单**只有一份**：从 scripts/parity-gap.py 里读。

    v0.10.8：这个集合此前在两个脚本里各抄一份，于是加了 R11 之后，
    parity-gap 认、B10 通过，plan-selfcheck 却说「没挂决策号」——
    **同一份真相的两份副本**（这个项目在键位表上已经付过一次学费）。
    """
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "parity_gap_mod", Path(__file__).resolve().parent / "parity-gap.py"
    )
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return set(mod.DECISIONS)


def run(cmd):
    return subprocess.run(cmd, cwd=REPO, capture_output=True, text=True)


def check(results, name, ok, detail=""):
    results.append({"check": name, "ok": bool(ok), "detail": detail})
    return bool(ok)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    results = []

    # C1 台账自洽
    r = run([sys.executable, "scripts/parity-gap.py", "--json"])
    check(results, "S1 台账自洽（parity-gap.py --json 退出 0）",
          r.returncode == 0, r.stderr.strip()[:300])
    try:
        ledger = json.loads(LEDGER.read_text(encoding="utf-8"))
        gaps = ledger["gaps"]
    except Exception as e:  # noqa: BLE001
        check(results, "S1 台账自洽", False, f"读不出台账：{e}")
        gaps = []

    plan = PLAN.read_text(encoding="utf-8") if PLAN.exists() else ""
    goals = GOALS.read_text(encoding="utf-8") if GOALS.exists() else ""

    # C2 必备章节
    missing = [s for s in REQUIRED_SECTIONS if s not in plan]
    check(results, "S2 计划文档章节齐全（§0..§10）", not missing,
          f"缺：{missing}" if missing else f"{len(REQUIRED_SECTIONS)} 节齐全")

    # C3 排序规则冲突已消除：GOALS 必须指向 parity plan 的 §3
    check(results, "S3 GOALS 与计划的排序规则不再冲突",
          "parity-with-pi.md" in goals and "功能面" in goals,
          "GOALS 没有指向 plans/parity-with-pi.md 的排序规则")

    # C4 台账与快照锁被引用
    refs = ["scripts/parity-gap.py", "scripts/parity-gaps.json",
            "scripts/pi-line-refs.py", "scripts/pi-oracle-lock.py",
            "scripts/pi_refs.py",
            "references/pi-0.85.1"]
    missing_refs = [x for x in refs if x not in plan]
    check(results, "S4 计划引用了台账脚本与 Pi 快照锁", not missing_refs,
          f"缺：{missing_refs}" if missing_refs else "全部引用到位")

    # C5 必填字段
    bad = []
    for g in gaps:
        for f in REQUIRED_GAP_FIELDS:
            if f not in g or g[f] in (None, ""):
                bad.append(f"{g.get('id', '?')}.{f}")
    check(results, "S5 每条差距的必填字段与判据非空", not bad,
          f"缺：{bad[:8]}" if bad else f"{len(gaps)} 条齐全")

    # C6 blocked-owner 必须挂在决策上
    decisions = _owner_decisions()
    bad = [g["id"] for g in gaps
           if g.get("status") == "blocked-owner"
           and g.get("blocking_decision") not in decisions]
    check(results, "S6 被 owner 决策阻塞的条目都挂着决策号", not bad,
          f"问题条目：{bad}" if bad else "全部挂着 R4/R5/R9/R10")

    # C7 fixed 必须有 oracle
    bad = [g["id"] for g in gaps
           if g.get("status") == "fixed" and str(g.get("oracle", "")).strip().lower() == "none"]
    check(results, "S7 标成 fixed 的条目都有 oracle", not bad,
          f"无 oracle 却标 fixed：{bad}" if bad else "全部有 oracle")

    # S8 已作废的数字只能出现在「已被推翻」的历史段落里
    STALE_PATTERNS = [r"51 个", r"5 原生", r"51 providers", r"5 of ~51"]
    stale = []
    for pat in STALE_PATTERNS:
        for m in re.finditer(pat, plan):
            ctx = plan[max(0, m.start() - 400): m.start() + 200]
            if "已被推翻" not in ctx and "推翻" not in ctx:
                stale.append(pat)
    check(results, "S8 已作废的数字没有以现状出现", not stale,
          f"仍然出现：{sorted(set(stale))}" if stale else "只出现在「已被推翻」表里")

    # S9 文档不得手写台账计数
    #
    # 这一条曾经是**空转**的：它的正则写错了，一条也匹配不到，于是永远
    # 通过。审查指出这一点之后，这里换成一组真的会命中的模式，并且
    # S10 会用合成的样例验证这些模式本身是活的——**一个抓不到东西的
    # 判据不是判据，是装饰**。
    COUNT_PATTERNS = [
        r"\d+\s*条差距",
        r"\*\*\d+\s*条\*\*",
        r"A\s*类\s*\d+",
        r"共\s*\d+\s*条",
    ]
    hand_counts = []
    for pat in COUNT_PATTERNS:
        for m in re.finditer(pat, plan):
            hand_counts.append(m.group(0))
    check(results, "S9 计划文档不手写台账计数", not hand_counts,
          f"手写的计数：{hand_counts}" if hand_counts else "计数全部交给脚本")

    # S10 判据本身要有效：每一条基于正则的判据，都必须能抓到它自己的
    # 合成样本。v0.10 记过一个真实的教训：有人在测试文件里插一行注释，
    # 不变量的覆盖数就变了，而门禁照样绿——因为「这条断言是否与这条
    # 不变量有关」不是机器能判断的。这里能做的是更弱但仍有价值的事：
    # **确保判据不是死的**。
    dead = []
    for pat, sample in [
        (r"\d+\s*条差距", "共 19 条差距"),
        (r"\*\*\d+\s*条\*\*", "**19 条**"),
        (r"A\s*类\s*\d+", "A 类 4 / B 类 11"),
        (r"共\s*\d+\s*条", "共 22 条"),
        (r"51 个", "51 个 Pi provider"),
        (r"5 原生", "5 原生 + models.json"),
        (r"51 providers", "51 providers implemented natively"),
        (r"5 of ~51", "5 of ~51 Pi providers"),
    ]:
        if not re.search(pat, sample):
            dead.append(pat)
    check(results, "S10 每条正则判据都抓得到它自己的样本（判据不是死的）",
          not dead, f"这些模式抓不到任何东西：{dead}" if dead else
          f"{len(COUNT_PATTERNS) + len(STALE_PATTERNS)} 条模式全部有效")

    # S12 工具对标计划（WF-BUILTIN）存在，且它的判据看板已接线
    tp = REPO / "docs" / "plans" / "tool-parity.md"
    ok_tp = tp.is_file()
    if ok_tp:
        ttext = tp.read_text(encoding="utf-8")
        need = ["## 0.", "## 1.", "## 2.", "## 3.", "## 4.", "## 5.", "## 6.", "## 7.", "## 8.",
                "T1", "T2", "T3", "T4", "退出判据", "红的凭据"]
        miss = [x for x in need if x not in ttext]
        ok_tp = not miss
        detail = f"缺：{miss}" if miss else ""
    else:
        detail = "docs/plans/tool-parity.md 不存在"
    check(results, "S12 工具对标计划存在且四个工作项与判据齐全", ok_tp, detail)
    r2 = run([sys.executable, "scripts/tool-parity-check.py", "--quick"])
    check(results, "S12b 工具对标判据看板可运行", r2.returncode in (0, 1),
          r2.stderr.strip()[-200:] if r2.returncode not in (0, 1) else "")

    # S11 对标目标版本冻结（owner 的长期指令）
    r = run([sys.executable, "scripts/pi-target-lock.py"])
    check(results, "S11 对标目标版本唯一且冻结（pi-target-lock）", r.returncode == 0,
          r.stdout.strip()[-400:] if r.returncode != 0 else "")

    # S14 台账新鲜度：台账自己说真话
    #
    # S5/S7 只看字段非不非空，看不见「标成 fixed 的那条差距，它的 oracle 文件
    # 现在还在不在、里面还有没有测试」。台账会过期（B2 就是台账落后于代码的
    # 例子），所以这一项把台账的 oracle 拉回磁盘上核对一遍；同时跑一遍
    # --self-test，证明这个判据抓得到错（抓不到东西的判据是装饰）。
    r = run([sys.executable, "scripts/ledger-freshness.py"])
    st = run([sys.executable, "scripts/ledger-freshness.py", "--self-test"])
    s14_ok = r.returncode == 0 and st.returncode == 0
    s14_detail = ""
    if r.returncode != 0:
        s14_detail = (r.stdout + r.stderr).strip()[-600:]
    elif st.returncode != 0:
        s14_detail = (st.stdout + st.stderr).strip()[-600:]
    check(results, "S14 台账新鲜度：fixed 条目的 oracle 文件都在且带测试、"
                   "open 条目要么有 oracle 要么明说人工", s14_ok, s14_detail)

    ok = all(x["ok"] for x in results)
    if args.json:
        print(json.dumps({"ok": ok, "checks": results}, ensure_ascii=False, indent=2))
    else:
        for x in results:
            print(("  ✓ " if x["ok"] else "  ✗ ") + x["check"] +
                  (f"  —— {x['detail']}" if (x["detail"] and not x["ok"]) else ""))
        print(f"\n{'PASS' if ok else 'FAIL'}: {sum(x['ok'] for x in results)}/{len(results)} 项通过")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
