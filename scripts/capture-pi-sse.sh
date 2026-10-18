#!/usr/bin/env bash
# 链② 编排：用真 Pi 0.84.2 打出一次真实的 tool_call 流，落盘为 SSE fixture。
#
# 零污染：用 PI_CODING_AGENT_DIR 指向临时目录，只把 models.json 的 baseUrl
# 改到抓包代理上；auth.json 用符号链接，不复制凭据；用户的 ~/.pi/agent 不动。
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REAL_AGENT_DIR="${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}"
LISTEN="${LISTEN:-127.0.0.1:9878}"
UPSTREAM="${UPSTREAM:-http://127.0.0.1:9877}"
OUT="${OUT:-$REPO/captures/pi-sse}"
mkdir -p "$(dirname "$OUT")"
touch "$OUT.proxy.log"
PROMPT="${PROMPT:-Use the bash tool to run exactly: echo nini-sse-capture . Then reply with the output. Do not skip the tool call.}"

echo "== 上游：$UPSTREAM   代理：$LISTEN"
PI_BIN="${PI_BIN:-pi}"
command -v "$PI_BIN" >/dev/null 2>&1 || { echo "找不到 $PI_BIN"; exit 1; }

# --- 1. 起代理 ---------------------------------------------------------------
python3 "$REPO/scripts/capture-sse.py" --listen "$LISTEN" --upstream "$UPSTREAM" --out "$OUT" \
  > "$OUT.proxy.log" 2>&1 &
PROXY_PID=$!
cleanup() { kill "$PROXY_PID" 2>/dev/null || true; }
trap cleanup EXIT

for _ in $(seq 1 50); do
  if ss -ltn 2>/dev/null | grep -q "${LISTEN##*:}"; then break; fi
  sleep 0.2
done
ss -ltn 2>/dev/null | grep -q "${LISTEN##*:}" || { echo "代理未起来"; cat "$OUT.proxy.log"; exit 1; }
echo "== 代理已就绪 (pid $PROXY_PID)"

# --- 2. 临时 Pi 配置目录 -----------------------------------------------------
TMP="$(mktemp -d /tmp/nini-cap.XXXXXX)"
ln -sf "$REAL_AGENT_DIR/auth.json" "$TMP/auth.json"
[ -f "$REAL_AGENT_DIR/settings.json" ] && ln -sf "$REAL_AGENT_DIR/settings.json" "$TMP/settings.json"
mkdir -p "$TMP/skills"
python3 - "$REAL_AGENT_DIR/models.json" "$TMP/models.json" "$LISTEN" <<'PY'
import json, sys
src, dst, listen = sys.argv[1:4]
cfg = json.load(open(src))
for name, p in cfg.get("providers", {}).items():
    if "baseUrl" in p:
        p["baseUrl"] = p["baseUrl"].replace("127.0.0.1:9877", listen)
json.dump(cfg, open(dst, "w"), ensure_ascii=False, indent=2)
print(f"  改写 provider baseUrl → {listen}: {', '.join(cfg.get('providers', {}))}")
PY
echo "== 临时配置目录：$TMP"

# --- 3. 跑真 Pi --------------------------------------------------------------
set +e
PI_CODING_AGENT_DIR="$TMP" PI_OFFLINE=1 PI_SKIP_VERSION_CHECK=1 \
  "$PI_BIN" -p "$PROMPT" 2>&1 | tee "$OUT.pi.log"
RC=$?
set -e

sleep 0.5
echo
echo "== Pi 退出码：$RC"
echo "== 抓包产物："
ls -la "$OUT" 2>/dev/null || echo "  （无产物）"
echo "== 代理日志："
cat "$OUT.proxy.log"

rm -rf "$TMP"
echo
echo "下一步：检查 $OUT/*.sse 里是否有 tool_calls 增量事件"
