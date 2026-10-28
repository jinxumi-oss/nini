#!/usr/bin/env bash
# 录制**真 Pi** 的会话 fixture。
#
# # 为什么
# ----
# nini 的 conformance fixture 之前是 **Pi 0.84.2** 录的，而且**洗过值**
# （id 被换成 8 位哈希、没同步 parentId → 父子链大面积悬空）。
# 目标统一到 0.85.1 之后，它们就成了**历史证据**而不是**对照物**。
#
# 本脚本用**脚本化的 mock provider**驱动**真 Pi**（本机安装的那一份），
# 把它写出来的 session JSONL 收下来，作为 0.85.1 的对照物。
#
# 关键设计：**只洗路径，不洗 id**。
# 旧 fixture 的悬空 parentId 是它最大的软肋（"它证明不了任何关于图的事"）。
# 保留 id 与 parentId 的对应关系，这个 fixture 就能同时证明**行级 schema**
# 与**图结构**——而后者正是 nini 至今没做到的部分。
#
# # 用法
# ----
#   bash scripts/pi-session-record.sh --out crates/nini-conformance/fixtures/pi-0.85.1-session.jsonl
#   bash scripts/pi-session-record.sh --out /tmp/x.jsonl --keep      # 保留临时目录
#
# # 退出码
# ----
#   0 录制成功并已写出
#   2 环境不可用（没有 pi / 没有 mock provider / 端口占用）
#   3 录制过程中 Pi 没有产出 session 文件
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

OUT=""
KEEP=0
LISTEN="${LISTEN:-127.0.0.1:9873}"
PI_BIN="${PI_BIN:-$(command -v pi || true)}"
MOCK_PORT="${LISTEN##*:}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    --keep) KEEP=1; shift ;;
    --pi) PI_BIN="$2"; shift 2 ;;
    *) echo "未知参数 $1" >&2; exit 2 ;;
  esac
done

[[ -n "$OUT" ]] || { echo "需要 --out" >&2; exit 2; }
[[ -n "$PI_BIN" && -x "$PI_BIN" ]] || { echo "找不到 pi（PI_BIN=…）" >&2; exit 2; }
if ss -ltn 2>/dev/null | grep -q ":$MOCK_PORT "; then
  echo "端口 $MOCK_PORT 被占用（上一轮的 mock 残留？）" >&2; exit 2
fi

# ── 场景：先一次工具调用，再一次纯文本 ────────────────────────────────────
# 工具调用那一步是为了让 fixture 里出现 toolCall / toolResult 条目——
# 那正是 nini 至今没有持久化的部分（差距 P0.1）。
SCENARIO="$(mktemp /tmp/nini-rec-scen.XXXXXX.json)"
cat > "$SCENARIO" <<'JSON'
{
  "turns": [
    {"type": "tool_call", "name": "bash", "arguments": {"command": "echo fixture-probe"}},
    {"type": "text", "text": "TOOL_RESULT_SEEN"},
    {"type": "text", "text": "SECOND_TURN_OK"}
  ]
}
JSON

# 换 HOME 会让 Python 找不到用户 site-packages（aiohttp 就在那里）。
# 必须在换 HOME **之前**把真实路径记下来（与 capture-tui.sh 同因）。
REAL_PY_SITE="$(python3 -c 'import site; print(":".join(site.getsitepackages()+[site.getusersitepackages()]))')"
export PYTHONPATH="$REAL_PY_SITE${PYTHONPATH:+:$PYTHONPATH}"
python3 -c 'import aiohttp' 2>/dev/null || { echo "python3 缺 aiohttp（pip install aiohttp）" >&2; exit 2; }

TMP="$(mktemp -d /tmp/nini-pi-rec.XXXXXX)"
export HOME="$TMP/home"
PROJECT="$TMP/project"
mkdir -p "$HOME/.pi/agent" "$PROJECT"
printf '{\n  "%s": true\n}\n' "$PROJECT" > "$HOME/.pi/agent/trust.json"

MOCK_PID=""
SESSION="nini-rec-$$"
cleanup() {
  tmux -L nini-rec kill-session -t "$SESSION" 2>/dev/null || true
  [[ -n "$MOCK_PID" ]] && kill "$MOCK_PID" 2>/dev/null || true
  rm -f "$SCENARIO"
  if [[ "$KEEP" == "1" ]]; then echo "保留：$TMP" >&2; else rm -rf "$TMP"; fi
}
trap cleanup EXIT

# ── mock provider ─────────────────────────────────────────────────────────
python3 "$REPO/scripts/mock-provider.py" --script "$SCENARIO" --listen "$LISTEN" \
  > "$TMP/mock.log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 50); do ss -ltn 2>/dev/null | grep -q ":$MOCK_PORT " && break; sleep 0.2; done
ss -ltn 2>/dev/null | grep -q ":$MOCK_PORT " || { cat "$TMP/mock.log" >&2; echo "mock 未启动" >&2; exit 2; }

cat > "$HOME/.pi/agent/models.json" <<JSON
{"providers":{"mock-rec":{"baseUrl":"http://$LISTEN/v1","api":"openai-completions","apiKey":"mock",
 "models":[{"id":"mock-rec-model","reasoning":false,"input":["text"],"contextWindow":200000,"maxTokens":8000}]}}}
JSON

# ── 起 Pi（真 Pi，不是 nini）────────────────────────────────────────────────
tmux -L nini-rec -f /dev/null new-session -d -s "$SESSION" -x 100 -y 30 -c "$PROJECT" \
  "env PI_CODING_AGENT_DIR=$HOME/.pi/agent PI_OFFLINE=1 $PI_BIN --provider mock-rec --model mock-rec-model"
tmux -L nini-rec set-option -t "$SESSION" -g extended-keys on 2>/dev/null || true

wait_for() {
  local needle="$1" tries="${2:-100}"
  for _ in $(seq 1 "$tries"); do
    tmux -L nini-rec capture-pane -p -t "$SESSION" 2>/dev/null | grep -qF -- "$needle" && return 0
    sleep 0.2
  done
  return 1
}

# 失败时把现场留下来：pane 上 Pi 说了什么、mock provider 收到了什么。
# 没有这两段，一个「没跑完」只能靠猜。
dump_state() {
  echo "--- pi pane ---" >&2
  tmux -L nini-rec capture-pane -p -t "$SESSION" 2>/dev/null | tail -40 >&2
  echo "--- mock provider ---" >&2
  tail -40 "$TMP/mock.log" 2>/dev/null >&2
}

wait_for "mock-rec-model" 150 || {
  tmux -L nini-rec capture-pane -p -t "$SESSION" 2>/dev/null | tail -20 >&2
  echo "Pi 没起来" >&2; exit 3
}

# 等启动流程真的结束。
#
# Pi 启动时会检查 fd / ripgrep，缺了就**下载**（tools-manager.js 的 ensureTool）。
# 在 CI 的 runner 上那是一次 GitHub 下载，几十秒起步；这段时间 Pi **不处理输入**
# ——贴进去的第一条提示会被 `handleStartupSubmit` 扣下（它只把文本留在编辑器里，
# 并显示「Startup is still in progress」），启动完成后**也不会自动重发**。
#
# PI_OFFLINE=1 让它跳过下载（fd/rg 只是 grep/find 工具的可选加速，本场景用不到）。
#
# 曾经这里还有一个「等 pane 上的『Startup is still in progress』消失」的循环——
# 它是**反的**：那个字符串只在「提交早了」之后才会出现，所以循环第一轮就 break，
# 于是它看起来像一道等待，实际上什么都没等（2026-10-28 在 CI 上实测：它等到
# 超时，而 Pi 还在启动）。
#
# 现在不再猜「启动完了没有」，而是等**机器可判定**的正向信号：mock provider
# 收到了请求。见下面的 submit_and_wait_for_request。

paste() {
  printf '%s' "$1" | tmux -L nini-rec load-buffer -b "nini-rec-$$" -
  tmux -L nini-rec paste-buffer -t "$SESSION" -b "nini-rec-$$" -p
}

# mock provider 收到几次请求了——「模型到底被问了几次」的机器证据。
#
# 注意 `grep -c` 在没有匹配时会**打印 0 并以 1 退出**，所以 `|| echo 0` 会
# 多出一行 0（值变成 "0\n0"，`[[ ]]` 直接报语法错）。只用 `|| true` 吃掉
# 退出码，计数仍由 grep 打印。
mock_request_count() {
  grep -c 'request #[0-9]* START' "$TMP/mock.log" 2>/dev/null || true
}

# 提交一条提示，并一直等到**模型真的被问到**为止。
#
# 为什么不能只等 pane 上的某个字：「Pi 启动期间提交」与「Pi 根本没回话」
# 在 pane 上长得一模一样——启动期间提交的提示被扣在编辑器里，Pi 显示
# 「Startup is still in progress」，之后也不会自动重发。
#
# 唯一可靠的正向信号是 mock provider 收到了请求。所以这里改成：
# 提交 → 等 mock 的请求数增加 → 还没增加就**再按一次 Enter**
# （启动完成后，这一次 Enter 才会真的把编辑器里的文本发出去；
#  而在启动完成前，Enter 只是被 handleStartupSubmit 收下，不会误发）。
submit_and_wait_for_request() {
  local text="$1" tries="${2:-150}"
  local before
  before="$(mock_request_count)"
  paste "$text"
  tmux -L nini-rec send-keys -t "$SESSION" Enter
  for _ in $(seq 1 "$tries"); do
    if [[ "$(mock_request_count)" -gt "$before" ]]; then
      return 0
    fi
    tmux -L nini-rec send-keys -t "$SESSION" Enter
    sleep 0.2
  done
  return 1
}

# 第一轮：触发工具调用
#
# 提示里**不能**出现要等的那个词。之前的提示是
# 「run the bash tool then say TOOL_RESULT_SEEN」，而 wait_for 找的是
# “pane 里出现 TOOL_RESULT_SEEN”——**用户自己输入的那行也满足它**。
# 于是「Pi 根本没回话」会被判成「跑完了」：这道门禁可以在 Pi 完全没响应的情况下变绿。
# 现在提示只说「做什么」，要等的是**助手**才说得出的话（那句话由 mock provider 给）。
submit_and_wait_for_request "run the bash tool" 150 \
  || { echo "第一轮：模型一直没被问到" >&2; dump_state; exit 3; }
wait_for "TOOL_RESULT_SEEN" 150 || { echo "第一轮没跑完" >&2; dump_state; exit 3; }
# 第二轮：纯文本
submit_and_wait_for_request "continue" 150 \
  || { echo "第二轮：模型一直没被问到" >&2; dump_state; exit 3; }
wait_for "SECOND_TURN_OK" 150 || { echo "第二轮没跑完" >&2; dump_state; exit 3; }
sleep 1

# 退出（Ctrl+D 两次）——退出时 Pi 才把会话写完
tmux -L nini-rec send-keys -t "$SESSION" C-d
sleep 0.6
tmux -L nini-rec send-keys -t "$SESSION" C-d
for _ in $(seq 1 60); do
  tmux -L nini-rec has-session -t "$SESSION" 2>/dev/null || break
  sleep 0.2
done
sleep 0.5

# ── 收集 session 文件 ──────────────────────────────────────────────────────
SESSION_FILE="$(find "$HOME/.pi/agent/sessions" -name '*.jsonl' -type f 2>/dev/null | sort | tail -1)"
[[ -n "$SESSION_FILE" ]] || { echo "Pi 没有产出 session 文件" >&2; dump_state; ls -R "$HOME/.pi/agent" >&2; exit 3; }

# 只洗路径：cwd 与临时目录名。其余（id、parentId、时间戳、正文）**原样保留**——
# 洗掉 id 正是旧 fixture 证明不了图结构的原因。
python3 - "$SESSION_FILE" "$OUT" "$PROJECT" <<'PY'
import json, sys, pathlib
src, out, project = sys.argv[1], sys.argv[2], sys.argv[3]
lines = []
for raw in pathlib.Path(src).read_text(encoding="utf-8").splitlines():
    if not raw.strip():
        continue
    d = json.loads(raw)
    if d.get("type") == "session":
        d["cwd"] = "/scrubbed/project"
    lines.append(json.dumps(d, ensure_ascii=False, separators=(",", ":")))
pathlib.Path(out).write_text("\n".join(lines) + "\n", encoding="utf-8")
print(f"写入 {out}：{len(lines)} 条")
PY

# ── 自检：录出来的东西必须真的能当对照物 ────────────────────────────────────
python3 - "$OUT" <<'PY'
import json, sys, pathlib
lines = [json.loads(l) for l in pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines() if l.strip()]
ids = {d.get("id") for d in lines if d.get("id")}
parents = [d.get("parentId") for d in lines if d.get("parentId")]
resolving = sum(1 for p in parents if p in ids)
assert lines, "fixture 是空的"
assert any(d.get("type") == "message" for d in lines), "没有 message 条目"
print(f"条目 {len(lines)}；parentId {resolving}/{len(parents)} 能解析")
if parents and resolving * 2 >= len(parents):
    print("  （父子链基本完整 → 这个 fixture 同时证明了图结构）")
PY
