#!/usr/bin/env bash
# TUI 对照 oracle：把真 Pi 与 nini 的**真实屏幕画面**录下来。
#
# # 为什么是屏幕而不是内部状态
#
# nini 的 1428 个测试里有 60 处 TestBackend 帧断言、6 个文件走 tmux，
# 但**没有一条是"Pi 实际长这样"录下来的**——它们全部是"作者认为 Pi
# 长这样"写出来的（docs/invariant-audit.md §3）。`/hotkeys` 显示 `Home ?`
# 这个真缺陷正是靠肉眼看屏幕发现的，不是靠任何测试。
#
# # 为什么能确定性
#
# 上游是 scripts/mock-provider.py（脚本化、离线、无 token 消耗）。
# 同一份脚本同时喂给 Pi 和 nini，所以画面差异就是对照差异。
# 等待一律轮询期望字符串出现，不用固定 sleep——sleep 录出来的东西
# 每次都不一样，那不是 oracle。
#
# # 用法
#
#   bash scripts/capture-tui.sh --bin pi   --scenario scenarios/01-hotkeys.json
#   bash scripts/capture-tui.sh --bin nini --scenario scenarios/01-hotkeys.json
#   bash scripts/capture-tui.sh --diff
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LISTEN="${LISTEN:-127.0.0.1:9880}"
COLS="${COLS:-100}"
ROWS="${ROWS:-30}"
OUT_ROOT="${OUT_ROOT:-$REPO/captures/tui}"
REAL_PI="${REAL_PI:-/home/jin/.nvm/versions/node/v25.9.0/bin/pi}"
NINI_BIN="${NINI_BIN:-$REPO/target/debug/nini}"

# 独立 socket：默认 socket 会复用遗留的 tmux server，而 server 继承的是
# 首次启动它时那个 client 的环境（HOME/尺寸/shell）。实测复用旧 server 时
# 输入行左侧会多出一个 `0 `，用新 server 则没有——那是污染，不是 Pi 的行为。
TMUX=(tmux -L nini-cap)
BIN=""; SCENARIO=""; MODE="capture"
[[ "${1:-}" == "--diff" ]] && MODE="diff"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --bin) BIN="$2"; shift 2 ;;
    --scenario) SCENARIO="$2"; shift 2 ;;
    --diff) MODE="diff"; shift ;;
    *) echo "未知参数 $1"; exit 1 ;;
  esac
done

# --- 确定性等待：轮询屏幕直到出现期望串 ---------------------------------
# 每次轮询给 tmux 一点时间重绘。超时则打印当前屏幕，便于诊断。
wait_for() {
  local sess="$1" needle="$2" tries="${3:-60}"
  for _ in $(seq 1 "$tries"); do
    if "${TMUX[@]}" capture-pane -p -t "$sess" 2>/dev/null | grep -qF -- "$needle"; then
      return 0
    fi
    sleep 0.15
  done
  echo "!! 等待超时：找不到 [$needle]" >&2
  echo "--- 当前屏幕 ---" >&2
  "${TMUX[@]}" capture-pane -p -t "$sess" 2>/dev/null | sed 's/^/   | /' >&2
  return 1
}

snap() {
  local sess="$1" out="$2"
  "${TMUX[@]}" capture-pane -p -t "$sess" > "$out"
}

# 等屏幕**稳定**：连续两次快照完全相同。
#
# 为什么不按字符串等：输入 `/hotkeys` 会同时触发输入回显与 autocomplete
# 下拉，两者重绘不同步。按字符串一命中就快照，录到的是“下拉画到一半”
# 的帧——我第一版就因此把输入读成了 `0/hotkeys`（那个 `0` 其实是下拉里的
# `→` 选择指示符落在输入行上）。
#
# 为什么不 sleep：sleep 录出来的画面每次都不一样，那不是 oracle。
snap_stable() {
  local sess="$1" out="$2" tries="${3:-40}"
  local prev="" cur=""
  for _ in $(seq 1 "$tries"); do
    cur="$("${TMUX[@]}" capture-pane -p -t "$sess" 2>/dev/null)"
    if [[ "$cur" == "$prev" ]]; then
      printf '%s\n' "$cur" > "$out"
      return 0
    fi
    prev="$cur"
    sleep 0.12
  done
  printf '%s\n' "$cur" > "$out"
  return 0
}

# --- diff 模式 -------------------------------------------------------------
if [[ "$MODE" == "diff" ]]; then
  fail=0
  for scen in "$REPO"/scenarios/*.json; do
    name="$(basename "$scen" .json)"
    for step_file in "$OUT_ROOT/$name"/pi/*.txt; do
      [[ -e "$step_file" ]] || continue
      step="$(basename "$step_file" .txt)"
      other="$OUT_ROOT/$name/nini/$step.txt"
      if [[ ! -e "$other" ]]; then
        echo "缺失 nini 侧：$name/$step"; fail=1; continue
      fi
      if diff -q "$step_file" "$other" >/dev/null; then
        echo "  同  $name/$step"
      else
        echo "差异 $name/$step"; diff -u "$step_file" "$other" | sed 's/^/      /' || true; fail=1
      fi
    done
  done
  exit $fail
fi

[[ -n "$BIN" && -n "$SCENARIO" ]] || { echo "需要 --bin 与 --scenario"; exit 1; }
SCEN_NAME="$(basename "$SCENARIO" .json)"
OUT="$OUT_ROOT/$SCEN_NAME/$BIN"
mkdir -p "$OUT"

# --- 临时 HOME：整个 ~ 落在临时目录，根除用户全局污染 -------------------
# 为什么要连 HOME 一起换：只换 PI_CODING_AGENT_DIR 时，启动画面仍然
# 加载到了用户那 29 个 skills，于是录出来的画面取决于“这台机器装了
# 什么”——那不是 oracle，是快照。oracle 的环境必须空。
TMP="$(mktemp -d /tmp/nini-tui.XXXXXX)"
export HOME="$TMP/home"
mkdir -p "$HOME/.pi/agent/skills" "$HOME/.pi/agent/prompts" "$HOME/.pi/agent/extensions"
ln -sf /home/jin/.pi/agent/auth.json "$HOME/.pi/agent/auth.json"

# 隔离 HOME 之后信任库是空的，Pi 会弹 “Trust project folder?” 并阻塞在
# 选择框上（选框不选，footer 永远不出现，wait_for 就只能超时）。
# 预先写入与真实环境同形的信任记录。
printf '{\n  "/home/jin": true\n}\n' > "$HOME/.pi/agent/trust.json"
mkdir -p "$HOME/.nini/agent" && cp "$HOME/.pi/agent/trust.json" "$HOME/.nini/agent/trust.json" 2>/dev/null || true
MOCK_PID=""
SESSION="nini-cap-$$"
cleanup() {
  "${TMUX[@]}" kill-session -t "$SESSION" 2>/dev/null || true
  [[ -n "$MOCK_PID" ]] && kill "$MOCK_PID" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# --- 起 mock provider -----------------------------------------------------
SCRIPT="$(python3 -c "
import json,sys
print(json.load(open('$SCENARIO'))['mock_script'])")"
python3 "$REPO/scripts/mock-provider.py" --script "$SCRIPT" --listen "$LISTEN" \
  > "$OUT.mock.log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 40); do ss -ltn 2>/dev/null | grep -q ":${LISTEN##*:}" && break; sleep 0.2; done
ss -ltn 2>/dev/null | grep -q ":${LISTEN##*:}" || { echo "mock 未启动"; cat "$OUT.mock.log"; exit 1; }
echo "mock 已就绪 $LISTEN  脚本=$SCRIPT"

# --- 组装命令行 ------------------------------------------------------------
if [[ "$BIN" == "pi" ]]; then
  python3 -c "
import json,pathlib
pathlib.Path('$HOME/.pi/agent/models.json').write_text(json.dumps({'providers':{'mock-tui':{
  'baseUrl':'http://$LISTEN/v1','api':'openai-completions','apiKey':'mock',
  'models':[{'id':'mock-tui-model','reasoning':False,'input':['text'],
             'contextWindow':200000,'maxTokens':8000}]}}},indent=2))"
  CMD=("$REAL_PI" --provider mock-tui --model mock-tui-model)
  LAUNCH_ENV=(env PI_OFFLINE=1)
else
  # nini 走 openai-compat，同一个上游；baseUrl 由 OPENAI_BASE_URL 指定
  [[ -x "$NINI_BIN" ]] || { echo "找不到 $NINI_BIN（先 cargo build）"; exit 1; }
  CMD=("$NINI_BIN" --provider openai-compat --model mock-tui-model)
  LAUNCH_ENV=(env NINI_AGENT_DIR="$HOME/.pi/agent" "OPENAI_BASE_URL=http://$LISTEN/v1" OPENAI_API_KEY=mock)
fi

# --- 起 tmux 会话 ---------------------------------------------------------
# -f /dev/null：不读用户 ~/.tmux.conf（否则里面任何一条设置都会让
#              录屏在别的机器上重放出不同画面）。extended-keys 必须开，
#              否则 Pi 自己会警告 "Modified Enter keys may not work"。
"${TMUX[@]}" -f /dev/null new-session -d -s "$SESSION" -x "$COLS" -y "$ROWS" \
  "${LAUNCH_ENV[*]} ${CMD[*]}"
"${TMUX[@]}" set-option -t "$SESSION" -g extended-keys on 2>/dev/null || true
echo "会话已起 ${COLS}x${ROWS}  bin=$BIN"

# 等 footer 稳定（footer 含 provider/model，早期渲染不含）
wait_for "$SESSION" "mock-tui-model" 150

# --- 逐步执行场景 ---------------------------------------------------------
python3 - "$SCENARIO" "$OUT" <<'PY' > "$OUT.steps"
import json, sys
scen = json.load(open(sys.argv[1]))
for step in scen["steps"]:
    print("\t".join([
        step["id"],
        step.get("send", "").replace("\\t", "\t").replace("\\n", "\n"),
        step.get("expect", ""),
        str(step.get("literal", "0")),
    ]))
PY

while IFS=$'\t' read -r id send expect literal; do
  [[ -z "$id" ]] && continue
  if [[ "$send" != "" ]]; then
    if [[ "$literal" == "1" ]]; then
      # 必须用 paste-buffer 注入，不能用 `send-keys -l`。
      # 实测对照（同一场景、同一会话）：
      #   load-buffer + paste-buffer → "/hotkeys"   ✅
      #   send-keys -l（一次性）      → "/hotkeys/hotkeys"（重复投递）
      #   send-keys -l（逐字符+等稳定） → "0/ho"（首字符被替换、末字符丢失）
      # 录屏工具自己的保真度必须先验证，否则 diff 全在比噪声。
      # 自检：bash scripts/capture-tui.sh --selftest
      printf '%s' "$send" | "${TMUX[@]}" load-buffer -b "nini-cap-$$" -
      "${TMUX[@]}" paste-buffer -t "$SESSION" -b "nini-cap-$$" -p
      sleep 0.9
    else
      "${TMUX[@]}" send-keys -t "$SESSION" "$send"
    fi
  fi
  if [[ -n "$expect" ]]; then
    wait_for "$SESSION" "$expect" 80 || true
  fi
  if [[ -n "$send" ]]; then snap_stable "$SESSION" "$OUT/$id.txt"; else snap "$SESSION" "$OUT/$id.txt"; fi
  echo "  录制 $id"
done < "$OUT.steps"

echo "完成 → $OUT"
