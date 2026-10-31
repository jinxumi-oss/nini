#!/usr/bin/env bash
# Oracle：批次级工具执行模式（`toolExecution`）在 nini 与真 Pi 0.85.1
# 两侧的行为必须一致。
#
# ## 这条门禁在钉住什么
#
# Pi 0.85.1 有个**批次级**开关 `toolExecution`
# （`pi-agent-core/dist/agent.js:134` 的 `runtimeOptions.toolExecution ??
# "parallel"`，判定在 `dist/agent-loop.js:288`）：
#
#   * `parallel`（默认）—— 一批工具调用一起跑；
#   * `sequential`      —— 一批工具调用**逐个**跑。
#
# nini 的 `RunConfig.tool_execution` 一直带着这个字段，Agent 也读它
# （`crates/nini-core/src/agent.rs` 的批次降级判定），但用户**没有任何办法**
# 改它：CLI 没有开关，settings 没有键。这个门禁守的是「用户能改之后，
# 改出来的行为与 Pi 一致」。
#
# 判据是三条同时成立：
#   1. **Pi 侧**：`PI_TOOL_EXECUTION=sequential` → `PI_PEAK == 1` 且
#      `PI_OVERLAP_PEAK == 1`（Pi 的两个独立证据都说「一个一个跑」）；
#   2. **nini 侧**：sequential → `NINI_SEQ_PEAK == 1`；
#   3. **对照组**：parallel 模式下**两边**的 peak 都 `== K`。
#
# 第 3 条不是凑数：没有它，「nini 永远 peak==1」也能骗过第 2 条。K 单一
# 来源（脚本的 `K`），同时喂给两侧——Pi 侧的 `--calls` 与 nini 侧的
# `NINI_PARALLEL_CALLS`——并核对 `NINI_SEQ_CALLS == K`，防止两侧剧本漂移。
#
# ## 「都跑完了」分不出并发度
#
# 一批 K 个工具调用「都跑完了」，既可能是 K 个同时跑，也可能是逐个跑了 K
# 遍。所以两侧各自报出**in-flight 峰值**（工具体内数自己有多少个 body 同时
# 在跑），比的是数字。
#
# Pi 侧的两个证据（计数器 vs 时间戳算出的最大重叠区间数）住在同一个
# `.mjs` 文件里——它们各自独立地测量，却住在同一处代码里。把探针改成串行、
# 同时把两行都写死，oracle 照样绿。**这个盲区是 `e2e-parallel-tools.sh`
# 已经写下来的**，本脚本不假装它不存在：能咬住的是**单侧**造假。
#
# ## 没有 Pi 的时候
#
# 差分没有做，就**说没有做**：退出码 2 + 明确文案（与
# `scripts/e2e-parallel-tools.sh`、`scripts/e2e-models-json-provider.sh`
# 同约定）。静默跳过等于把这道门禁变成空话。

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

# ---------------------------------------------------------------------------
# 判据清单（必须在第一次 record_assertion 之前定义）
# ---------------------------------------------------------------------------
# v0.15.0 (审查修正): 判据**必须成清单**。理由是一条真实的审查发现 ——
# 把脚本里的某条判据删掉，脚本**照样退出 0**，全仓测试也全绿：
# 判据集自己没人看守。于是「Pi 侧的断言」可以被悄悄删掉，而 Pi 侧的行为
# 就再也没人核对。
#
# 现在每条判据执行时登记自己，脚本收尾时核对「清单里的每一条都跑到了」。
# 删掉一条判据 → 清单与执行数不符 → 脚本自己红。
# 再加一条 Rust 测试（crates/nini-core/tests/oracle_criteria.rs）钉住
# 清单的**内容**，所以删判据在 cargo test 层面也会红。
ASSERTIONS=(
  "pi_mode_is_sequential"
  "pi_seq_peak_is_1"
  "pi_seq_overlap_is_1"
  "pi_mode_is_parallel"
  "pi_par_peak_equals_k"
  "pi_par_overlap_equals_k"
  "nini_mode_is_sequential"
  "nini_calls_match_k"
  "nini_seq_peak_is_1"
  "nini_mode_is_parallel"
  "nini_par_peak_equals_k"
)
declare -A ASSERTION_DONE=()
record_assertion() { ASSERTION_DONE["$1"]=1; }


command -v node >/dev/null 2>&1 || not_done "环境里没有 node。"
[[ -d "$PI_ROOT" ]] || not_done "找不到 Pi 0.85.1（PI_ROOT=$PI_ROOT）。装上 Pi（npm i -g @earendil-works/pi-coding-agent@0.85.1）或设 PI_ROOT=/path/to/pi 再跑。"
# 版本必须是 0.85.1。目录存在**不等于**版本对 —— 一个装坏或装错版本的 Pi
# 仍然会让 oracle 拿一个不存在于目标版本的语义去比。CI 里另一个 job
# （pi-target-lock.py）会锁住版本，但本地跑这个脚本时没人管。
if [[ -f "$PI_ROOT/package.json" ]]; then
  PI_VERSION="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$PI_ROOT/package.json" 2>/dev/null || true)"
  [[ "$PI_VERSION" == "0.85.1" ]] || \
    not_done "Pi 的版本是 ${PI_VERSION:-<读不出>}，不是 0.85.1 —— 这个 oracle 比的是 0.85.1 的语义，换版本就没有意义（或者说是另一件事）。装 npm i -g @earendil-works/pi-coding-agent@0.85.1，或设 PI_ROOT 指向它。"
fi
command -v cargo >/dev/null 2>&1 || not_done "环境里没有 cargo。"

# ---------------------------------------------------------------------------
# Pi 侧：跑两轮 —— sequential 一轮、parallel 一轮。
# ---------------------------------------------------------------------------
run_pi() {
  local mode="$1"
  local out code
  out="$(PI_TOOL_EXECUTION="$mode" node "$REPO/scripts/pi-parallel-tools-probe.mjs" \
    --calls "$K" --sleep-ms "$SLEEP_MS" 2>&1)" && code=0 || code=$?
  # 探针的原始输出报到 stderr：命令替换只收 stdout，这样它既能被脚本解析，
  # 又在人看得到的地方留痕。
  echo "$out" >&2
  if [[ "$code" == "2" ]]; then
    echo "$out" >&2
    not_done "Pi 侧探针说环境不可用（探针的退出码是 2）。"
  fi
  if [[ "$code" != "0" ]]; then
    echo "$out" >&2
    fail "Pi 侧探针没跑成（退出码 $code）。"
  fi
  printf '%s\n' "$out"
}

pi_field() {
  # $1 = 输出，$2 = 字段名（PI_PEAK / PI_OVERLAP_PEAK / PI_MODE）
  printf '%s\n' "$1" | grep -oE "$2=[A-Za-z0-9]+" | tail -1 | cut -d= -f2 || true
}

note "Pi 侧：$K 个工具调用，每个睡 ${SLEEP_MS}ms —— sequential"
PI_SEQ_OUT="$(run_pi sequential)"
PI_SEQ_MODE="$(pi_field "$PI_SEQ_OUT" 'PI_MODE')"
PI_SEQ_PEAK="$(pi_field "$PI_SEQ_OUT" 'PI_PEAK')"
PI_SEQ_OVERLAP="$(pi_field "$PI_SEQ_OUT" 'PI_OVERLAP_PEAK')"
[[ -n "$PI_SEQ_PEAK" ]] || fail "Pi 侧探针没有报出 PI_PEAK=（输出：$PI_SEQ_OUT）"
[[ -n "$PI_SEQ_OVERLAP" ]] || fail "Pi 侧探针没有报出 PI_OVERLAP_PEAK=（输出：$PI_SEQ_OUT）"
[[ "$PI_SEQ_MODE" == "sequential" ]] || \
  fail "Pi 侧探针报的模式是 ${PI_SEQ_MODE:-<没有>}，不是 sequential —— 探针没切到串行"
record_assertion "pi_mode_is_sequential"

note "Pi 侧：$K 个工具调用，每个睡 ${SLEEP_MS}ms —— parallel"
PI_PAR_OUT="$(run_pi parallel)"
PI_PAR_MODE="$(pi_field "$PI_PAR_OUT" 'PI_MODE')"
PI_PAR_PEAK="$(pi_field "$PI_PAR_OUT" 'PI_PEAK')"
PI_PAR_OVERLAP="$(pi_field "$PI_PAR_OUT" 'PI_OVERLAP_PEAK')"
[[ -n "$PI_PAR_PEAK" ]] || fail "Pi 侧探针没有报出 PI_PEAK=（输出：$PI_PAR_OUT）"
[[ -n "$PI_PAR_OVERLAP" ]] || fail "Pi 侧探针没有报出 PI_OVERLAP_PEAK=（输出：$PI_PAR_OUT）"
[[ "$PI_PAR_MODE" == "parallel" ]] || \
  fail "Pi 侧探针报的模式是 ${PI_PAR_MODE:-<没有>}，不是 parallel —— 探针没切到并行"
record_assertion "pi_mode_is_parallel"

# ---------------------------------------------------------------------------
# nini 侧：同一份剧本，模式由环境变量 `NINI_SEQ_MODE` 切换。
#
# 测试自己打印 `NINI_SEQ_PEAK=` / `NINI_SEQ_CALLS=`（测试名与打印标记都要
# 稳定——脚本消费的就是这两行）。
#
# `--nocapture` 会把 `test <name> ... ` 打在 `NINI_SEQ_PEAK=` 同一行，所以用
# `grep -oE` 抓，不用 `^NINI_SEQ_PEAK=`。
# ---------------------------------------------------------------------------
run_nini() {
  local mode="$1"
  local out code
  out="$(cd "$REPO" && NINI_SEQ_MODE="$mode" NINI_PARALLEL_CALLS="$K" \
    cargo test -q -p nini-core --test agent_integration \
    the_batch_execution_mode_decides_the_peak -- --nocapture 2>&1)" && code=0 || code=$?
  # 抓出来的数字报到 stderr（命令替换只收 stdout），这样人看得到。
  echo "$out" | grep -oE 'NINI_SEQ_(MODE|CALLS|PEAK)=[A-Za-z0-9]+' >&2 || true
  # 测量测试自己红 = 这次测量**不可信**，脚本必须因此失败。
  # v0.15.0 (审查修正): 以前这里只打印一句提示就继续，于是脚本会拿
  # 一个「测试已经失败了、但数字还在」的输出继续判据，最后打印出
  # 自相矛盾的结论（`OK: … NINI_SEQ_PEAK=20 …` —— 而 20 恰恰是
  # sequential 下**不该**出现的值）。测量失败 = 门禁失败。
  if [[ "$code" != "0" ]]; then
    echo "$out" >&2
    fail "nini 侧的测量测试自己也是红的（退出码 $code）—— 这次测量不可信，门禁不能拿它当判据"
  fi
  printf '%s\n' "$out"
}

note "nini 侧：$K 个工具调用，每个睡 ${SLEEP_MS}ms —— sequential"
NINI_SEQ_OUT="$(run_nini sequential)"
NINI_SEQ_PEAK="$(printf '%s\n' "$NINI_SEQ_OUT" | grep -oE 'NINI_SEQ_PEAK=[0-9]+' | tail -1 | cut -d= -f2 || true)"
NINI_SEQ_CALLS="$(printf '%s\n' "$NINI_SEQ_OUT" | grep -oE 'NINI_SEQ_CALLS=[0-9]+' | tail -1 | cut -d= -f2 || true)"
NINI_SEQ_MODE="$(printf '%s\n' "$NINI_SEQ_OUT" | grep -oE 'NINI_SEQ_MODE=[A-Za-z]+' | tail -1 | cut -d= -f2 || true)"
[[ -n "$NINI_SEQ_PEAK" ]] || \
  fail "nini 侧没有报出 NINI_SEQ_PEAK=（测试改名了，或它没跑起来）。"
[[ -n "$NINI_SEQ_CALLS" ]] || \
  fail "nini 侧没有报出 NINI_SEQ_CALLS=（测试改名了，或它没跑起来）。"
[[ "$NINI_SEQ_MODE" == "sequential" ]] || \
  fail "nini 侧报的模式是 ${NINI_SEQ_MODE:-<没有>}，不是 sequential —— 测量测的不是串行"
record_assertion "nini_mode_is_sequential"

note "nini 侧：$K 个工具调用，每个睡 ${SLEEP_MS}ms —— parallel"
NINI_PAR_OUT="$(run_nini parallel)"
NINI_PAR_PEAK="$(printf '%s\n' "$NINI_PAR_OUT" | grep -oE 'NINI_SEQ_PEAK=[0-9]+' | tail -1 | cut -d= -f2 || true)"
NINI_PAR_CALLS="$(printf '%s\n' "$NINI_PAR_OUT" | grep -oE 'NINI_SEQ_CALLS=[0-9]+' | tail -1 | cut -d= -f2 || true)"
NINI_PAR_MODE="$(printf '%s\n' "$NINI_PAR_OUT" | grep -oE 'NINI_SEQ_MODE=[A-Za-z]+' | tail -1 | cut -d= -f2 || true)"
[[ -n "$NINI_PAR_PEAK" ]] || \
  fail "nini 侧没有报出 NINI_SEQ_PEAK=（测试改名了，或它没跑起来）。"
[[ -n "$NINI_PAR_CALLS" ]] || \
  fail "nini 侧没有报出 NINI_SEQ_CALLS=（测试改名了，或它没跑起来）。"
[[ "$NINI_PAR_MODE" == "parallel" ]] || \
  fail "nini 侧报的模式是 ${NINI_PAR_MODE:-<没有>}，不是 parallel —— 测量测的不是并行"
record_assertion "nini_mode_is_parallel"


note "sequential：nini peak=$NINI_SEQ_PEAK Pi peak=$PI_SEQ_PEAK Pi重叠=$PI_SEQ_OVERLAP"
note "parallel：  nini peak=$NINI_PAR_PEAK Pi peak=$PI_PAR_PEAK Pi重叠=$PI_PAR_OVERLAP K=$K"

# 两侧的剧本必须真的是同一个。
[[ "$NINI_SEQ_CALLS" == "$K" ]] || \
  fail "两侧的剧本不是同一个：nini 侧跑了 $NINI_SEQ_CALLS 个调用，脚本要的是 $K —— K 必须两侧同源"
[[ "$NINI_PAR_CALLS" == "$K" ]] || \
  fail "两侧的剧本不是同一个：nini 侧跑了 $NINI_PAR_CALLS 个调用，脚本要的是 $K —— K 必须两侧同源"
record_assertion "nini_calls_match_k"

# 断言 1：Pi 侧在 sequential 下必须一个一个跑（两个独立证据都要这么说）。
[[ "$PI_SEQ_PEAK" == "1" ]] || \
  fail "Pi 侧 sequential 的峰值是 $PI_SEQ_PEAK，不是 1 —— Pi 的批次级串行没有生效"
record_assertion "pi_seq_peak_is_1"
[[ "$PI_SEQ_OVERLAP" == "1" ]] || \
  fail "Pi 侧 sequential 的重叠区间数是 $PI_SEQ_OVERLAP，不是 1 —— Pi 侧的两个证据互相矛盾"
record_assertion "pi_seq_overlap_is_1"

# 断言 2：nini 侧在 sequential 下也必须一个一个跑。
[[ "$NINI_SEQ_PEAK" == "1" ]] || \
  fail "nini 侧 sequential 的峰值是 $NINI_SEQ_PEAK，不是 1 —— nini 的批次级串行没有生效"
record_assertion "nini_seq_peak_is_1"

# 断言 3：对照组 —— parallel 模式下两边都必须把 K 个调用一起跑。
# 没有这一条，「nini 永远 peak==1」也能骗过断言 2。
[[ "$PI_PAR_PEAK" == "$K" ]] || \
  fail "Pi 侧 parallel 的峰值是 $PI_PAR_PEAK，不是 K=$K —— 对照组不成立"
record_assertion "pi_par_peak_equals_k"
[[ "$PI_PAR_OVERLAP" == "$K" ]] || \
  fail "Pi 侧 parallel 的重叠区间数是 $PI_PAR_OVERLAP，不是 K=$K —— 对照组不成立"
record_assertion "pi_par_overlap_equals_k"
[[ "$NINI_PAR_PEAK" == "$K" ]] || \
  fail "nini 侧 parallel 的峰值是 $NINI_PAR_PEAK，不是 K=$K —— 对照组不成立"
record_assertion "nini_par_peak_equals_k"

# 收尾：清单里的每一条判据都必须真的跑到了。
# 少了哪一条 → 说明判据集被改动了（而被删掉的那条正是有人在守的东西）。
for _assertion in "${ASSERTIONS[@]}"; do
  if [[ -z "${ASSERTION_DONE[$_assertion]:-}" ]]; then
    fail "判据清单里的 \"$_assertion\" 没有被执行 —— 判据集被改动过（这条判据现在没人守了）"
  fi
done

echo "OK：sequential 两侧的峰值都是 1，parallel 两侧的峰值都是 K=$K（NINI_SEQ_PEAK=$NINI_SEQ_PEAK PI_SEQ_PEAK=$PI_SEQ_PEAK NINI_PAR_PEAK=$NINI_PAR_PEAK PI_PAR_PEAK=$PI_PAR_PEAK）"
