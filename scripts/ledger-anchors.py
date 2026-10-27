#!/usr/bin/env python3
"""台账锚点检查：每条 gap 的 `nini_state` 必须指向 crates/ 里**真实存在**的符号。

为什么需要它
------------
差距 C2 的病根是「计划文档与代码失真：文档里的状态字段不能指向代码符号」。
台账（scripts/parity-gaps.json）里的 `nini_state` 是人写的，它写下的
`crates/nini-tui/src/commands/dispatch.rs:805` 会在下一次改动后悄悄漂移，
而**没有任何一条自动检查能发现这件事**：

  * `parity-gap.py --check-plan` 只比 gap id 与计数；
  * `plan-selfcheck.py` 的 S5/S7 只看字段非空；
  * `ledger-freshness.py` 的 F2/F3 只看 `oracle` 字段里的文件还在不在。

本脚本补上这一层：**台账说的每个代码锚点，都必须真的存在**。

规则
----
对每条 gap 的 `nini_state`：

  A. 路径锚点：抽出所有 `crates/…/x.rs`（含 .jsonl/.json/.toml 等）路径。
     文件不存在 → **failure**。

  B. 符号锚点：符号名从**反引号**里抽：`` `identifier` ``。
     抽出的标识符要「像 Rust 符号」才算数（见下面的形状规则），
     并且必须能在**它所配的那个文件**里 grep 得到 → 否则 **failure**。

  C. 配对：一个符号配给**紧挨着它的那个文件引用**。「紧挨着」的意思是：
     两者之间只有连接词（的/与/和/、/：/括号/逗号/反引号里的别的符号），
     而且不超过 60 个字符。所以这两种写法都能配上：

         `crates/nini-tui/src/state.rs` 的 `RunState::pending_steer_messages`
         `RunState::pending_followup_messages`（`crates/nini-tui/src/state.rs`）

     配不上的符号（散文里提到、但没指文件）→ 去**整个 crates/ 树**里找；
     整个树里都找不到 → **failure**（它多半是写错了或已经删了）。

  D. 行号漂移：路径写成 `file.rs:123` 时，检查该符号是否出现在 ±40 行内。
     不在 → **warning**（行号会随代码变动，不该因此判死），但符号整体
     不存在仍然是 failure。

  E. 每条 gap 至少要有**一个能解析的锚点**（文件存在 + 符号在该文件里
     grep 得到），否则 **failure**（纯散文的 `nini_state` 不算）。

「像 Rust 符号」的形状规则（以及**排除**清单）
---------------------------------------------
从反引号内容里用 `[A-Za-z_][A-Za-z0-9_]*` 切出标识符后，只保留这三类：

  * 大驼峰 `UpperCamelCase`：`EscapeTarget`、`TrustSelector`、`LsTool`
  * 全大写 `SCREAMING_SNAKE`：`CARGO_PKG_VERSION`、`APP_TITLE`
  * 含下划线的 snake_case：`cmd_export`、`submit_user_input_inner`

**不**保留：

  * 单个的小写单词（`bool`、`copy`、`abort`）——它们是英文词，不是符号；
  * 小驼峰 `lowerCamelCase`（`isBashMode`）——那是 Pi 侧 JS 的写法，
    nini 侧不存在同名符号，拿它去 grep 只会制造假警报；
  * 专有名词与缩写白名单：`Pi`、`LLM`、`JSON`、`YAML`、`TOML`、`TUI`、
    `CLI`、`MCP`、`OSC`、`ANSI`、`HTTP`、`API`、`URL`、`URI`、`README`、
    `LICENSE`、`AGENTS`、`CLAUDE`、`TTY`、`UTF`、`SSD`、`GPU`、`CPU`、
    `RAM`、`SSH`、`IDE`、`SDK`、`SQL`、`HTML`、`XML`、`MIT`、`BSD`、
    `GPT`、`Windows`、`Linux`、`macOS`、`M2b` 等。

诚实地说，这个抽取器**抓不到**的东西：

  * 符号与路径之间**夹了中文动词**（「`sym` 记在 `path` 里」）——中间不是
    连接词，配不上对；这种符号会退到「整个 crates/ 树里找」那条判据上。
  * 符号**存在但语义已经变了**（函数还在、行为改了）——本脚本只做文本
    层面的存在性检查，抓不到语义漂移；那要靠 oracle 侧的测试。
  * 一个符号被**重命名**后，旧名字在别处（注释、别的文件）还留着
    ——文件内 grep 命中即算过；本脚本不做跨文件的唯一性检查。
  * 裸文件名（`runtime.rs` 而不是 `crates/nini-tui/src/runtime.rs`）在
    crates/ 下同名文件不止一个时无法解析，只能跳过（会打 warning）。

用法
----
    python3 scripts/ledger-anchors.py                 # 检查真实台账
    python3 scripts/ledger-anchors.py --json          # 机读
    python3 scripts/ledger-anchors.py --self-test     # 用合成台账自证判据有效

退出码
------
    0  全部通过（warning 不影响退出码）
    1  有 failure
    2  台账文件本身读不出来
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

# ── 文本抽取的正则 ────────────────────────────────────────────────────────

# 仓库相对路径锚点：crates/ 开头，带一个已知扩展名。
# 扩展名按长度优先（jsonl 必须排在 json 前面）。
PATH_RE = re.compile(
    r"(?<![\w./-])"
    r"(?P<path>crates/"
    r"[A-Za-z0-9_.\-/]*?"
    r"\.(?:jsonl|json|toml|md|mjs|py|sh|rs))"
    r"(?::(?P<line>\d+))?"
)

# Rust 源文件引用：crates/…/x.rs 或裸 x.rs。
# 裸文件名只用来**配对**（「`runtime.rs` 的 `QueueKind`」里的 runtime.rs
# 是哪个文件），解析时按 basename 在 crates/ 下唯一匹配。
RS_TOKEN_RE = re.compile(
    r"(?<![\w./-])"
    r"(?P<token>[A-Za-z0-9_.\-/]*?\.rs)"
    r"(?::(?P<line>\d+))?"
)

# 反引号片段
TICK_RE = re.compile(r"`([^`\n]+)`")

# 反引号里包着的**文件路径**（不是符号）
PATH_LIKE_RE = re.compile(r"^(?:[\w./-]+/)*[\w.-]+\.[A-Za-z0-9]+$")

# 标识符
IDENT_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")

# 形状
UPPER_CAMEL_RE = re.compile(r"^[A-Z][a-z0-9]+(?:[A-Z][a-z0-9]*)*$")
SCREAMING_RE = re.compile(r"^[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)*$")
SNAKE_RE = re.compile(r"^[a-z][a-z0-9]*(?:_[a-z0-9]+)+$")

# 专有名词 / 缩写：不是 nini 代码里的符号，排除。
STOPWORDS = {
    "AGENTS", "ANSI", "API", "ASCII", "BSD", "CD", "CI", "CLAUDE", "CPU",
    "CSS", "DLL", "EOF", "GPU", "GPT", "HTML", "HTTP", "HTTPS", "IDE", "IO",
    "JSON", "Known", "LICENSE", "LLM", "M1", "M2b", "MCP", "MIT", "MVC",
    "NPM", "OS", "OSC", "Pi", "PR", "RAM", "README", "RUST", "Rust", "SDK",
    "SQL", "SSH", "SSD", "TCP", "TOML", "TUI", "TTY", "UDP", "URI", "URL",
    "UTF", "VIM", "WASM", "XML", "YAML", "ESC",
}

# 「紧挨着」的判定：符号与文件引用之间只允许出现这些连接词/标点，
# 以及**别的**反引号片段（那是另一个符号）。
CONNECTOR_CHARS = set(" \t的与和、：:（()），,。·—–-")

# 配对窗口（字符）。
PAIR_WINDOW = 60

# 行号漂移的容差（行）。
LINE_TOLERANCE = 40

# **外部**符号：不是 nini 自己的代码，但台账里合理地会提到（ratatui /
# crossterm 的 API、Pi 侧的 JS 符号……）。这些符号在 crates/ 里当然 grep
# 不到，所以不进「全树 grep 不到 → failure」那条判据。
EXTERNAL_SYMBOLS = {
    # ratatui 的私有 API：crates/ 里当然没有（Cell / Buffer / Terminal /
    # Modifier / HYPERLINK 这些名字 nini 自己也用得到，所以只有这一个需要
    # 单独豁免）。
    "set_stringn",
}


def is_symbol_like(token: str) -> bool:
    """这个标识符像不像 Rust 代码里的符号？"""
    if len(token) < 3:
        return False
    if token in STOPWORDS:
        return False
    if SCREAMING_RE.match(token):
        return True
    if UPPER_CAMEL_RE.match(token):
        return True
    if SNAKE_RE.match(token):
        return True
    return False


def _segment_symbols(content: str) -> list[str]:
    """从一个反引号片段里抽出符号名。

    `A::B::C` 取**最后一段**（模块路径的前缀不是符号）；其余取全部
    像符号的标识符。
    """
    if "::" in content:
        last = content.rsplit("::", 1)[-1]
        content = last
    out: list[str] = []
    for tok in IDENT_RE.findall(content):
        if is_symbol_like(tok) and tok not in out:
            out.append(tok)
    return out


def collect_symbols(text: str) -> list[tuple[int, int, int, int, str, bool]]:
    """抽出文本里的符号候选。

    返回 (span_start, span_end, inner_start, inner_end, symbol, 是否显式)。

    * span_* = 符号所在的**片段**边界（显式符号 = 整个反引号片段；
      隐式符号 = 标识符本身）。
    * inner_* = 标识符在片段里的位置（算距离用）。
    两者都要：「符号在路径左边还是右边」要看片段边界，而「离哪个路径近」
    要看标识符本身。

    显式 = 写在反引号里的（`cmd_clone`）；隐式 = 裸标识符，且紧跟在
    「的 / 与 / 和 / 、 / ：」之后（弱锚点）。

    **文件路径里的名字不算符号**：`` `crates/nini-tui/tests/foo.rs` `` 里的
    `foo` 是文件名，不是代码符号。
    """
    out: list[tuple[int, int, int, int, str, bool]] = []
    path_spans = [(m.start(), m.end()) for m in PATH_RE.finditer(text)]
    path_spans += [(m.start(), m.end()) for m in RS_TOKEN_RE.finditer(text)]

    tick_spans: list[tuple[int, int]] = []
    for m in TICK_RE.finditer(text):
        tick_spans.append((m.start(), m.end()))
        base = m.start(1)
        content = m.group(1)
        # 反引号里包的是**文件路径**时，它不是符号（`` `crates/…/x.rs` ``）。
        if PATH_LIKE_RE.match(content.strip()):
            continue
        for sym in _segment_symbols(content):
            idx = content.find(sym)
            if idx < 0:
                idx = 0
            out.append((m.start(), m.end(), base + idx, base + idx + len(sym), sym, True))
    for im in IDENT_RE.finditer(text):
        s, e = im.span()
        if any(a <= s and e <= b for a, b in tick_spans):
            continue  # 已经在反引号里算过了
        if any(a <= s and e <= b for a, b in path_spans):
            continue  # 文件名，不是符号
        tok = im.group(0)
        if not is_symbol_like(tok):
            continue
        before = text[:s].rstrip()
        if not before or before[-1] not in "的和、与：:（(,":
            continue
        out.append((s, e, s, e, tok, False))
    return out


def _connector_only(text: str) -> bool:
    """这段文字里只有连接词/标点（以及别的反引号片段）吗？"""
    stripped = TICK_RE.sub("", text).replace("`", "")
    return all(ch in CONNECTOR_CHARS for ch in stripped)


def _symbol_in_file(path: Path, symbol: str) -> tuple[bool, list[int]]:
    """symbol 是否在该文件里出现？返回 (found, 命中的行号列表)。"""
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return False, []
    pattern = re.compile(r"(?<![A-Za-z0-9_])" + re.escape(symbol) + r"(?![A-Za-z0-9_])")
    lines = []
    for idx, line in enumerate(text.splitlines(), start=1):
        if pattern.search(line):
            lines.append(idx)
    return bool(lines), lines


def _basename_index(repo_root: Path) -> dict[str, list[Path]]:
    """crates/ 下所有 .rs 文件的 basename → 路径列表（解析裸文件名用）。"""
    index: dict[str, list[Path]] = {}
    crates = repo_root / "crates"
    if not crates.is_dir():
        return index
    for p in crates.rglob("*.rs"):
        index.setdefault(p.name, []).append(p)
    return index


class CrateIndex:
    """crates/ 树的符号索引（给「配不上的符号」做全树查找用）。"""

    def __init__(self, repo_root: Path) -> None:
        self.repo_root = repo_root
        self._files: list[Path] = sorted((repo_root / "crates").rglob("*.rs"))
        self._cache: dict[str, bool] = {}

    def exists_anywhere(self, symbol: str) -> bool:
        if symbol in self._cache:
            return self._cache[symbol]
        pattern = re.compile(r"(?<![A-Za-z0-9_])" + re.escape(symbol) + r"(?![A-Za-z0-9_])")
        found = False
        for f in self._files:
            try:
                if pattern.search(f.read_text(encoding="utf-8", errors="replace")):
                    found = True
                    break
            except OSError:
                continue
        self._cache[symbol] = found
        return found


def _pair(text: str, symbols, tokens) -> list[tuple[int, int, str, bool]]:
    """给每个符号配一个文件引用：紧挨着、且中间只有连接词的那一个。

    「的」是决定性的：`` `path` 的 `sym` `` 里 sym 属于**前面**的 path。
    反过来 `` `sym`（`path`）`` 里 sym 属于**后面**的 path（短括号）。
    两边都不像时，取最近的。
    """
    pairs = []
    for span_s, span_e, in_s, in_e, sym, explicit in symbols:
        # 候选分三层：①「…的 X」→ 属于**前面**的路径；
        #              ②「X（path）」→ 属于**后面**的路径；
        #              ③ 都没有 → 取最近的。
        tiers: list[list] = [[], [], []]
        for tm in tokens:
            t_start, t_end = tm.start(), tm.end()
            if span_e <= t_start:
                between = text[span_e:t_start]
                dist = t_start - in_e
                after = True   # 文件引用在符号**后面**
            elif span_s >= t_end:
                between = text[t_end:span_s]
                dist = in_s - t_end
                after = False  # 文件引用在符号**前面**
            else:
                continue  # 符号落在文件引用内部（路径自己的名字）
            if dist > PAIR_WINDOW:
                continue
            if not _connector_only(between):
                continue
            stripped = TICK_RE.sub("", between).replace("`", "").strip()
            if "的" in between:
                if after:
                    continue
                tiers[0].append((dist, tm))
            elif stripped.startswith(("（", "(")) and len(stripped) <= 8:
                if not after:
                    continue
                tiers[1].append((dist, tm))
            else:
                tiers[2].append((dist, tm))
        best = None
        for tier in tiers:
            if tier:
                best = min(tier)[1]
                break
        if best is not None:
            pairs.append((span_s, span_e, sym, explicit, best))
    return pairs


def check_gap(gap: dict, repo_root: Path, basenames: dict[str, list[Path]],
              index: CrateIndex) -> dict:
    """检查一条 gap 的 nini_state 锚点。"""
    findings: list[str] = []   # failure
    warnings: list[str] = []   # warning
    anchors: list[dict] = []

    gid = str(gap.get("id", "?"))
    text = str(gap.get("nini_state", ""))

    tokens = list(RS_TOKEN_RE.finditer(text))
    symbols = collect_symbols(text)

    # ── A. crates/ 路径必须存在 ──────────────────────────────────────
    for m in PATH_RE.finditer(text):
        rel = m.group("path")
        if not (repo_root / rel).is_file():
            findings.append(f"nini_state 指向的文件不存在：{rel}")

    # ── B/C. 配对 + 符号存在性 ───────────────────────────────────────
    paired = _pair(text, symbols, tokens)
    paired_syms = {p[2] for p in paired}
    warned_tokens: set[str] = set()

    for span_s, span_e, sym, explicit, tm in paired:
        tok_text = tm.group("token")
        line_no = int(tm.group("line")) if tm.group("line") else None
        if tok_text.startswith("crates/"):
            target = repo_root / tok_text
        else:
            candidates = basenames.get(Path(tok_text).name, [])
            if len(candidates) != 1:
                if tok_text not in warned_tokens:
                    warned_tokens.add(tok_text)
                    warnings.append(
                        f"裸文件名 `{tok_text}` 在 crates/ 下无法唯一解析"
                        f"（{len(candidates)} 个同名文件）——不当作锚点"
                    )
                continue
            target = candidates[0]
        rel = str(target.relative_to(repo_root)) if repo_root in target.parents else tok_text
        ok, lines = _symbol_in_file(target, sym)
        anchors.append({"path": rel, "symbol": sym, "explicit": explicit})
        if ok:
            if line_no is not None and not any(
                abs(line_no - ln) <= LINE_TOLERANCE for ln in lines
            ):
                warnings.append(
                    f"{rel}:{line_no} 附近（±{LINE_TOLERANCE} 行）找不到 "
                    f"`{sym}`（它在第 {','.join(str(x) for x in lines[:5])} 行）"
                    "——行号漂了"
                )
        elif explicit:
            findings.append(f"{rel} 里没有符号 `{sym}`")
        else:
            warnings.append(f"{rel} 里没有裸标识符 `{sym}`（弱锚点，不判死）")

    # ── 配不上的显式符号：整个 crates/ 树里也必须存在 ────────────────
    for span_s, span_e, in_s, in_e, sym, explicit in symbols:
        if not explicit or sym in paired_syms or sym in EXTERNAL_SYMBOLS:
            continue
        if not index.exists_anywhere(sym):
            findings.append(
                f"nini_state 里的符号 `{sym}` 在 crates/ 全树里都 grep 不到"
                "（写错了？或者它已经被删了？）"
            )

    # ── E. 至少一个能解析的锚点 ──────────────────────────────────────
    resolved = len({
        (a["path"], a["symbol"])
        for a in anchors
        if _symbol_in_file(repo_root / a["path"], a["symbol"])[0]
    })
    if not tokens:
        findings.append(
            "nini_state 里没有任何 crates/ 路径锚点（纯散文，无法机器核对）"
        )
    elif resolved == 0:
        findings.append(
            "nini_state 里没有一个能解析的锚点"
            "（文件存在 + 符号在该文件里 grep 得到）"
        )

    return {
        "id": gid,
        "ok": not findings,
        "findings": findings,
        "warnings": warnings,
        "anchors": anchors,
        "resolved": resolved,
        "detail": "；".join(findings),
    }


def check_ledger(data: dict, repo_root: Path) -> list[dict]:
    gaps = data.get("gaps")
    if not isinstance(gaps, list):
        raise ValueError("台账里没有 gaps 数组")
    basenames = _basename_index(repo_root)
    index = CrateIndex(repo_root)
    return [check_gap(g, repo_root, basenames, index) for g in gaps]


def _human(results: list[dict]) -> None:
    for r in results:
        where = f"{r['resolved']}/{len(r['anchors'])} 锚点可解析"
        if r["ok"]:
            print(f"{r['id']:<5} {where:<24} 通过")
        else:
            print(f"{r['id']:<5} {where:<24} 失败：{r['detail']}")
        for w in r["warnings"]:
            print(f"      warn  {w}")
    bad = [r for r in results if not r["ok"]]
    print()
    if bad:
        print(f"台账锚点：FAIL —— {len(bad)}/{len(results)} 条不合格")
        for r in bad:
            print(f"  - {r['id']}：{r['detail']}")
    else:
        warn_count = sum(len(r["warnings"]) for r in results)
        print(
            f"台账锚点：PASS —— {len(results)}/{len(results)} 条合格"
            + (f"（{warn_count} 条 warning）" if warn_count else "")
        )


def self_test() -> int:
    """用合成台账证明这个判据抓得到错。"""
    ok = True
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        src = root / "crates" / "nini-tui" / "src"
        src.mkdir(parents=True)
        (src / "state.rs").write_text(
            "pub struct RunState {\n    pub agent_alive: bool,\n}\n"
            "pub fn cmd_clone() {}\n",
            encoding="utf-8",
        )

        def run(state: str):
            data = {"schema": 1, "gaps": [{
                "id": "T", "status": "open", "oracle": "none",
                "exit_criterion": "x", "nini_state": state,
                "machine_checkable": True,
            }]}
            return check_ledger(data, root)[0]

        cases = [
            ("(a) 指向的文件不存在",
             "crates/nini-tui/src/gone.rs 的 `cmd_clone`", True),
            ("(b) 符号在文件里不存在",
             "crates/nini-tui/src/state.rs 的 `no_such_symbol`", True),
            ("(c) 纯散文，没有任何路径锚点",
             "已经修好了，行为符合 Pi 的语义。", True),
            ("(d) 文件存在但没有任何符号锚点",
             "crates/nini-tui/src/state.rs 里的字段都改了", True),
            ("(f) 符号连 crates/ 全树里都没有",
             "crates/nini-tui/src/state.rs 的 `totally_bogus_symbol`", True),
        ]
        for label, state, should_fail in cases:
            res = run(state)
            if should_fail and res["ok"]:
                ok = False
                print(f"self-test {label}：**没抓到** ✗")
            else:
                print(f"self-test {label}：抓到 → {res['detail']} ✓")

        # 正样本：文件 + 符号都对，必须通过
        good = run("crates/nini-tui/src/state.rs 的 `agent_alive`")
        if good["ok"]:
            print("self-test 正样本：文件 + 符号都对 → 通过 ✓")
        else:
            ok = False
            print(f"self-test 正样本：竟然失败了：{good['detail']} ✗")

        # 符号写在路径前面（中文常见的「`符号`（`路径`）」写法）也要配上
        backward = run("`agent_alive`（`crates/nini-tui/src/state.rs`）")
        if backward["ok"]:
            print("self-test 正样本：符号写在路径之前 → 通过 ✓")
        else:
            ok = False
            print(f"self-test 正样本（符号在前）：竟然失败了：{backward['detail']} ✗")

        # 行号漂移：只能 warning，不能判死
        drifted = run("crates/nini-tui/src/state.rs:999 的 `agent_alive`")
        if drifted["ok"] and drifted["warnings"]:
            print(f"self-test (e) 行号漂移 → 只 warning ✓（{drifted['warnings'][0]}）")
        else:
            ok = False
            print(f"self-test (e) 行号漂移：判错了 ✗ {drifted}")

    print()
    print("self-test：" + ("PASS —— 判据抓得到错" if ok else "FAIL —— 判据有盲区"))
    return 0 if ok else 1


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="差距台账锚点检查（nini_state ↔ 代码符号）。")
    ap.add_argument("--ledger", default=str(LEDGER), help="台账路径")
    ap.add_argument("--repo-root", default=str(REPO), help="仓库根（锚点路径相对于它）")
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
            "warnings": sum(len(r["warnings"]) for r in results),
            "gaps": results,
        }, ensure_ascii=False, indent=2))
    else:
        _human(results)

    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
