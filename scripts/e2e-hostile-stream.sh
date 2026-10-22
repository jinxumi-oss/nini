#!/usr/bin/env bash
# 确定性端到端：**敌意**流 × 真 TUI。
#
# # 为什么要有这个脚本
#
# 用户的报告是「一遇到复杂任务/工具调用任务就乱码」。那个失败在真 provider
# 上是**偶发**的：网络在任意字节边界切断 TCP 流，而中文一个字是三个字节，
# 所以每三个字里总有一个会被切断 —— 切断的那一刻，回合以
# `invalid utf-8` 死掉，屏幕上留下半个答案和一行错误。
#
# 偶发的东西不能用「跑一次看看」来验收。所以这里把网络变成确定性的：
# mock provider **按 7 字节一写**（必然切断中文），内容是敌意的
# （中文 / emoji / 表格 / 标题 / 代码块），然后把真 TUI 跑起来，抓屏，
# 断言答案**完整**。
#
# # 判据
#
#   * 屏幕上有 `invalid utf-8` → 失败（这一行就是用户看到的东西）
#   * 屏幕上有 U+FFFD（替换字符）→ 失败
#   * 表格的竖线不在同一列 → 失败（框线错位 = 用户说的"下划线"）
#   * 退出了但没有 EXIT=0 → 失败
#
# # 用法
#
#   bash scripts/e2e-hostile-stream.sh [width]
#
# 需要 aiohttp（mock provider 用它）。退出码 0 = 通过。
set -uo pipefail

W="${1:-100}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$(mktemp -d /tmp/nini-hostile.XXXXXX)"
SOCK=nini-hostile
SESS=hostile
PORT="${PORT:-9897}"
SCENARIO="$OUT/scenario.json"
MOCK_PID=""

cleanup() {
  tmux -L "$SOCK" kill-server 2>/dev/null
  [[ -n "$MOCK_PID" ]] && kill "$MOCK_PID" 2>/dev/null
  rm -rf "$OUT"
}
trap cleanup EXIT

cat > "$SCENARIO" <<'JSON'
{
  "turns": [
    {"type": "text", "text": "# 统计结果\n\n统计结果：crates 下共有二百一十三个 Rust 源文件，其中一百零六个在 nini-tui。✅\n\n| 项目 | 数量 | 说明 |\n|---|---|---|\n| crates 下 .rs 文件 | 213 | 覆盖 9 个 crate |\n| nini-core/tests 的 #[test] | 80 | 含 tokio::test |\n| 宽度辅助模块 | 约 20 | 处理 CJK 与 emoji 🎉\n\n```bash\nfind crates -name '*.rs' | wc -l\n```\n\n1. 第一条：代码规模。\n2. 第二条：测试分布。\n3. 第三条：宽度计算。\n"}
  ]
}
JSON

python3 -c 'import aiohttp' 2>/dev/null || {
  echo "!! 缺 aiohttp（mock provider 需要它）：pip install aiohttp" >&2; exit 1; }

# 端口预检：沿用 capture-tui.sh 的纪律 —— 宁可失败，也不要用别人的 mock。
if ss -ltn 2>/dev/null | grep -q ":$PORT "; then
  echo "!! 端口 $PORT 已被占用，换一个：PORT=9901 bash $0" >&2; exit 1
fi

python3 "$REPO/scripts/mock-provider.py" --listen "127.0.0.1:$PORT" \
  --script "$SCENARIO" --split-bytes 7 > "$OUT/mock.log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 40); do
  ss -ltn 2>/dev/null | grep -q ":$PORT " && break
  sleep 0.25
done

BIN="${BIN:-$REPO/target/release/nini}"
tmux -L "$SOCK" kill-server 2>/dev/null
tmux -L "$SOCK" new-session -d -s "$SESS" -x "$W" -y 40 \
  "cd $REPO && OPENAI_BASE_URL=http://127.0.0.1:$PORT/v1 OPENAI_API_KEY=mock \
   $BIN --provider openai-compat --model mock-tui-model; \
   echo EXIT=\$? > $OUT/exit.txt; sleep 5"
sleep 3
printf '%s' '请把统计结果整理成表格并解释。' | tmux -L "$SOCK" load-buffer -
tmux -L "$SOCK" paste-buffer -t "$SESS"
sleep 1
tmux -L "$SOCK" send-keys -t "$SESS" Enter
for i in $(seq 1 40); do
  sleep 1
  tmux -L "$SOCK" capture-pane -p -t "$SESS" > "$OUT/f$i.txt" 2>/dev/null
  grep -q "EXIT=" "$OUT/exit.txt" 2>/dev/null && break
done
tmux -L "$SOCK" capture-pane -p -t "$SESS" > "$OUT/final.txt" 2>/dev/null
tmux -L "$SOCK" send-keys -t "$SESS" C-d; sleep 1
tmux -L "$SOCK" send-keys -t "$SESS" C-d; sleep 2
tmux -L "$SOCK" kill-server 2>/dev/null

python3 - "$OUT" "$W" <<'PY'
import sys, glob, os, unicodedata, re
out, w = sys.argv[1], int(sys.argv[2])
frames = sorted(glob.glob(os.path.join(out, "f*.txt")))
last = open(os.path.join(out, "final.txt"), encoding="utf-8", errors="replace").read()
problems = []

if "invalid utf-8" in last or any("invalid utf-8" in open(f, encoding="utf-8", errors="replace").read() for f in frames):
    problems.append("the stream died with `invalid utf-8` — a character was cut in half by a chunk boundary")

if "\ufffd" in last:
    problems.append("the screen carries U+FFFD (a replacement character)")

# 表格的竖线必须在同一列 —— 按**连续块**比较：表格是一块，输入框是另一块，
# 两块的列当然不同（把两块放在一起比，比出来的差异是假的）。
def border_cols(line):
    wpx = 0
    pos = []
    for ch in line:
        if ch in "│┌┐├┤└┘┼┬┴":
            pos.append(wpx)
        wpx += 2 if unicodedata.east_asian_width(ch) in "WF" else 1
    return tuple(pos)

group, groups = [], []
for line in last.split("\n"):
    if any(c in line for c in "│┌┐├┤└┘┼┬┴"):
        group.append(border_cols(line))
    else:
        if group:
            groups.append(group)
        group = []
if group:
    groups.append(group)

for g in groups:
    if len(set(g)) > 1:
        problems.append(f"a bordered block's columns do not line up: {sorted(set(g))}")

# 答案必须完整
if "二百一十三个" not in last or "宽度计算" not in last:
    problems.append("the answer is not complete on screen")

if problems:
    print("FAIL")
    for p in problems:
        print("  -", p)
    print("--- 屏幕 ---")
    print(last)
    sys.exit(1)
print("PASS: the answer is whole, the columns line up, no replacement characters")
PY
