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
# 判据是这些同时成立：
#   * nini peak == Pi peak == K；
#   * **Pi 侧自己的两个证据也一致**：工具体内的计数器报的高水位，与探针
#     拿每个调用的开始/结束时间算出的最大重叠区间数，必须相等。
#
# 只要求「两个相等」太弱：8 与 20 互不相等，但「两个都等于 1」也相等——
# 那说明两边都退化了，oracle 假绿。所以 K 也进判据。
#
# 只信 Pi 侧**自报**的数字也太弱：如果探针测错了却写死 `PI_PEAK=20`，
# oracle 照样绿。所以 Pi 侧给两个**独立**的证据（计数器 vs 时间戳），两者
# 一致才算数。
#
# ## 这个门禁**看不见**的东西（盲区，写在前面，免得读者以为它无懈可击）
#
# Pi 侧的两个证据住在**同一个 `.mjs` 文件**里（`scripts/pi-parallel-tools-
# probe.mjs`）。它们各自独立地测量，可它们**住在同一处代码里**：把探针改成
# 串行、同时把 `PI_PEAK=` 与 `PI_OVERLAP_PEAK=` 两行都写死成 20，oracle 照样
# 绿（已实测：退出码 0）。换句话说：「两个证据一致」证明的是**两次测量没
# 互相矛盾**，不是「有人没法同时骗过它们」。
#
# 要消灭这个盲区，只能写**第二个独立实现**（另一门语言、另一份代码、另一条
# 可观测路径），成本与收益不成比例——所以本轮**只把它写在这里**，而不是假装
# 它不存在。
#
# 能咬住的是**单侧**造假：只改一个证据、另一个仍然实测，两个就对不上 →
# oracle 变红（已实测：改计数器 → 红；改区间算法 → 红）。
#
# 剧本两侧完全一样：K 个 tool_call，每个工具睡 60ms。
# **peak 与 sleep 的长短无关**：nini 侧全部 future 在 `FuturesUnordered` 的
# 同一次 poll 里各被首次 poll，每个 future 在第一个 await 之前就把 in-flight
# 计数加一；Pi 侧 `Promise.all` 在同一个同步 map 里调用每个 `tool.execute`。
# 两侧的工具都是**进程内**的（不是 bash，没有 fork 开销）。sleep 只用来
# 保证 in-flight 窗口不塌。
#
# K 只有一个来源：脚本的 `K`（默认 20）。它同时喂给两侧——Pi 侧探针的
# `--calls`，nini 侧 Rust 测试的环境变量 `NINI_PARALLEL_CALLS`。两侧各写死
# 一个数的话，改了一处就会得到「nini peak=20 ≠ Pi peak=30」这种读起来像
# 并发 bug、其实只是剧本漂移的报错。
#
# ## 没有 Pi 的时候
#
# 差分没有做，就**说没有做**：退出码 2 + 明确文案（与
# `scripts/e2e-models-json-provider.sh` 同约定）。静默跳过等于把这道门禁变成
# 空话。同理：没有 node、没有 cargo、Pi 装坏了（探针自己说环境不可用），
# 一律是 2 + 「没有做」。

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

# 「没做」与「做坏了」是两回事：前者退出 2，后者退出 1。
not_done() {
  echo "!! $*" >&2
  echo "!! 差分 oracle **没有做**（这不是失败，是没做）。" >&2
  exit 2
}

note() { echo "-- $*"; }

command -v node >/dev/null 2>&1 || not_done "环境里没有 node。"
[[ -d "$PI_ROOT" ]] || not_done "找不到 Pi 0.85.1（PI_ROOT=$PI_ROOT）。装上 Pi（npm i -g @earendil-works/pi-coding-agent@0.85.1）或设 PI_ROOT=/path/to/pi 再跑。"
command -v cargo >/dev/null 2>&1 || not_done "环境里没有 cargo。"

# ---------------------------------------------------------------------------
# Pi 侧：node 探针。
# ---------------------------------------------------------------------------
note "Pi 侧：$K 个工具调用，每个睡 ${SLEEP_MS}ms"
PI_OUT="$(node "$REPO/scripts/pi-parallel-tools-probe.mjs" --calls "$K" --sleep-ms "$SLEEP_MS" 2>&1)" && PI_CODE=0 || PI_CODE=$?
# 探针自己说「环境不可用」时，差分是**没做**，不是失败。
if [[ "$PI_CODE" == "2" ]]; then
  echo "$PI_OUT" >&2
  not_done "Pi 侧探针说环境不可用（探针的退出码是 2）。"
fi
if [[ "$PI_CODE" != "0" ]]; then
  echo "$PI_OUT" >&2
  fail "Pi 侧探针没跑成（退出码 $PI_CODE）。"
fi
echo "$PI_OUT"
PI_PEAK="$(printf '%s\n' "$PI_OUT" | grep -oE 'PI_PEAK=[0-9]+' | tail -1 | cut -d= -f2 || true)"
PI_OVERLAP="$(printf '%s\n' "$PI_OUT" | grep -oE 'PI_OVERLAP_PEAK=[0-9]+' | tail -1 | cut -d= -f2 || true)"
[[ -n "$PI_PEAK" ]] || fail "Pi 侧探针没有报出 PI_PEAK=（输出：$PI_OUT）"
[[ -n "$PI_OVERLAP" ]] || fail "Pi 侧探针没有报出 PI_OVERLAP_PEAK=（输出：$PI_OUT）"

# ---------------------------------------------------------------------------
# nini 侧：Rust 侧的原子计数器（`crates/nini-core/tests/agent_integration.rs`
# 的 `ConcurrencyProbeTool`）。测试自己打印 `NINI_PEAK=`。
#
# K 从环境变量进 Rust 侧（`NINI_PARALLEL_CALLS`）——两侧同一个来源。
#
# `--nocapture` 会把 `test <name> ... ` 打在 `NINI_PEAK=` 同一行，所以用
# `grep -oE` 抓，不用 `^NINI_PEAK=`。
# ---------------------------------------------------------------------------
note "nini 侧：$K 个工具调用，每个睡 ${SLEEP_MS}ms"
NINI_OUT="$(cd "$REPO" && NINI_PARALLEL_CALLS="$K" cargo test -q -p nini-core --test agent_integration \
  the_default_batch_runs_all_calls_at_once -- --nocapture 2>&1)" && NINI_CODE=0 || NINI_CODE=$?
echo "$NINI_OUT" | grep -oE 'NINI_(CALLS|PEAK)=[0-9]+' || true
NINI_PEAK="$(printf '%s\n' "$NINI_OUT" | grep -oE 'NINI_PEAK=[0-9]+' | tail -1 | cut -d= -f2 || true)"
NINI_CALLS="$(printf '%s\n' "$NINI_OUT" | grep -oE 'NINI_CALLS=[0-9]+' | tail -1 | cut -d= -f2 || true)"
# 断言失败也要先把 peak 读出来：变异验证靠的就是这两个数字的差
# （比如把默认值改回 8 → nini peak=8 ≠ Pi peak=20）。
if [[ "$NINI_CODE" != "0" && -z "$NINI_PEAK" ]]; then
  echo "$NINI_OUT" >&2
  fail "nini 侧的并发峰值测试没过，也没报出 NINI_PEAK="
fi
[[ -n "$NINI_PEAK" ]] || {
  echo "$NINI_OUT" >&2
  fail "nini 侧没有报出 NINI_PEAK=（测试改名了，或它没跑起来）。"
}
[[ -n "$NINI_CALLS" ]] || {
  echo "$NINI_OUT" >&2
  fail "nini 侧没有报出 NINI_CALLS=（测试改名了，或它没跑起来）。"
}
# 两侧的剧本必须真的是同一个。
[[ "$NINI_CALLS" == "$K" ]] || \
  fail "两侧的剧本不是同一个：nini 侧跑了 $NINI_CALLS 个调用，脚本要的是 $K —— K 必须两侧同源"

# ---------------------------------------------------------------------------
# 判据：两个数相等，**且都等于 K**；Pi 侧自己的两个证据也要相等。
# ---------------------------------------------------------------------------
note "峰值：nini=$NINI_PEAK Pi=$PI_PEAK Pi重叠=$PI_OVERLAP K=$K"
[[ "$PI_PEAK" == "$PI_OVERLAP" ]] || \
  fail "Pi 侧的两个证据不一致：自报峰值 $PI_PEAK ≠ 重叠区间数 $PI_OVERLAP —— 探针的测量不可信"
[[ "$NINI_PEAK" == "$PI_PEAK" ]] || \
  fail "nini peak=$NINI_PEAK ≠ Pi peak=$PI_PEAK —— 两边的工具并发度不一样"
[[ "$NINI_PEAK" == "$K" ]] || \
  fail "nini peak=$NINI_PEAK ≠ K=$K —— 这一轮没有把全部工具调用一起跑起来"
[[ "$PI_PEAK" == "$K" ]] || \
  fail "Pi peak=$PI_PEAK ≠ K=$K —— Pi 侧没有把全部工具调用一起跑起来"

echo "OK：两边的工具并发峰值相等，且都等于 K=$K（NINI_PEAK=$NINI_PEAK PI_PEAK=$PI_PEAK PI_OVERLAP_PEAK=$PI_OVERLAP）"
