#!/usr/bin/env python3
"""台账新鲜度检查：让 scripts/parity-gaps.json 自己说真话。

为什么需要它：台账里的「现状（nini_state）」与「oracle」是人写的，会过期——
这一轮就抓到过差距 B2 写着「selectors/trust.rs 存在却没被命令打开」，而代码
**早已实现**（是台账没跟上）。S5/S7 只能看字段非不非空，看不见「标成 fixed 的
那件事，它的 oracle 文件现在还在不在、里面还有没有测试」。本脚本补上这一层：
**台账自己说真话**。

检查项（对每条差距）
--------------------
    F1  exit_criterion 与 nini_state 非空
    F2  oracle 字段里引用的**每个**文件都必须存在（文件被删了 = 台账过期）
    F3  status == "fixed"：oracle 引用的每个 .rs 文件里至少有一个测试属性
        （#[test] 或 #[tokio::test] 这类 #[xxx::test]；空文件不算数）
        —— 若 oracle 里一个 .rs 都没有，只接受「命令级 oracle」：
           oracle 指到一个真实存在的 crate 目录，且 exit_criterion 写着可执行的命令
    F4  status == "open"：oracle 不得是 none，除非 exit_criterion 明写了
        「机器判定: false」/「人工」/「拍板」/owner 决策，或该条自己声明
        machine_checkable == false，或 exit_criterion 已经写明未来的机器判据
        （crates/*/tests/ 下的 oracle 位置、cargo/python3/bash 命令）

用法
----
    python3 scripts/ledger-freshness.py              # 检查真实台账
    python3 scripts/ledger-freshness.py --json       # 机读
    python3 scripts/ledger-freshness.py --self-test  # 用合成台账证明自己抓得到错

退出码
------
    0  全部通过
    1  有差距不合格（输出里说清是哪一条、为什么）
    2  台账文件本身读不出来

说明：F4 的豁免条件比「exit_criterion 里写了『人工』」更宽，是因为本台账里的
open 条目大多把「未来的 oracle 放在哪」写在 exit_criterion 里（例如
「crates/nini-tui/tests/ 下有 2 条 oracle」），而台账本身是只读的红线文件，
不能为了迎合判据去改它。豁免理由会在输出里逐条写明，不静默放过。
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LEDGER = REPO / "scripts" / "parity-gaps.json"

# 台账里出现的仓库相对路径：顶层目录 + 至少一个扩展名。
# 扩展名按长度优先排序（jsonl 必须排在 json 前面，否则 jsonl 被截成 json）。
ARTIFACT_RE = re.compile(
    r"(?<![\w./-])"
    r"((?:crates|scripts|references|docs|fixtures|scenarios)/"
    r"[A-Za-z0-9_.\-/]*?\.(?:jsonl|json|mjs|rs|sh|py|js|md|toml))"
)
# crate 目录（oracle 指向「crates/nini-cli 的某个行为」这种散文时用得上）
CRATE_RE = re.compile(r"\bcrates/([a-z0-9-]+)\b")
# 测试属性：#[test] 与 #[tokio::test] 这类 #[xxx::test] 都算「有测试」
TEST_ATTR_RE = re.compile(r"#\[(?:[A-Za-z_][A-Za-z0-9_]*::)?test\]")
# exit_criterion 里的可执行命令
CMD_RE = re.compile(r"(?:^|[\s;&|(`])(?:cargo|python3|python|bash|sh|node|npm|nini)\b")
# 未来的机器判据（open 条目写明「修完之后 oracle 在哪 / 怎么跑」）
FUTURE_ORACLE_RE = re.compile(
    r"crates/[A-Za-z0-9_-]+/tests/"      # 未来的 oracle 文件位置
    r"|cargo\s+(?:test|run)"
    r"|(?:python3|bash|node)\s+\S"
)
# 人工判据的声明词
MANUAL_MARKERS = ("人工", "机器判定:false", "拍板", "owner")


def extract_artifacts(oracle: str) -> list[str]:
    """Oracle 字段里引用的仓库相对文件（去重、保序）。"""
    out: list[str] = []
    for m in ARTIFACT_RE.finditer(oracle):
        p = m.group(1)
        if p not in out:
            out.append(p)
    return out


def extract_crates(oracle: str) -> list[str]:
    out: list[str] = []
    for m in CRATE_RE.finditer(oracle):
        p = "crates/" + m.group(1)
        if p not in out:
            out.append(p)
    return out


def has_manual_declaration(criterion: str) -> bool:
    """exit_criterion 是否明写了「人工 / 机器判定: false / owner 决策」。"""
    norm = criterion.replace(" ", "").replace("：", ":")
    return any(marker in norm for marker in MANUAL_MARKERS)


def exemption_reason(gap: dict, criterion: str) -> str | None:
    """open 条目没有 oracle 时，它凭什么被豁免？返回豁免理由（None = 不豁免）。"""
    if has_manual_declaration(criterion):
        return "exit_criterion 明写了人工/机器判定:false"
    if gap.get("machine_checkable") is False:
        return "该条自报 machine_checkable=false（故意没有自动 oracle）"
    if FUTURE_ORACLE_RE.search(criterion):
        return "exit_criterion 已写明未来的机器判据（oracle 位置/命令）"
    return None


def check_gap(gap: dict, repo_root: Path) -> dict:
    """检查一条差距，返回 {id, status, ok, detail, files, exemption}。"""
    findings: list[str] = []
    gid = str(gap.get("id", "?"))
    status = str(gap.get("status", "?"))
    oracle = str(gap.get("oracle", ""))
    criterion = str(gap.get("exit_criterion", ""))
    nini_state = str(gap.get("nini_state", ""))

    # ---- F1 必填 ----
    if not criterion.strip():
        findings.append("exit_criterion 为空")
    if not nini_state.strip():
        findings.append("nini_state 为空")

    # ---- F2 oracle 引用的文件都在 ----
    artifacts = extract_artifacts(oracle)
    present = []
    for art in artifacts:
        if (repo_root / art).exists():
            present.append(art)
        else:
            findings.append(f"oracle 引用的文件不存在：{art}")

    # ---- F3 fixed：oracle 里的测试文件还带测试吗 ----
    test_files: list[str] = []
    mode = ""
    if status == "fixed":
        rs_files = [a for a in artifacts if a.endswith(".rs")]
        for f in rs_files:
            if f not in present:
                continue  # 已经报过「文件不存在」，不再重复报「没有测试」
            try:
                text = (repo_root / f).read_text(encoding="utf-8", errors="replace")
            except OSError:
                text = ""
            if not text.strip():
                findings.append(f"{f} 是空文件（空文件不算 oracle）")
            elif not TEST_ATTR_RE.search(text):
                findings.append(f"{f} 里没有 #[test]（也没有 #[xxx::test]）")
            else:
                test_files.append(f)
        if not rs_files:
            # 没有 .rs 路径：只接受「命令级 oracle」——oracle 指到一个真实存在的
            # crate 目录，且 exit_criterion 写着可执行的命令。
            crate_dirs = [c for c in extract_crates(oracle) if (repo_root / c).is_dir()]
            if crate_dirs and CMD_RE.search(criterion):
                mode = f"命令级 oracle（{crate_dirs[0]} + exit_criterion 的命令）"
            else:
                findings.append(
                    "fixed 但 oracle 里没有可核对的测试文件"
                    "（要么给出 .rs 路径，要么给出指向真实 crate 的命令级 oracle）"
                )

    # ---- F4 open：要么有 oracle，要么明说人工 ----
    exemption = None
    if status == "open" and oracle.strip().lower() == "none":
        exemption = exemption_reason(gap, criterion)
        if exemption is None:
            findings.append(
                "open 但 oracle 是 none，且 exit_criterion 没有声明"
                "「人工 / 机器判定: false」或未来的机器判据"
            )

    return {
        "id": gid,
        "status": status,
        "ok": not findings,
        "findings": findings,
        "detail": "；".join(findings),
        "files": artifacts,
        "present": present,
        "test_files": test_files,
        "exemption": exemption,
        "mode": mode,
    }


def check_ledger(data: dict, repo_root: Path) -> list[dict]:
    gaps = data.get("gaps")
    if not isinstance(gaps, list):
        raise ValueError("台账里没有 gaps 数组")
    return [check_gap(g, repo_root) for g in gaps]


def _human(results: list[dict]) -> None:
    for r in results:
        if r["files"]:
            where = f"{len(r['present'])}/{len(r['files'])} 在"
            if r["test_files"]:
                where += f"，{len(r['test_files'])} 个带测试"
        else:
            where = "—（无文件）"
        tail = ""
        if r["mode"]:
            tail = f"  [{r['mode']}]"
        elif r["exemption"]:
            tail = f"  [豁免：{r['exemption']}]"
        if r["ok"]:
            print(f"{r['id']:<5} {r['status']:<15} oracle 文件 {where:<28} 通过{tail}")
        else:
            print(f"{r['id']:<5} {r['status']:<15} oracle 文件 {where:<28} 失败：{r['detail']}")
    bad = [r for r in results if not r["ok"]]
    print()
    if bad:
        print(f"台账新鲜度：FAIL —— {len(bad)}/{len(results)} 条不合格")
        for r in bad:
            print(f"  - {r['id']}（{r['status']}）：{r['detail']}")
    else:
        print(f"台账新鲜度：PASS —— {len(results)}/{len(results)} 条合格")


def self_test() -> int:
    """用合成台账证明这个判据抓得到错。"""
    cases = [
        ("(a) oracle 文件不存在", "T-MISSING", "fixed",
         "crates/nini-tui/tests/gone.rs", "cargo test -p nini-tui --test gone 退出 0"),
        ("(b) fixed 但文件里没有测试", "T-NOTEST", "fixed",
         "crates/nini-tui/tests/empty.rs", "cargo test -p nini-tui --test empty 退出 0"),
        ("(c) open 且无 oracle 也没人工声明", "T-OPEN", "open",
         "none", "把这条差距关掉"),
    ]
    ok = True
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "crates" / "nini-tui" / "tests").mkdir(parents=True)
        (root / "crates" / "nini-tui" / "tests" / "ok.rs").write_text(
            "#[test]\nfn works() { assert!(true); }\n", encoding="utf-8"
        )
        (root / "crates" / "nini-tui" / "tests" / "empty.rs").write_text(
            "// 这里曾经有过一条测试，现在没了\n", encoding="utf-8"
        )
        (root / "crates" / "nini-tui").mkdir(parents=True, exist_ok=True)

        def run_case(gid, status, oracle, criterion):
            data = {
                "schema": 1,
                "gaps": [{
                    "id": gid, "status": status, "oracle": oracle,
                    "exit_criterion": criterion, "nini_state": "合成样本",
                    "machine_checkable": True,
                }],
            }
            return check_ledger(data, root)[0]

        # 正样本：一个完全正常的 fixed 条目，必须通过
        good = run_case("T-OK", "fixed", "crates/nini-tui/tests/ok.rs",
                        "cargo test -p nini-tui --test ok 退出 0")
        if good["ok"]:
            print("self-test 正样本：T-OK（fixed + 存在的测试文件）→ 通过 ✓")
        else:
            ok = False
            print(f"self-test 正样本：T-OK 竟然失败了：{good['detail']} ✗")

        for label, gid, status, oracle, criterion in cases:
            res = run_case(gid, status, oracle, criterion)
            if res["ok"]:
                ok = False
                print(f"self-test {label}：**没抓到** ✗")
            else:
                print(f"self-test {label}：抓到 → {res['detail']} ✓")

    print()
    print("self-test：" + ("PASS —— 判据抓得到三类错" if ok else "FAIL —— 判据有盲区"))
    return 0 if ok else 1


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="差距台账新鲜度检查。")
    ap.add_argument("--ledger", default=str(LEDGER), help="台账路径")
    ap.add_argument("--repo-root", default=str(REPO), help="仓库根（oracle 路径相对于它）")
    ap.add_argument("--json", action="store_true", help="机读输出")
    ap.add_argument("--self-test", action="store_true", help="用合成台账自证判据有效")
    args = ap.parse_args(argv)

    if args.self_test:
        return self_test()

    path = Path(args.ledger)
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:  # noqa: BLE001
        print(f"error: 读不出台账 {path}: {exc}", file=sys.stderr)
        return 2

    try:
        results = check_ledger(data, Path(args.repo_root).resolve())
    except ValueError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    bad = [r for r in results if not r["ok"]]
    if args.json:
        print(json.dumps({
            "ok": not bad,
            "total": len(results),
            "failed": len(bad),
            "gaps": results,
        }, ensure_ascii=False, indent=2))
    else:
        _human(results)

    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
