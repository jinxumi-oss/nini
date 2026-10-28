#!/usr/bin/env bash
# Oracle：复杂长任务必须走到**交付**，而不是在半路被轮数上限截断。
#
# ## 这条门禁在钉住什么
#
# nini 曾经给一次 run 设了 50 轮硬上限（`MAX_TOOL_ITERATIONS = 50`）。
# 50 轮不是「很多工作」，是「做了一半」：一个 60 步的任务会在
# `too many tool iterations (50)` 上死掉，transcript 里留下 49 条工具
# 结果、**没有答案**，session 回到 idle。Pi 0.85.1 没有这个上限
# （`references/pi-0.85.1/src/loop/agent-loop.ts` 里没有任何轮数计数器），
# 同一份 transcript 它会做完。
#
# 停在「还在干」和「干完了」之间的停止不是安全网，是截断。
#
# ## 为什么两边都跑
#
# 只测 nini 只能证明「nini 交付了」；拿真 Pi 跑同一个剧本，才能证明
# 「nini 交付得和 Pi 一样」——包括**请求次数**一样。请求次数是唯一
# 能区分「做了 60 步」和「做了 60 步然后自己停了」的证据：屏幕只能说
# 末态长什么样，mock 的日志才能说它到底被叫了几次。
#
# Pi 不在环境里时，脚本只跑 nini 那一侧并明确说明（不是静默跳过）。
#
# 用法：
#   scripts/e2e-long-task.sh
#   NINI_BIN=/path/to/nini scripts/e2e-long-task.sh
#   PI_BIN=/nonexistent scripts/e2e-long-task.sh   # 只跑 nini 侧

set -euo pipefail

# 端口是挑出来的，不是写死的：写死的端口在别人的机器上已经被别的服务
# 占着（这台机器的 9877 就被一个系统服务占了），而“端口被占”在这条
# 门禁里意味着“要么失败，要么静默地用别人的 mock”。
pick_free_port() {
  python3 -c 'import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()'
}

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NINI_BIN="${NINI_BIN:-$REPO/target/release/nini}"
PI_BIN="${PI_BIN:-$(command -v pi || true)}"
SCRIPT="${SCRIPT:-$REPO/fixtures/tui/60-step-task.json}"
NINI_LISTEN="${NINI_LISTEN:-127.0.0.1:$(pick_free_port)}"
PI_LISTEN="${PI_LISTEN:-127.0.0.1:$(pick_free_port)}"
PROMPT="do the 60-step task and deliver"
DELIVERY="TASK DELIVERED"
# 60 个 tool_call 回合 + 1 个最终回答回合。
EXPECTED_REQUESTS=61

fail() {
  echo "!! $*" >&2
  exit 1
}

note() { echo "-- $*"; }


# --- 前置检查 ---------------------------------------------------------------
[[ -x "$NINI_BIN" ]] || fail "找不到 nini 二进制：$NINI_BIN（先 cargo build --release）"
[[ -f "$SCRIPT" ]] || fail "找不到 mock 剧本：$SCRIPT"

# 换 HOME 会让 Python 找不到用户 site-packages（aiohttp 就在那里），
# 所以先把真实路径记下来。
REAL_PY_SITE="$(python3 -c 'import site; print(":".join(site.getsitepackages()+[site.getusersitepackages()]))')"
export PYTHONPATH="$REAL_PY_SITE${PYTHONPATH:+:$PYTHONPATH}"
python3 -c 'import aiohttp' 2>/dev/null || fail "python3 缺 aiohttp：pip install aiohttp"

# 端口预检：端口被占时，mock 起不来，而**上一轮的 mock 残留**会让
# 这一轮静默地用别人的剧本——宁可失败。
for listen in "$NINI_LISTEN" "$PI_LISTEN"; do
  port="${listen##*:}"
  if ss -ltn 2>/dev/null | grep -q ":$port "; then
    fail "端口 $port 已被占用——可能是上一轮的 mock 残留。换一个 LISTEN_* 端口。"
  fi
done

TMP="$(mktemp -d /tmp/nini-long-task.XXXXXX)"
PIDS=()
cleanup() {
  for pid in "${PIDS[@]:-}"; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$TMP"
}
trap cleanup EXIT

start_mock() {
  local listen="$1" log="$2"
  python3 "$REPO/scripts/mock-provider.py" --script "$SCRIPT" --listen "$listen" \
    > "$log" 2>&1 &
  PIDS+=("$!")
  local port="${listen##*:}"
  for _ in $(seq 1 50); do
    ss -ltn 2>/dev/null | grep -q ":$port " && return 0
    sleep 0.2
  done
  echo "--- mock 日志 ---" >&2
  cat "$log" >&2
  fail "mock provider 没起来：$listen"
}

count_requests() {
  grep -c 'request #[0-9]* START' "$1" || true
}

# --- 一次运行：跑一个二进制，断言它交付了 ----------------------------------
# $1 = 二进制，$2 = 监听地址，$3 = 日志前缀，$4.. = 额外环境/命令
run_side() {
  local bin="$1" listen="$2" tag="$3"
  shift 3
  local log="$TMP/$tag.mock.log"
  start_mock "$listen" "$log"
  local out="$TMP/$tag.out" err="$TMP/$tag.err"
  set +e
  "$@" > "$out" 2> "$err"
  local code=$?
  set -e
  local requests
  requests="$(count_requests "$log")"
  echo "$requests" > "$TMP/$tag.requests"

  if [[ $code -ne 0 ]]; then
    echo "--- stdout ---" >&2; tail -20 "$out" >&2
    echo "--- stderr ---" >&2; tail -20 "$err" >&2
    fail "$tag 退出码 $code（期望 0）"
  fi
  grep -q "$DELIVERY" "$out" || {
    echo "--- stdout ---" >&2; tail -20 "$out" >&2
    fail "$tag 没有交付：stdout 里找不到 '$DELIVERY'"
  }
  if grep -qE "too many tool iterations|iteration limit reached" "$out" "$err"; then
    fail "$tag 撞上了轮数上限——长任务又被截断了"
  fi
  if [[ "$requests" != "$EXPECTED_REQUESTS" ]]; then
    fail "$tag 收到 $requests 次请求（期望 $EXPECTED_REQUESTS）"
  fi
  note "$tag：$requests 次请求，交付达成"
}

# --- nini 侧 ----------------------------------------------------------------
note "nini：$NINI_BIN"
run_side "$NINI_BIN" "$NINI_LISTEN" nini \
  env "OPENAI_BASE_URL=http://$NINI_LISTEN/v1" OPENAI_API_KEY=mock \
  "$NINI_BIN" --provider openai-compat --model mock-tui-model -p "$PROMPT"
NINI_REQUESTS="$(cat "$TMP/nini.requests")"

# --- Pi 侧（对标） ----------------------------------------------------------
if [[ -n "$PI_BIN" && -x "$PI_BIN" ]]; then
  note "Pi：$PI_BIN"
  PI_HOME="$TMP/pi-home"
  mkdir -p "$PI_HOME/.pi/agent"
  python3 - "$PI_HOME/.pi/agent/models.json" "$PI_LISTEN" <<'PY'
import json, sys
path, listen = sys.argv[1], sys.argv[2]
json.dump({"providers": {"mock-long-task": {
    "baseUrl": f"http://{listen}/v1",
    "api": "openai-completions",
    "apiKey": "mock",
    "models": [{"id": "mock-tui-model", "reasoning": False, "input": ["text"],
                "contextWindow": 200000, "maxTokens": 8000}],
}}}, open(path, "w"), indent=2)
PY
  # 隔离的 HOME 里信任库是空的，Pi 会弹「Trust project folder?」并阻塞。
  printf '{\n  "%s": true\n}\n' "$TMP" > "$PI_HOME/.pi/agent/trust.json"

  run_side "$PI_BIN" "$PI_LISTEN" pi \
    env "HOME=$PI_HOME" PI_OFFLINE=1 \
    "$PI_BIN" --provider mock-long-task --model mock-tui-model -p "$PROMPT"
  PI_REQUESTS="$(cat "$TMP/pi.requests")"

  if [[ "$NINI_REQUESTS" != "$PI_REQUESTS" ]]; then
    fail "nini 用了 $NINI_REQUESTS 次请求，Pi 用了 $PI_REQUESTS 次——两者必须一致"
  fi
  note "nini 与 Pi 的请求次数一致：$NINI_REQUESTS"
else
  note "Pi 不在环境里——只断言了 nini 一侧（没有对标就没有对标）"
fi

echo "OK：长任务交付达成，且没有轮数上限截断"
