#!/usr/bin/env python3
"""链③ 审计：把历史缺陷固化成不变量，并检查它们是否已被测试保护。

## 为什么存在这个脚本

nini 的验证能力目前是三条独立的供给链：

| 链 | 供给源 | 工具 |
|---|---|---|
| ① 格式层 | references/spec-v0.85.1 | sha256 校验 |
| ② 行为层 | 真 Pi 0.84.2 录制 | 尚缺 |
| ③ 不变量层 | **历史缺陷** | **本脚本** |

链③ 的供给源已经躺在仓库里：每一条 `fix:` 提交都是一次真实故障，
而它的标题本身就是一条可判定的祈使句不变量——

    a busy session lock was eleven kinds of data loss
    a user's hook panic must not end their session
    a reply wider than the terminal is folded, not cut

这些不变量是 nini 全部痛点（"改了很多轮还是有问题"）的直接解药：
每一次复发都是因为修复只落在代码里，没有落在断言里。

## 本脚本做什么

对每条 `fix:` 提交：

1. 取 diff 的**新增行标识符**与**触及文件**（修复点的机器证据）
2. 与**测试语料**求交，测试语料 = `crates/*/tests/*.rs` **加上**
   src 内所有 `#[cfg(test)]` 模块（只看 tests/ 会漏掉单元测试）
3. 分档输出：

| 档位 | 含义 |
|---|---|
| STRONG | diff 新增标识符出现在测试语料中——有断言碰到了修复点 |
| NAME  | 标题词与某个测试文件名重叠，但 diff 标识符无交集——弱证据 |
| NONE  | 无任何交集——**该不变量裸奔，修过的 bug 可以原样复发** |

## 它不是什么

它不是"覆盖率"。它是一个**待办生成器**：输出即下一轮工作的最短清单。
NONE 档每一条都对应一个可以立刻写的回归断言。

## 用法

    python3 scripts/invariants.py                 # 人读的报告
    python3 scripts/invariants.py --json          # 机读，进 gate
    python3 scripts/invariants.py --fail-on-none   # 有裸奔不变量则退出码非 0

退出码恒为 0 除非 --fail-on-none（那是给未来封版门禁用的；
现在必然非 0，因为这份清单正是待办本身）。
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# ---------------------------------------------------------------------------
# 词法
# ---------------------------------------------------------------------------

IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]{3,}")

# 英文虚词与"描述缺陷的通用动词"。这些词出现在标题里不携带定位信息，
# 留着它们会让 NAME 档看起来有覆盖，其实没有。
STOP = {
    # 冠词/介词/连词
    "the", "and", "for", "with", "that", "this", "was", "were", "are",
    "but", "not", "its", "his", "her", "their", "them", "they", "you",
    "your", "has", "have", "had", "does", "did", "done", "can", "could",
    "will", "would", "should", "than", "then", "there", "here", "into",
    "from", "out", "off", "over", "under", "one", "two", "three", "four",
    "five", "six", "seven", "eight", "nine", "ten", "each", "every",
    "some", "any", "all", "own", "same", "other", "another", "because",
    "instead", "about", "after", "before", "while", "when", "where",
    "only", "also", "both", "just", "like", "such", "very", "much",
    # 描述缺陷状态的通用词：几乎每条 fix 都有，无法定位
    "must", "never", "always", "still", "used", "uses", "using", "user",
    "users", "case", "cases", "thing", "things", "way", "ways", "part",
    "side", "kind", "kinds", "sort", "sorts", "list", "lists", "list",
    "mean", "means", "meant", "cost", "costs", "make", "makes", "made",
    "take", "takes", "took", "give", "gives", "gave", "get", "gets",
    "got", "put", "puts", "say", "says", "said", "tell", "tells", "told",
    "show", "shows", "shown", "see", "sees", "saw", "look", "looks",
    "find", "finds", "found", "keep", "keeps", "kept", "leave", "leaves",
    "left", "end", "ends", "ended", "start", "starts", "started",
    "begin", "begins", "began", "stop", "stops", "stopped", "back",
    "now", "new", "old", "first", "last", "next", "more", "less", "most",
    "least", "many", "few", "long", "short", "high", "low", "big",
    "small", "large", "old", "due", "since", "already", "still", "yet",
}


def tokenize(text: str) -> set[str]:
    """标识符 → 小写词元集合，拆分 snake_case / kebab-case / camelCase。"""
    out: set[str] = set()
    for raw in IDENT.findall(text):
        for part in re.split(r"[^A-Za-z0-9]+|[a-z][A-Z]", raw):
            part = part.lower().strip()
            if len(part) >= 4 and part not in STOP and not part.isdigit():
                out.add(part)
    return out


# Rust / 测试框架的通用词。它们在几乎每个测试文件里都出现，因此与 diff
# 求交时是**必然命中**的假证据。不滤掉它们，本脚本会输出 100% 保护率——
# 那比没有审计更危险。
NOISE = {
    "assert", "assert_eq", "expect", "unwrap", "unwrap_or", "panic",
    "test", "tests", "tokio", "main", "args", "tempdir", "tempfile",
    "serial", "env", "lock", "mutex", "guard", "atomic", "thread",
    "sleep", "duration", "instant", "vec", "string", "format", "write",
    "read", "path", "paths", "dir", "home", "file", "files", "config",
    "command", "output", "result", "option", "default", "new", "from",
    "into", "iter", "collect", "map", "filter", "clone", "drop", "borrow",
    "self", "crate", "core", "std", "rust", "impl", "struct", "enum",
    "trait", "async", "await", "spawn", "channel", "arc", "mutex_",
    "should", "would", "could", "must_", "have", "has_", "with_", "for_",
    "does", "did", "not_", "is_", "as_", "ok", "err", "some", "none",
    "true", "false", "zero", "one_", "two", "init", "setup", "helper",
    "fn_", "run", "call", "make", "get", "set", "add", "push", "line",
    "lines", "text", "data", "value", "values", "item", "items", "name",
    "names", "key", "keys", "state", "app", "runtime", "session",
}


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=REPO, capture_output=True, text=True, check=True
    ).stdout


# ---------------------------------------------------------------------------
# 测试语料
# ---------------------------------------------------------------------------


NOISE_RE = re.compile("|".join(sorted(NOISE)) + r"\b")

_COMMENT_BLOCK = re.compile(r"/\*.*?\*/", re.S)
_COMMENT_LINE = re.compile(r"//[^\n]*")
_STRING = re.compile(r'"(?:[^"\\\n]|\\.)*"' r"|'(?:[^'\\\n]|\\.)*'")


def code_only(text: str) -> str:
    """剥离注释与字符串字面量，只留代码。

    不做这一步，本脚本会把 `ensures` / `badge` / `mismatch` 这类
    **注释里的英文散文**当成代码标识符，于是 diff 与测试语料必然相交，
    输出 100% 保护率——一个看起来比没有审计更危险的结论。
    """
    text = _COMMENT_BLOCK.sub(" ", text)
    text = _COMMENT_LINE.sub(" ", text)
    return _STRING.sub('""', text)


def tokens_of(text: str) -> set[str]:
    return tokenize(NOISE_RE.sub(" ", code_only(text)))


def test_corpus() -> dict[str, set[str]]:
    """返回 {测试文件名stem: 词元集合}。

    语料包含两处，这是本脚本最容易写错的地方：
      * crates/*/tests/*.rs        —— 集成测试
      * crates/*/src/**/*.rs 中 `#[cfg(test)]` 之后的单元测试
    只扫 tests/ 会把 `theme_watcher.rs` 的 5 个单元测试算成"未保护"。
    """
    corpus: dict[str, set[str]] = {}

    for path in sorted(REPO.glob("crates/*/tests/*.rs")):
        corpus[path.stem] = tokens_of(path.read_text(errors="replace"))

    for path in sorted(REPO.glob("crates/*/src/**/*.rs")):
        text = code_only(path.read_text(errors="replace"))
        if "#[cfg(test)]" not in text:
            continue
        unit = text.split("#[cfg(test)]", 1)[1]
        name = f"{path.stem}::unit"
        corpus[name] = tokens_of(unit)

    return corpus


ANNOTATION = re.compile(r"invariant:\s*([0-9a-f]{7,40})")


def test_annotations() -> dict[str, list[str]]:
    """采集测试里的 `invariant: <sha>` 显式标注 → {sha: [文件]}。

    ## 为什么需要这个机制

    本脚本原本靠**词法重叠**判断一条 fix 有没有被断言保护：取该提交
    新增行里的专有标识符，去测试语料里找。问题是——
    **走公共 API 高层入口的测试不引用内部符号**，于是被判成"未保护"。

    这不是理论问题，2026-10-19 就实测到 4 条误判：`c3944d13`、
    `7a71f5b3`、`9c9783e3`、`3bf4815d` 都已由
    `crates/nini-ai/tests/pi_wire_oracle.rs` 与 `fallback_diagnostics.rs`
    断言（且都通过了变异验证），却仍显示 NONE。

    而那种测试恰恰是**正确**的那种：GOALS T3 第 2 条明写“接口会变的修复，
    oracle 必须走签名不变的更高层入口”，因为编译失败不是 oracle。

    所以词法猜测会系统性地**高估**未保护数，进而让基线有偏。
    显式标注把它变成**声明**：写测试的人说“这条不变量我守了”，审计据此认定。
    声明可以被撒谎，但撒谎需要写出具体断言，而断言会被变异验证咬住。
    """
    out: dict[str, list[str]] = {}
    for path in sorted(REPO.glob("crates/*/tests/*.rs")) + sorted(
        REPO.glob("crates/*/src/**/*.rs")
    ):
        try:
            text = path.read_text(errors="replace")
        except OSError:
            continue
        for sha in ANNOTATION.findall(text):
            # 标注写短 sha（人写短的全称太丑），而 git log 给的是完整 40 位。
            # 统一按短 sha 存，否则两者永远对不上——这个 bug 实测过一次，
            # 症状是加了标注而 ANNOTATED 仍然是 0。
            key = sha[:8].lower()
            out.setdefault(key, []).append(path.stem)
    return out


def doc_freq(corpus: dict[str, set[str]]) -> dict[str, int]:
    """词元 → 出现在多少个测试文件里。df 越低越具体，越能当证据。"""
    df: dict[str, int] = {}
    for toks in corpus.values():
        for t in toks:
            df[t] = df.get(t, 0) + 1
    return df


# 证据门槛：出现在超过这么多个测试文件里的词元，算"通用词"，不作证据。
DF_MAX = 3


def src_identifiers() -> set[str]:
    """所有源码与测试中的**代码**标识符全集。

    注释里的英文词不算标识符——否则见 code_only() 的说明。
    """
    out: set[str] = set()
    for path in list(REPO.glob("crates/*/src/**/*.rs")) + list(
        REPO.glob("crates/*/tests/*.rs")
    ):
        out |= tokens_of(path.read_text(errors="replace"))
    return out


# ---------------------------------------------------------------------------
# 单条不变量
# ---------------------------------------------------------------------------


def commit_tokens(sha: str) -> set[str]:
    body = git("show", sha, "--unified=0", "--format=")
    added = "\n".join(
        line[1:]
        for line in body.splitlines()
        if line.startswith("+") and not line.startswith("+++")
    )
    return tokens_of(added)


def analyze(sha: str, subject: str, corpus, test_df, commit_df, annotated) -> dict:
    body = git("show", sha, "--unified=0", "--format=")

    touched: list[str] = []
    adds: list[str] = []
    for line in body.splitlines():
        if line.startswith("+++ b/"):
            touched.append(line[6:])
        elif line.startswith("+") and not line.startswith("+++"):
            adds.append(line[1:])
    added_text = "\n".join(adds)

    # 信号 1（地面真值）：这条修复自己带不带测试？
    self_test = bool(re.search(r"#\[(?:test|tokio::test|rstest|test_case)", added_text))

    # 信号 2：证据必须是"这条修复独有的"词元。
    #   commit_df  = 该词元出现在多少条 fix 提交的改动里（全局专一度）
    #   test_df    = 该词元出现在多少个测试文件里
    # 两个门槛缺一不可：只按 test_df 过滤时，null / digit / relaxed 这类
    # 低频但无意义的词会造成假阳性，脚本会输出接近 100% 的保护率。
    added_tokens = tokens_of(added_text)
    evidence = {
        t
        for t in added_tokens
        if commit_df.get(t, 99) <= 2 and test_df.get(t, 99) <= 3
    }

    hits: list[tuple[str, list[str]]] = []
    for stem, toks in corpus.items():
        overlap = evidence & toks
        # 要求 ≥2 个独立证据词元同时命中，降低偶然碰撞
        if len(overlap) >= 2:
            hits.append((stem, sorted(overlap)))
    hits.sort(key=lambda kv: -len(kv[1]))

    backticked: set[str] = set()
    for m in re.findall(r"`([^`]+)`", subject):
        backticked |= tokens_of(m)
    weak = sorted(
        stem for stem, toks in corpus.items() if backticked & toks
    )

    if annotated.get(sha[:8].lower()):
        tier = "ANNOTATED"
    elif self_test or hits:
        tier = "SELF_TEST" if self_test else "CORROBORATED"
    elif weak:
        tier = "WEAK"
    else:
        tier = "NONE"

    return {
        "sha": sha[:8],
        "subject": subject,
        "touched": touched,
        "self_test": self_test,
        "annotated_by": annotated.get(sha[:8].lower(), []),
        "evidence": sorted(evidence)[:12],
        "hits": [{"test": s, "overlap": o} for s, o in hits[:4]],
        "weak": weak[:4],
        "tier": tier,
    }


TIER_DOC = {
    "ANNOTATED": "测试里写明 `invariant: <sha>` 显式声明保护（高层 API oracle 靠它识别）",
    "SELF_TEST": "修复自带测试——能挡回归，**挡不住当初的理解错误**（同源）",
    "CORROBORATED": "无自带测试，但有其他测试以修复点的专有词元命中",
    "WEAK": "仅反引号标识符与某测试文件相关，证据不足",
    "NONE": "无任何证据——该不变量没有任何断言保护",
}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--fail-on-none", action="store_true")
    args = ap.parse_args()

    corpus = test_corpus()
    test_df = doc_freq(corpus)
    known = src_identifiers()

    # `git log --grep='^fix'` matches the **whole message**, not the subject,
    # and `^` anchors per line inside it. So `997e8bbd` —
    # `docs(ux): extend UX report with v0.7.4 regression verification` —
    # matched because its *body* has a second line reading
    # "fixes (model display, think-tag stripping, event redraws)".
    # A pure documentation commit is not a defect invariant.
    #
    # `%s` is the subject, so filtering on it in Python is both simpler and
    # correct. (Third bug in this script, after the short/full-hash mismatch
    # and the comment-prose identifier collision.)
    shas = [
        line.split("\t", 1)
        for line in git(
            "log", "--format=%H%x09%s", "--reverse", "--grep=^fix"
        ).splitlines()
        if line.strip()
    ]
    shas = [(sha, subj) for sha, subj in shas if subj.lstrip().startswith("fix")]

    # 第一遍：收集每条修复改动引入的词元，用于算全局专一度
    history = {sha.strip(): commit_tokens(sha.strip()) for sha, _ in shas}
    commit_df: dict[str, int] = {}
    for toks in history.values():
        for t in toks:
            commit_df[t] = commit_df.get(t, 0) + 1

    annotated = test_annotations()

    rows = [
        analyze(sha.strip(), subj.strip(), corpus, test_df, commit_df, annotated)
        for sha, subj in shas
    ]

    counts = {k: 0 for k in TIER_DOC}
    for r in rows:
        counts[r["tier"]] += 1

    total = len(rows)
    regression = counts["SELF_TEST"] + counts["CORROBORATED"]

    if args.json:
        json.dump(
            {
                "total": total,
                "counts": counts,
                "regression_protected": regression,
                "regression_rate": round(regression / total * 100, 1) if total else 0.0,
                "invariants": rows,
            },
            sys.stdout,
            ensure_ascii=False,
            indent=2,
        )
        print()
        return 1 if (args.fail_on_none and counts["NONE"]) else 0

    print("# 链③ 不变量保护审计\n")
    print(f"来源：`fix:` 提交 **{total}** 条（每条 = 一次真实故障 = 一条不变量）\n")
    print("| 档位 | 条数 | 含义 |")
    print("|---|---|---|")
    for k, doc in TIER_DOC.items():
        print(f"| {k} | {counts[k]} | {doc} |")
    print(
        f"\n**回归保护率 = {regression}/{total} = "
        f"{regression / total * 100:.1f}%**\n"
    )
    print(
        "> 注意：回归保护 ≠ 正确性。同源测试（写在同一条 fix 提交里的断言）\n"
        "> 能挡住代码被改坏，挡不住**当初那个理解本身就是错的**——\n"
        "> `stop_reason` 大小写与 footer 的 3 份失真副本都是这样活下来的。\n"
        "> 对照验证只能来自链②（真 Pi 录制），不来自更多自写测试。\n"
    )

    for tier in ("NONE", "WEAK", "ANNOTATED"):
        sel = [r for r in rows if r["tier"] == tier]
        if not sel:
            continue
        print(f"\n## {tier}（{len(sel)} 条）\n")
        for r in sel:
            crates = sorted(
                {p.split("/")[1] for p in r["touched"] if p.startswith("crates/")}
            ) or ["-"]
            extra = ""
            if r["hits"]:
                extra = " ← " + ", ".join(f"{h['test']}" for h in r["hits"][:2])
            print(f"- `{r['sha']}` [{'/'.join(crates)}] {r['subject']}{extra}")

    return 1 if (args.fail_on_none and counts["NONE"]) else 0


if __name__ == "__main__":
    raise SystemExit(main())
