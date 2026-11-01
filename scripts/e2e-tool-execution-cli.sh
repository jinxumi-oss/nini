#!/usr/bin/env bash
# Oracle（黑盒）：**真的跑 nini 二进制**，测 `--tool-execution` 有没有被兑现。
#
# 为什么要有这一条（v0.15.0 的第二轮复审）：
#
#   scripts/e2e-tool-execution.sh 里的 nini 侧是用**测试内的环境变量**
#   （`NINI_SEQ_MODE=sequential`）切换模式的，测的是「agent 核心在
#   sequential 下峰值是 1」。它**完全没有经过 CLI**。
#
#   这正是 v0.14 翻车的那一类：v0.14 的 tester 实测过「CLI flag 被丢弃时，
#   全仓测试仍然全绿」。那一轮的补救是加断言，但断言守的是
#   「flag 被解析」和「装配门把它传下去了」—— 没有任何一条**端到端**证据
#   说「用户敲的 `--tool-execution sequential` 真的让工具一个一个跑」。
#
#   这个脚本补的就是那条端到端证据：跑**编译出来的二进制**，
#   用**墙钟耗时**判断并发度。
#
# 为什么用耗时而不是峰值：
#
#   峰值（同时在飞的工具数）在黑盒里看不见 —— nini 不把它打印出来。
#   耗时可以：K 个各睡 S 秒的 bash 调用，并发时总耗时 ≈ S，串行时 ≈ K×S。
#   这个判据是**单向安全**的：机器慢只会让并发侧变慢（不会让串行侧变快），
#   所以阈值只需卡在两者之间。
#
# 阈值为什么取**两端的中点**，而不是拍脑袋的倍数：
#
#   设「进程与机器开销」为 T（实测 ≈ 40ms：nini 启动 + 两次本地 HTTP +
#   工具派发），两个真值是 S+T 与 K·S+T。
#   * 阈值太靠串行侧 → 并发侧离阈值只剩一点，机器一慢就是**假红**；
#   * 阈值太靠并发侧 → 变异（flag 被丢弃）就贴着阈值过，**假绿**。
#   中点让两种风险**同时**最大：余量 = (K−1)·S/2。
#   K=4、S=1s 时余量 1.5s —— 比 CI runner 上真会出现的抖动大一个量级。
#
# 第三条判据（黑盒能证明的「真的跑了 K 个工具」）：
#
#   工具调用本身在黑盒里看不见，但**工具结果**看得见：mock 收到第二次
#   请求时，body 里带着 K 条 role="tool" 的消息。所以 mock 的日志里多记
#   一个 `tool_results=N`，oracle 据此断言「这一批真的跑了 K 个工具，
#   结果都回来了」。没有它，「模型压根没给出工具调用、nini 秒回」这种
#   塌缩只能靠耗时间接发现。
#
# 退出码：0 = 通过；1 = 失败；2 = 环境不具备（没做）。

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

K="${K:-4}"                # 一批里的工具调用数
SLEEP="${SLEEP:-1}"        # 每个工具睡多久（秒）

export PATH="/home/jin/.cargo/bin:$PATH"

say()  { echo "-- $*"; }
fail() { echo "!! $*" >&2; exit 1; }
not_done() { echo "-- 没做：$*" >&2; exit 2; }

command -v cargo >/dev/null 2>&1 || not_done "环境里没有 cargo。"
command -v python3 >/dev/null 2>&1 || not_done "环境里没有 python3。"
python3 -c 'import aiohttp' 2>/dev/null || not_done "环境里没有 aiohttp（mock provider 需要它）。"

# ---------------------------------------------------------------------------
# 参数护栏。
#
# 这个判据是**墙钟**，所以进程开销会吃掉余量。实测开销 ≈ 40ms —— 但 CI 的
# runner 未必这么空，而 SLEEP 是可以被人调小的：
#
#   SLEEP=0.1 时余量只剩 (K−1)×S/2 = 150ms，一次 CI 抖动就是一次假红。
#
# 所以 SLEEP 有下限（0.5s），K 有下限（2 —— K=1 时「一批」没有意义，
# 串行与并发的真值相同，判据无从区分）。
# ---------------------------------------------------------------------------
# 注意：这里**不能**直接写 `[[ "$K" -ge 2 ]]` —— 脚本开着 `set -u`，
# 而 bash 的 `[[ ]]` 算术比较会把非数字的 K 当成**变量名**去展开，
# 未绑定变量的后果是**脚本静默退出 1**（既没有消息，也不是约定的 2）。
# 所以先用正则确认它是整数，再做比较。
if ! [[ "${K}" =~ ^[0-9]+$ ]] || [[ "${K}" -lt 2 ]]; then
  not_done "K=${K} 不可用：K 必须是不小于 2 的整数（K=1 时「一批」没有意义，串行与并发的真值相同，判据无从区分）。"
fi
# 归一化：`04` 在 bash 算术里是**八进制**（4），但 `10#$K` 才是十进制，
# 而且它让后面「工具结果个数 == K」的比较不会因为 `04` 与 `4` 而误判。
K=$((10#$K))
python3 -c "import sys; sys.exit(0 if float('$SLEEP') >= 0.5 else 1)" 2>/dev/null || \
  not_done "SLEEP=${SLEEP} 不可用：它必须是不小于 0.5 的秒数——墙钟判据的余量会被进程开销淹没（实测开销 ≈ 40ms）。"

free_port() {
  python3 -c 'import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()'
}

# ---------------------------------------------------------------------------
# 造一个只属于本次运行的临时目录 —— 绝不碰用户的 ~/.nini/agent/。
# ---------------------------------------------------------------------------
WORK="$(mktemp -d)"
MOCK_PIDS=()
cleanup() {
  for pid in ${MOCK_PIDS[@]+"${MOCK_PIDS[@]}"}; do
    kill "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
  done
  rm -rf "$WORK"
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# 脚本：第一轮一次给 K 个 tool_call（每个睡 SLEEP 秒），第二轮给最终回答。
# ---------------------------------------------------------------------------
python3 - "$WORK/script.json" "$K" "$SLEEP" <<'PYEOF'
import json, sys
path, k, sleep = sys.argv[1], int(sys.argv[2]), float(sys.argv[3])
calls = [{"name": "bash", "arguments": {"command": f"sleep {sleep}"}} for _ in range(k)]
json.dump({"turns": [
    {"type": "tool_calls", "calls": calls},
    {"type": "text", "text": "DONE"},
]}, open(path, "w"))
PYEOF

# ---------------------------------------------------------------------------
# 起 mock provider。
#
# **每个 mode 一个独立的 mock 实例。**
#
# 教训：mock 的 turn 计数是**进程内全局**的，第 N 次请求返回 turns[N]，
# 超出后重复最后一条。所以两次跑若共用一个 mock，第二次的第一轮请求
# 直接拿到最后那条 `{"type":"text","text":"DONE"}` —— 没有工具调用，
# 于是「parallel 用了 20ms」。那不是并行快，是**什么都没跑**。
# 一个会让人得出相反结论的假测量，比没有测量更坏。
#
# 端口也**不写死**：写死端口时，上一次运行残留的 mock 会占着它，而
# 「端口能连上」看起来一切正常 —— nini 的请求于是打到**别人的** mock 上，
# 报 404，整个 oracle 判成失败，而失败原因与它要测的东西毫无关系。
# ---------------------------------------------------------------------------
start_mock() {
  local port="$1"
  local script="$2"
  python3 scripts/mock-provider.py --listen "127.0.0.1:$port" \
    --script "$script" --model mock-model > "$WORK/mock-$port.log" 2>&1 &
  local pid=$!
  MOCK_PIDS+=("$pid")
  for _ in $(seq 1 60); do
    if ! kill -0 "$pid" 2>/dev/null; then
      cat "$WORK/mock-$port.log" >&2
      not_done "mock provider 没能起来（端口 $port）。"
    fi
    if python3 -c 'import socket,sys
s = socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=0.3)
s.close()' "$port" 2>/dev/null; then
      return 0
    fi
    sleep 0.2
  done
  not_done "mock provider 在端口 $port 上没就绪。"
}

# ---------------------------------------------------------------------------
# 跑一次 nini，把**耗时**和**退出码**写到文件。
#
# 为什么不直接 `T="$(run_nini …)"`：`$( )` 在**子 shell**里跑，函数里的
# `fail` 只能 `exit` 那个子 shell —— 父脚本会继续往下跑，把同一次失败
# 又报一遍，然后拿一个**空字符串**当耗时继续判。
# 所以这里只让函数写文件，判断留在父 shell。
# ---------------------------------------------------------------------------
run_nini() {
  local mode="$1"
  local start end code port agent_dir
  port="$(free_port)"
  agent_dir="$WORK/agent-$mode"
  mkdir -p "$agent_dir"

  # 每个 mode 一份 models.json，指向**自己**的 mock。
  # The spelling is Pi's: a top-level "providers" object.
  cat > "$agent_dir/models.json" <<JSONEOF
{
  "providers": {
    "mock-gw": {
      "api": "openai-completions",
      "baseUrl": "http://127.0.0.1:$port/v1",
      "apiKey": "mock-key",
      "models": [{ "id": "mock-model", "name": "mock-model" }]
    }
  }
}
JSONEOF
  start_mock "$port" "$WORK/script.json"

  start="$(python3 -c 'import time;print(int(time.time()*1000))')"
  ( cd "$WORK" && NINI_AGENT_DIR="$agent_dir" "$NINI_BIN" \
      --provider mock-gw --model mock-model \
      --tool-execution "$mode" \
      -p "run the batch" ) > "$WORK/nini-$mode.log" 2>&1
  code=$?
  end="$(python3 -c 'import time;print(int(time.time()*1000))')"
  printf '%s\n' "$(( end - start ))" > "$WORK/time-$mode"
  printf '%s\n' "$code" > "$WORK/code-$mode"
  printf '%s\n' "$port" > "$WORK/port-$mode"
}

# ---------------------------------------------------------------------------
# 编译 nini（用 release —— debug 的工具执行慢，耗时会失真）。
# ---------------------------------------------------------------------------
say "编译 nini（release）……"
cargo build --release --quiet 2>"$WORK/build.log" || {
  tail -20 "$WORK/build.log" >&2
  fail "nini 编译失败。"
}
NINI_BIN="$REPO/target/release/nini"
[[ -x "$NINI_BIN" ]] || not_done "找不到 $NINI_BIN。"

say "跑 nini --tool-execution sequential ……"
run_nini sequential
say "跑 nini --tool-execution parallel ……"
run_nini parallel

for mode in sequential parallel; do
  code="$(cat "$WORK/code-$mode")"
  if [[ "$code" != "0" ]]; then
    echo "--- nini-$mode.log ---" >&2
    cat "$WORK/nini-$mode.log" >&2
    fail "nini --tool-execution $mode 退出码是 $code —— 这次测量不算数。"
  fi
  # 交付必须真的发生（否则「秒回」会被误当成「并发」）。
  grep -q "DONE" "$WORK/nini-$mode.log" || {
    echo "--- nini-$mode.log ---" >&2
    cat "$WORK/nini-$mode.log" >&2
    fail "nini 的 $mode 那次没有交付（DONE 不在输出里）—— 这次测量不算数。"
  }
  # 这一批真的跑了 K 个工具吗？工具调用在黑盒里看不见，但**工具结果**看得见：
  # mock 第二次请求里带着 K 条 role="tool" 的消息，mock 的日志把它们记成
  # `tool_results=N`。取**最大值**：nini 可能在交付后再发一次请求（那次没有
  # 工具结果），所以不能用「最后一条」。
  got="$(grep -o 'tool_results=[0-9]*' "$WORK/mock-$(cat "$WORK/port-$mode").log" \
          | cut -d= -f2 | sort -n | tail -1)"
  got="${got:-0}"
  if [[ "$got" != "$K" ]]; then
    echo "--- mock-$(cat "$WORK/port-$mode").log ---" >&2
    cat "$WORK/mock-$(cat "$WORK/port-$mode").log" >&2
    fail "nini 的 $mode 那次只带回 $got 个工具结果，剧本要的是 $K —— 这一批没有真的跑满 K 个工具，这次测量不算数。"
  fi
done

T_SEQ="$(cat "$WORK/time-sequential")"
T_PAR="$(cat "$WORK/time-parallel")"

# ---------------------------------------------------------------------------
# 判据。
#
# 并发：≈ SLEEP（K 个调用一起睡）
# 串行：≈ K × SLEEP
#
# 阈值取两端的**中点**：MID = (K+1)×SLEEP/2。
#
#   真值   并发 ≈ SLEEP + T，串行 ≈ K×SLEEP + T（T = 进程与机器开销，实测 ≈ 40ms）
#   阈值   MID
#   余量   MID − (SLEEP + T) = (K−1)×SLEEP/2 − T
#
# 为什么是点而不是倍数：
#   * 阈值太靠串行侧 → 并发侧离阈值只剩一点，机器一慢就是**假红**；
#   * 阈值太靠并发侧 → 变异（flag 被丢弃）就贴着阈值过，**假绿**。
#   中点让两种风险**同时**最大。K=4、SLEEP=1s 时余量 1.5s —— 比 CI runner
#   上真会出现的抖动大一个量级。
#
# 第三条（sequential 必须严格更慢）防的是「两次跑法塌在同一侧」：
# 如果两次都慢（或都快），那这个 oracle 什么也没证明。
# ---------------------------------------------------------------------------
MID_MS="$(python3 -c "print(int($SLEEP * ($K + 1) * 1000 / 2))")"
SEQ_LOW="$MID_MS"
PAR_HIGH="$MID_MS"

say "sequential 耗时 ${T_SEQ}ms，parallel 耗时 ${T_PAR}ms（K=$K，每个睡 ${SLEEP}s，中点 ${MID_MS}ms）"

if [[ "$T_SEQ" -lt "$SEQ_LOW" ]]; then
  fail "nini --tool-execution sequential 只用了 ${T_SEQ}ms，不到中点 ${SEQ_LOW}ms —— 这个 flag 没有被兑现（或者它根本没接到工具执行上）。"
fi
if [[ "$T_PAR" -gt "$PAR_HIGH" ]]; then
  fail "nini --tool-execution parallel 用了 ${T_PAR}ms，超过中点 ${PAR_HIGH}ms —— 并行没有生效。"
fi
if [[ "$T_SEQ" -le "$T_PAR" ]]; then
  fail "sequential（${T_SEQ}ms）没有比 parallel（${T_PAR}ms）慢 —— 两次跑法的差别没体现在行为上，这个 oracle 什么也没证明。"
fi

echo "OK：nini 二进制真的兑现了 --tool-execution（sequential ${T_SEQ}ms ≥ ${SEQ_LOW}ms，parallel ${T_PAR}ms ≤ ${PAR_HIGH}ms，K=$K，工具结果 $K 个）"
