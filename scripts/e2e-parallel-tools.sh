#!/usr/bin/env bash
# Oracle：同一份剧本，nini 与真 Pi 0.85.1 的**工具并发峰值**必须相等，
# 且都等于这一轮的 K。
#
# ## 这条门禁在钉住什么
#
# nini 曾经给一次 run 的工具并发设了上限：默认 8，一轮 20 个工具调用分
# 3 批跑（8+8+4）。Pi 0.85.1 没有这个上限——它的并行分支是无上限的
# `Promise.all`（`references/pi-0.85.1/src/loop/agent-loop.ts:547-549`）。
#
# 「都跑完了」分不出「8 个同时跑」和「20 个同时跑」，所以两边各自报出
# **in-flight 峰值**（工具体内数自己有多少个 body 同时在跑），比的是数字。
#
# 判据是三条同时成立：**nini peak == Pi peak == K**。只要求「两个相等」太弱：
# 8 与 20 互不相等，但「两个都等于 1」也相等——那说明两边都退化了，oracle
# 假绿。所以 K 也进判据。
#
# 剧本两侧完全一样：K=20 个 tool_call，每个工具睡 60ms。
# **peak 与 sleep 的长短无关**：nini 侧全部 future 在 `FuturesUnordered` 的
# 同一次 poll 里各被首次 poll，每个 future 在第一个 await 之前就把 in-flight
# 计数加一；Pi 侧 `Promise.all` 在同一个同步 map 里调用每个 `tool.execute`。
# 两侧的工具都是**进程内**的（不是 bash，没有 fork 开销）。sleep 只用来
# 保证 in-flight 窗口不塌。
#
# ## 没有 Pi 的时候
#
# 差分没有做，就**说没有做**：退出码 2 + 明确文案（与
# `scripts/e2e-models-json-provider.sh` 同约定）。静默跳过等于把这道门禁变成
# 空话。

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# 两侧同一剧本：K 个调用，每个工具睡同样的时长。
K="${K:-20}"
SLEEP_MS="${SLEEP_MS:-60}"

# Pi 包的取法：**环境变量 PI_ROOT → 命令行 --pi-root → 本机 nvm 默认路径**。
PI_ROOT_ARG=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --pi-root)
      PI_ROOT_ARG="$2"
      shift 2
      ;;
    *)
      echo "!! 未知参数：$1" >&2
      exit 2
      ;;
  esac
done
export PI_ROOT="${PI_ROOT:-${PI_ROOT_ARG:-/home/jin/.nvm/versions/node/v25.9.0/lib/node_modules/@earendil-works/pi-coding-agent}}"

fail() {
  echo "!! $*" >&2
  exit 1
}

note() { echo "-- $*"; }

command -v node >/dev/null 2>&1 || {
  echo "!! 环境里没有 node：差分 oracle **没有做**（这不是失败，是没做）。" >&2
  exit 2
}
[[ -d "$PI_ROOT" ]] || {
  echo "!! 找不到 Pi 0.85.1（PI_ROOT=$PI_ROOT）：差分 oracle **没有做**（这不是失败，是没做）。" >&2
  echo "!! 装上 Pi（npm i -g @earendil-works/pi-coding-agent@0.85.1）或设 PI_ROOT=/path/to/pi 再跑。" >&2
  exit 2
}

# ---------------------------------------------------------------------------
# Pi 侧：node 探针。
# ---------------------------------------------------------------------------
note "Pi 侧：$K 个工具调用，每个睡 ${SLEEP_MS}ms"
PI_OUT="$(node "$REPO/scripts/pi-parallel-tools-probe.mjs" --calls "$K" --sleep-ms "$SLEEP_MS")" || {
  echo "$PI_OUT" >&2
  fail "Pi 侧探针没跑成"
}
echo "$PI_OUT"
PI_PEAK="$(printf '%s\n' "$PI_OUT" | grep -oE 'PI_PEAK=[0-9]+' | tail -1 | cut -d= -f2)"
[[ -n "$PI_PEAK" ]] || fail "Pi 侧探针没有报出 PI_PEAK=（输出：$PI_OUT）"

# ---------------------------------------------------------------------------
# nini 侧：Rust 侧的原子计数器（`crates/nini-core/tests/agent_integration.rs`
# 的 `ConcurrencyProbeTool`）。测试自己打印 `NINI_PEAK=`。
#
# `--nocapture` 会把 `test <name> ... ` 打在 `NINI_PEAK=` 同一行，所以用
# `grep -oE` 抓，不用 `^NINI_PEAK=`。
# ---------------------------------------------------------------------------
note "nini 侧：$K 个工具调用，每个睡 ${SLEEP_MS}ms"
NINI_OUT="$(cd "$REPO" && cargo test -q -p nini-core --test agent_integration \
  the_default_batch_runs_all_calls_at_once -- --nocapture 2>&1)" || NINI_CODE=1
NINI_CODE="${NINI_CODE:-0}"
echo "$NINI_OUT" | grep -oE 'NINI_PEAK=[0-9]+' || true
NINI_PEAK="$(printf '%s\n' "$NINI_OUT" | grep -oE 'NINI_PEAK=[0-9]+' | tail -1 | cut -d= -f2)"
# 断言失败也要先把 peak 读出来：变异验证靠的就是这两个数字的差
# （比如把默认值改回 8 → nini peak=8 ≠ Pi peak=20）。
if [[ "$NINI_CODE" != "0" && -z "$NINI_PEAK" ]]; then
  echo "$NINI_OUT" >&2
  fail "nini 侧的并发峰值测试没过，也没报出 NINI_PEAK="
fi

# ---------------------------------------------------------------------------
# 判据：两个数相等，**且都等于 K**。
# ---------------------------------------------------------------------------
note "峰值：nini=$NINI_PEAK Pi=$PI_PEAK K=$K"
[[ "$NINI_PEAK" == "$PI_PEAK" ]] || \
  fail "nini peak=$NINI_PEAK ≠ Pi peak=$PI_PEAK —— 两边的工具并发度不一样"
[[ "$NINI_PEAK" == "$K" ]] || \
  fail "nini peak=$NINI_PEAK ≠ K=$K —— 这一轮没有把全部工具调用一起跑起来"
[[ "$PI_PEAK" == "$K" ]] || \
  fail "Pi peak=$PI_PEAK ≠ K=$K —— Pi 侧没有把全部工具调用一起跑起来"

echo "OK：两边的工具并发峰值相等，且都等于 K=$K（NINI_PEAK=$NINI_PEAK PI_PEAK=$PI_PEAK）"
