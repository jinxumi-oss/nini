#!/usr/bin/env bash
# 交付门禁：一个脚本，一个退出码。任何 agent（人或模型）跑它得到同一个判定。
#
# # 为什么要有基线
#
# 只检查"测试是否全绿"是不够的：nini 的 1428 个绿测试里，90 条历史缺陷有
# 18 条至今没有任何断言保护（scripts/invariants.py 实测）。全绿与正确不是
# 同一件事。所以门禁锁定的是**基线不许退步**：
#
#   * 裸奔不变量数不得增加（修了 bug 却没加断言 = 门禁红）
#   * 测试数不得减少（删测试让它变绿 = 门禁红）
#
# 趋势由修复推动，而不是由遗忘推动。
#
# # 用法
#
#   bash scripts/gate.sh              # 快门禁，约 1 分钟
#   bash scripts/gate.sh --full       # 加 release profile 与录屏对照，约 5 分钟
#   bash scripts/gate.sh --update-baseline   # 确认修复有效后刷新基线
#
# # 不许做的事
#
# 本脚本本身、scripts/invariants.py 的判定逻辑、references/spec-v0.85.1/、
# 以及任何已有 fixture，都不许为了让门禁变绿而修改。门禁红了只有两条路：
# 修代码，或在证据充分时显式刷新基线。
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

BASELINE="$REPO/scripts/gate-baseline.json"
FULL=0
UPDATE=0
for arg in "$@"; do
  case "$arg" in
    --full) FULL=1 ;;
    --update-baseline) UPDATE=1 ;;
    *) echo "未知参数 $arg"; exit 2 ;;
  esac
done

PASS=0; FAIL=0; SKIP=0
FAILED_NAMES=()

ok()   { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); FAILED_NAMES+=("$1"); printf '  \033[31m✗\033[0m %s\n' "$1"; [[ $# -gt 1 ]] && printf '      %s\n' "$2"; }
skip() { SKIP=$((SKIP+1)); printf '  \033[33m–\033[0m %s（跳过）\n' "$1"; }
head2() { printf '\n\033[1m%s\033[0m\n' "$1"; }

# --- 0. 基线 ---------------------------------------------------------------
head2 "基线"
if [[ $UPDATE -eq 1 ]]; then
  echo "  （--update-baseline：由调用者确认后手工刷新）"
  skip "基线刷新"
else
  [[ -f "$BASELINE" ]] || { echo "缺少 $BASELINE"; exit 2; }
fi

# --- 0b. 工具链在不在 -------------------------------------------------------
# 没有 cargo 时，后面每一步都会失败，然后报出一堆与代码无关的红。门禁的
# 红必须是关于代码的红。
if ! command -v cargo >/dev/null 2>&1; then
  printf 'cargo: command not found\n' >&2
  printf '门禁的红必须是关于代码的红 —— 先把工具链加进 PATH：\n' >&2
  printf '  export PATH="$HOME/.cargo/bin:$PATH"\n' >&2
  exit 2
fi

# --- 1. 格式与 lint --------------------------------------------------------
head2 "格式与 lint"
if cargo fmt --all --check >/dev/null 2>&1; then ok "cargo fmt"; else bad "cargo fmt" "跑 cargo fmt --all 后重试"; fi

CLIPPY_OUT="$(cargo clippy --workspace --all-targets -- -D warnings 2>&1)"
if [[ $? -eq 0 ]]; then
  ok "cargo clippy -D warnings"
else
  bad "cargo clippy" "$(printf '%s' "$CLIPPY_OUT" | grep -E '^error' | head -3)"
fi

# --- 2. 构建（两个 profile）------------------------------------------------
head2 "构建"
if cargo build -q 2>/dev/null; then ok "build (debug)"; else bad "build (debug)"; fi
if cargo build -q --release 2>/dev/null; then ok "build (release)"; else bad "build (release)"; fi

# 身份必须能自报来源（8c8a009d：CI 产物曾自称 unknown）
if [[ $FULL -eq 1 ]]; then
  VER="$(./target/release/nini --version 2>&1 | head -1)"
  # 不能 grep "commit"：`main.rs` 的格式是 `{commit} ({when}) [{profile}]`，
  # 输出里从不出现该单词，那样这条检查恒红（终审实测）。
  if printf '%s' "$VER" | grep -qE 'nini [0-9]+\.[0-9]+\.[0-9]+ - [0-9a-f]{7}'; then
    ok "版本自报身份"
  else
    bad "版本自报身份" "release 二进制 --version: $VER"
  fi
else
  skip "版本自报身份（--full）"
fi

# --- 3. 测试 ---------------------------------------------------------------
head2 "测试"
T0=$SECONDS
if cargo test --workspace -q >/tmp/gate-test.log 2>&1; then
  ok "cargo test --workspace（debug, $((SECONDS-T0))s）"
else
  bad "cargo test --workspace (debug)" "$(grep -E '^test .*FAILED|panicked' /tmp/gate-test.log | head -3)"
fi

if [[ $FULL -eq 1 ]]; then
  T0=$SECONDS
  if cargo test --workspace --release -q >/tmp/gate-test-rel.log 2>&1; then
    ok "cargo test --workspace (release, $((SECONDS-T0))s)"
  else
    bad "cargo test --workspace (release)" "$(grep -E 'FAILED|panicked' /tmp/gate-test-rel.log | head -3)"
  fi
else
  skip "release profile 测试（--full；GOALS 记过 abort 语义只有它能验）"
fi

# --- 4. 链③ 基线：裸奔不变量不许增加 --------------------------------------
head2 "链③ 不变量基线"
# 每条 `//! invariant: <sha>` 是人工担保。本检查要求它在
# docs/invariant-verification.md 里有对应的变异验证结论（注入什么、哪个断言红了）。
# 写一行注释骗不过这一条；编造的记录会在独立审计里露馅。
VERIF="docs/invariant-verification.md"
if [[ -f "$VERIF" ]]; then
  MISSING_VERIF="$(python3 - "$VERIF" <<'PYEOF'
import json, re, subprocess, sys
verif = open(sys.argv[1], encoding="utf-8").read()
out = subprocess.run(["python3", "scripts/invariants.py", "--json"],
                     capture_output=True, text=True).stdout
d = json.loads(out)
miss = [r["sha"] for r in d["invariants"]
        if r["tier"] == "ANNOTATED" and r["sha"] not in verif]
print(" ".join(miss))
PYEOF
)"
  if [[ -z "${MISSING_VERIF// /}" ]]; then
    ok "每条不变量声明都有变异验证留痕"
  else
    bad "不变量声明缺少变异验证留痕" "$MISSING_VERIF —— 在 $VERIF 里补上：注入了什么、哪个断言红了"
  fi
else
  bad "缺少 $VERIF" "每条 invariant 声明都需要变异验证留痕"
fi
if [[ $UPDATE -eq 1 ]]; then
  skip "不变量基线（--update-baseline）"
else
  BASE_NONE="$(python3 -c "import json;print(json.load(open('$BASELINE'))['invariants_none'])" 2>/dev/null)"
  NOW="$(python3 scripts/invariants.py --json 2>/dev/null | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d["counts"]["NONE"])')"
  if [[ -n "$BASE_NONE" && -n "$NOW" ]]; then
    if (( NOW <= BASE_NONE )); then
      ok "裸奔不变量 $NOW ≤ 基线 $BASE_NONE"
    else
      bad "裸奔不变量回退" "现在 $NOW 条，基线 $BASE_NONE 条——有 fix 提交没有对应断言。跑 python3 scripts/invariants.py 看清单"
    fi
  else
    bad "不变量基线无法读取" "invariants.py 或 $BASELINE 有问题"
  fi
fi

# --- 5. 无死代码 -----------------------------------------------------------
head2 "静态"
# sweep-unwired 的 42 个 site 是**审计清单**，不是错误清单——它列的是
# “签名承诺了什么却什么都不消费”的位置。把 0 当门槛会让门禁永远红，
# 所以锁基线：新增未接线签名 = 门禁红，减少则无感。
if [[ -f scripts/sweep-unwired.py ]]; then
  UNWIRED="$(python3 scripts/sweep-unwired.py 2>/dev/null | grep -oE '\(([0-9]+) sites' | grep -oE '[0-9]+' | paste -sd+ | bc 2>/dev/null)"
  BASE_UNWIRED="$(python3 -c "import json;print(json.load(open('$BASELINE')).get('sweep_unwired_sites',0))" 2>/dev/null)"
  if [[ -n "$UNWIRED" && -n "$BASE_UNWIRED" ]] && (( UNWIRED <= BASE_UNWIRED )); then
    ok "未接线签名 $UNWIRED ≤ 基线 $BASE_UNWIRED"
  else
    bad "未接线签名回退" "现在 $UNWIRED，基线 $BASE_UNWIRED——新增了“有签名无调用”的位置"
  fi
else
  skip "sweep-unwired（脚本不存在）"
fi

# --- 6. 对外声明与代码一致（T4）-------------------------------------------
# 必须**精确相等**，不是 ≤。GOALS §0 记过一个真实缺陷：“README 徽章
# 1360 测试，实际 1395”——低报同样是失真，用户按徽章判断项目状态就会错。
head2 "对外声明（T4）"
README_TESTS="$(grep -oE 'tests-[0-9]+' README.md | head -1 | cut -d- -f2 || echo "")"
ACTUAL="$(cargo test --workspace -q 2>/dev/null | grep -oE 'test result: ok\. [0-9]+' | grep -oE '[0-9]+' | paste -sd+ | bc 2>/dev/null || echo "")"
if [[ -n "$README_TESTS" && -n "$ACTUAL" ]]; then
  if [[ "$README_TESTS" == "$ACTUAL" ]]; then
    ok "README 测试数 = 实测 $ACTUAL"
  else
    bad "README 测试数与实测不符" "README 徽章写 $README_TESTS，实测 $ACTUAL（低报也是失真）"
  fi
else
  bad "README 测试数无法解析" "解析不出数字等于没有声明，而没有声明同样是对外声明失真"
fi

# README 里不止一处声明测试数：徽章一个，正文一个。徽章被检查了，正文那句
# 曾经写着 1436（徽章 1488）而门禁照样绿——漏检的那一处恰恰是用户先读到的地方。
# 两处必须同时等于实测。
README_BODY_TESTS="$(grep -oE '[0-9]+ unit \+ integration tests' README.md | head -1 | grep -oE '[0-9]+' || echo "")"
if [[ -n "$README_BODY_TESTS" && -n "$ACTUAL" ]]; then
  if [[ "$README_BODY_TESTS" == "$ACTUAL" ]]; then
    ok "README 正文测试数 = 实测 $ACTUAL"
  else
    bad "README 正文测试数与实测不符" "README 正文写 $README_BODY_TESTS，实测 $ACTUAL（正文与徽章互相矛盾也是失真）"
  fi
else
  bad "README 正文测试数无法解析" "正文那句 'N unit + integration tests' 解析不出来，等于没有声明"
fi

# CHANGELOG 同样是对外声明，同样必须精确相等。它和 README 的区别只在于
# 数字出现在多处（顶部段落一句，其余是历史段落里的旧数与 badge），所以只取
# **第一个 ## 段落**里的那一个——历史段落写的是当时的事实，不许被改写。
CHANGELOG_TESTS="$(awk '/^## /{n++} n<2' CHANGELOG.md | grep -oE '[0-9]+ tests' | head -1 | grep -oE '[0-9]+' || echo "")"
if [[ -n "$CHANGELOG_TESTS" && -n "$ACTUAL" ]]; then
  if [[ "$CHANGELOG_TESTS" == "$ACTUAL" ]]; then
    ok "CHANGELOG 测试数 = 实测 $ACTUAL"
  else
    bad "CHANGELOG 测试数与实测不符" "CHANGELOG 顶部段落写 $CHANGELOG_TESTS，实测 $ACTUAL（低报也是失真）"
  fi
else
  bad "CHANGELOG 测试数无法解析" "把数字整句删掉不该让这项检查消失"
fi

# --- 7. 录屏对照（慢，需 tmux + 真 Pi）------------------------------------
head2 "链② 对照"
if [[ $FULL -eq 1 ]]; then
  if python3 scripts/export-oracle.py >/tmp/gate-export.log 2>&1; then
    ok "export-oracle（内容保全）"
  else
    bad "export-oracle" "$(grep -E '\*\*丢\*\*|丢失内容' /tmp/gate-export.log | head -3)"
  fi
  if python3 scripts/tui-oracle.py >/dev/null 2>&1; then
    ok "tui-oracle"
  else
    skip "tui-oracle（提取器已知噪声大，见 docs/invariant-audit.md §5.4，暂不进 gate）"
  fi
else
  skip "export-oracle（--full）"
  skip "tui-oracle（提取器未达标，不进 gate）"
fi

# --- 汇总 ------------------------------------------------------------------
head2 "汇总"
printf '  通过 %d   失败 %d   跳过 %d\n' "$PASS" "$FAIL" "$SKIP"
if (( FAIL > 0 )); then
  printf '\n\033[31m门禁未通过\033[0m：%s\n' "${FAILED_NAMES[*]}"
  echo "修代码或补断言。**不要**改门禁、spec、fixture 让它变绿。"
  exit 1
fi
printf '\n\033[32m门禁通过\033[0m\n'
