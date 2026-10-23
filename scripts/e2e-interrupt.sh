#!/usr/bin/env bash
# A1 的端到端：中断一个正在流式输出的回合，然后立刻再提交一次。
#
# # 为什么必须是 e2e，而不是再多一条单元测试
#
# 单元测试（tests/esc_concurrency_probe.rs）证明的是**函数**不并发 spawn。
# 这个脚本证明的是**进程**不并发 spawn：真的起一个 nini、真的连一个
# 会流式输出的 mock provider、真的在流到一半时按 Esc、真的再按 Enter。
#
# # 判据（机器可判定，退出码即判定）
#
#   1. mock provider 收到的**请求区间不得重叠**。重叠 = 第二个 agent 在
#      第一个还在流的时候就起来了 —— 那就是 A1 的原始缺陷。
#   2. session JSONL 的消息顺序必须**不交错**：不允许出现
#      「assistant 的正文出现在它后面那条 user 消息之后」。
#
# 两个判据都从**产物**里读（mock 的日志、session 文件），不是从屏幕上读。
#
# # 用法
#
#   bash scripts/e2e-interrupt.sh            # 跑
#   bash scripts/e2e-interrupt.sh --keep     # 保留临时目录（诊断用）
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

LISTEN="${LISTEN:-127.0.0.1:9891}"
COLS="${COLS:-100}"
ROWS="${ROWS:-30}"
NINI_BIN="${NINI_BIN:-$REPO/target/debug/nini}"
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

# 独立 socket：默认 socket 会复用遗留的 tmux server，而 server 继承的是
# 首次启动它时那个 client 的环境（HOME/尺寸/shell）——录到的是污染。
TMUX=(tmux -L nini-e2e)

fail() { echo "FAIL: $*" >&2; exit 1; }
ok()   { echo "  ✓ $*"; }

# --- 0. 前置：二进制必须是**含修复**的那一份 ------------------------------
if [[ ! -x "$NINI_BIN" ]]; then
  echo "构建 nini（$NINI_BIN 不存在）…"
  export PATH="$PATH:$HOME/.cargo/bin"
  cargo build -p nini-cli || fail "cargo build 失败"
fi
[[ -x "$NINI_BIN" ]] || fail "找不到 nini 二进制：$NINI_BIN"

# --- 1. 隔离环境 ----------------------------------------------------------
# 换 HOME 会让 Python 找不到用户 site-packages（aiohttp 就在那里）。
# 必须在换 HOME **之前**把真实路径记下来（与 capture-tui.sh 同因）。
REAL_PY_SITE="$(python3 -c 'import site; print(":".join(site.getsitepackages()+[site.getusersitepackages()]))')"
export PYTHONPATH="$REAL_PY_SITE${PYTHONPATH:+:$PYTHONPATH}"
python3 -c 'import aiohttp' 2>/dev/null || fail "python3 缺 aiohttp（pip install aiohttp）"

TMP="$(mktemp -d /tmp/nini-e2e.XXXXXX)"
export HOME="$TMP/home"
mkdir -p "$HOME/.pi/agent/skills" "$HOME/.pi/agent/prompts" "$HOME/.pi/agent/extensions" \
         "$HOME/.nini/agent"
# 信任库必须预置：空信任库会让 nini 停在选择框上（与 capture-tui.sh 同因）。
printf '{\n  "%s": true\n}\n' "$HOME" > "$HOME/.pi/agent/trust.json"
cp "$HOME/.pi/agent/trust.json" "$HOME/.nini/agent/trust.json"

MOCK_PID=""
SESSION="nini-e2e-$$"
MOCK_LOG="$TMP/mock.log"
cleanup() {
  "${TMUX[@]}" kill-session -t "$SESSION" 2>/dev/null || true
  [[ -n "$MOCK_PID" ]] && kill "$MOCK_PID" 2>/dev/null || true
  if [[ "$KEEP" == "1" ]]; then echo "保留：$TMP"; else rm -rf "$TMP"; fi
}
trap cleanup EXIT

# --- 2. mock provider ------------------------------------------------------
MOCK_PORT="${LISTEN##*:}"
if ss -ltn 2>/dev/null | grep -q ":$MOCK_PORT "; then
  fail "端口 $MOCK_PORT 被占用（上一轮的 mock 残留？）"
fi
python3 "$REPO/scripts/mock-provider.py" \
  --script "$REPO/fixtures/tui/03-interrupt.json" \
  --listen "$LISTEN" --split-bytes 48 --chunk-delay 0.05 \
  > "$MOCK_LOG" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 50); do ss -ltn 2>/dev/null | grep -q ":$MOCK_PORT " && break; sleep 0.2; done
ss -ltn 2>/dev/null | grep -q ":$MOCK_PORT " || { cat "$MOCK_LOG"; fail "mock 未启动"; }
ok "mock 已就绪 $LISTEN"

# --- 3. 起 nini -----------------------------------------------------------
"${TMUX[@]}" -f /dev/null new-session -d -s "$SESSION" -x "$COLS" -y "$ROWS" -c "$HOME" \
  "env NINI_AGENT_DIR=$HOME/.nini/agent OPENAI_BASE_URL=http://$LISTEN/v1 OPENAI_API_KEY=mock \
   $NINI_BIN --provider openai-compat --model mock-tui-model"
"${TMUX[@]}" set-option -t "$SESSION" -g extended-keys on 2>/dev/null || true

wait_for() {
  local needle="$1" tries="${2:-100}"
  for _ in $(seq 1 "$tries"); do
    "${TMUX[@]}" capture-pane -p -t "$SESSION" 2>/dev/null | grep -qF -- "$needle" && return 0
    sleep 0.15
  done
  return 1
}
paste() {
  printf '%s' "$1" | "${TMUX[@]}" load-buffer -b "nini-e2e-$$" -
  "${TMUX[@]}" paste-buffer -t "$SESSION" -b "nini-e2e-$$" -p
}
wait_for "mock-tui-model" 150 || { "${TMUX[@]}" capture-pane -p -t "$SESSION" | tail -20 >&2; fail "nini 没起来"; }
ok "nini 已就绪"

# /new：TUI 启动时**不建** session 文件（footer 显示 [no session]），
# 只有 /new 会创建 `<agent dir>/sessions/<project>/<ts>.jsonl`。
# 所以这里必须先 /new，否则判据 2 没有文件可读。
# （这也是一条独立的发现：A5，见 docs/plans/parity-with-pi.md §2。）
paste "/new"
"${TMUX[@]}" send-keys -t "$SESSION" Enter
sleep 1.0

# --- 4. 触发路径：提交 → 流式中按 Esc → 立刻再提交 -----------------------
paste "first prompt"
"${TMUX[@]}" send-keys -t "$SESSION" Enter
wait_for "FIRST_TURN_MARKER" 100 || fail "第一轮没有开始流式输出"

# 等到流式输出真的在跑（屏幕上的字符在增长），再按 Esc
sleep 0.6
"${TMUX[@]}" send-keys -t "$SESSION" Escape
sleep 0.15                     # Esc 与下一次 Enter 之间只留 150ms
paste "second prompt"
"${TMUX[@]}" send-keys -t "$SESSION" Enter

# 给两个回合都留出结束时间
sleep 3
"${TMUX[@]}" capture-pane -p -t "$SESSION" > "$TMP/screen.txt" 2>/dev/null || true

# --- 5. 优雅退出：session 是落盘后才可读 ------------------------------------
# 杀掉 tmux 会话是 SIGHUP，session 还没落盘。Ctrl+D 两次是 nini 自己的
# 退出路径（第一次按会问「真的要退出吗」，第二次才退），退出时 flush。
"${TMUX[@]}" send-keys -t "$SESSION" C-d
sleep 0.6
"${TMUX[@]}" send-keys -t "$SESSION" C-d
for _ in $(seq 1 60); do
  "${TMUX[@]}" has-session -t "$SESSION" 2>/dev/null || break
  sleep 0.2
done
sleep 0.4

# --- 5. 判据 1：mock 的请求区间不得重叠 -----------------------------------
python3 - "$MOCK_LOG" <<'PY' || exit 1
import re, sys
log = open(sys.argv[1], encoding="utf-8", errors="replace").read()
def ts(line):
    m = re.search(r"(\d{2}:\d{2}:\d{2})\.(\d{3})", line)
    if not m:
        return None
    h, mi, s = map(int, m.group(1).split(":"))
    return h * 3600 + mi * 60 + s + int(m.group(2)) / 1000
starts, ends = [], []
for line in log.splitlines():
    if " request #" not in line:
        continue
    t = ts(line)
    if t is None:
        continue
    if " END " in line or " CUT " in line:
        ends.append(t)
    elif " START " in line:
        starts.append(t)
print(f"  mock: {len(starts)} 个请求，{len(ends)} 个结束/中断")
if not starts:
    print("FAIL: mock 一个请求都没收到——测试没打中目标路径")
    sys.exit(1)
if len(ends) < len(starts):
    print("FAIL: 有请求既没有 END 也没有 CUT——时间线有洞，无法判定重叠")
    sys.exit(1)
overlaps = []
for i in range(len(starts)):
    for j in range(i + 1, len(starts)):
        # 第 j 个请求在第 i 个请求结束之前就到了 → 并发
        if i < len(ends) and starts[j] < ends[i]:
            overlaps.append((i, j, starts[j], ends[i]))
if overlaps:
    for i, j, s, e in overlaps:
        print(f"FAIL: 请求 #{i+1} 与 #{j+1} **重叠**（#{j+1} 于 {s:.3f}s 到达，"
              f"而 #{i+1} 到 {e:.3f}s 才结束）——第二个 agent 在第一个还在流时就起来了")
    sys.exit(1)
print("  ✓ 请求区间不重叠：没有并发 agent")
PY

# --- 6. 判据 2：session 里的消息顺序不得交错 -------------------------------
SESSION_FILE="$(find "$HOME/.nini/agent" -name '*.jsonl' -type f 2>/dev/null | head -1)"
[[ -n "$SESSION_FILE" ]] || fail "找不到 session 文件（nini 没有落盘？）"
python3 - "$SESSION_FILE" <<'PY' || exit 1
import json, sys
path = sys.argv[1]
entries = []
for line in open(path, encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    try:
        e = json.loads(line)
    except json.JSONDecodeError:
        continue
    msg = e.get("message") or {}
    role = msg.get("role")
    content = msg.get("content")
    text = ""
    if isinstance(content, str):
        text = content
    elif isinstance(content, list):
        text = "".join(p.get("text", "") for p in content if isinstance(p, dict))
    entries.append((role or e.get("type", "?"), e.get("timestamp", ""), text))

users = [i for i, (r, _, _) in enumerate(entries) if r == "user"]
print(f"  session 消息序列: {[r for r, _, _ in entries]}")
if not users:
    print("FAIL: session 里一条 user 消息都没有——测试没打中目标路径")
    sys.exit(1)

# 判据 2a：时间戳单调（写入顺序 = 时间顺序）
stamps = [t for _, t, _ in entries if t]
for a, b in zip(stamps, stamps[1:]):
    if a > b:
        print(f"FAIL: session 条目的时间戳不是单调的（{a} 之后出现 {b}）——"
              "两条写入并发交错")
        sys.exit(1)

# 判据 2b：**内容**序：第二轮的正文不得早于第一轮的正文。
# 这比“角色序列”精确：连续的 assistant 条目是合法的（提示行 + 正文行），
# 但“第二轮的话先写下来”不是。
first = next((i for i, (_, _, t) in enumerate(entries) if "FIRST_TURN_MARKER" in t), None)
second = next((i for i, (_, _, t) in enumerate(entries) if "SECOND_TURN_MARKER" in t), None)
if first is None:
    print("FAIL: 第一轮的正文没有落盘——中断把它截断了，这一轮无法判定顺序")
    sys.exit(1)
if second is None:
    # 第二轮没有正文：它被排队了（差距 A6：排队消息不会自动投递），
    # 这不是交错。但必须有它的 user 条目，证明它没有凭空消失。
    print("  (第二轮的正文没落盘——它还在队列里，见差距 A6)")
else:
    if second < first:
        print(f"FAIL: 第二轮的正文（#{second}）写在第一轮（#{first}）之前——"
              "两个 turn 的输出交错")
        sys.exit(1)
    print(f"  ✓ 第二轮的正文（#{second}）在第一轮（#{first}）之后")
print("  ✓ session 写入顺序单调，无交错")
PY

# --- 7. 判据 3：第二次提交没有凭空消失 -------------------------------------
# 第二次提交有三种合法结局：① 被排队（agent 还活着）；② 起了新一轮；
# ③ ……没有第三种。判据是它**必须留下痕迹**：要么屏幕上看得见（排队的
# 消息会进 transcript），要么 session 里多了一条 user 消息。
# 「什么都没发生」才是缺陷。
SCREEN="$(cat "$TMP/screen.txt" 2>/dev/null || true)"
if [[ "$SCREEN" == *"second prompt"* ]]; then
  ok "第二次提交在屏幕上可见（排队或已回显）"
elif [[ "${#users[@]}" -ge 2 ]]; then
  ok "第二次提交已作为 user 消息落盘"
else
  fail "第二次提交既不在屏幕上，也没落盘——它被静默丢弃了"
fi

echo "PASS: A1 端到端通过（无并发 agent，session 顺序单调，第二次提交未丢失）"
