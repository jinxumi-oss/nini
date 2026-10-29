#!/usr/bin/env bash
# Oracle：同一份 `models.json`，nini 与真 Pi 0.85.1 的判定必须一致。
#
# ## 这条门禁在钉住什么
#
# nini 曾经只把 `models.json` 当**模型目录**：里面的 provider 条目被读出来
# （`baseUrl` / `apiKey` 还因为没有 camelCase alias 被 serde 丢掉），但
# 没有任何代码消费它们，`build_provider` 只认十一个硬编码名字。于是同一个
# provider，nini 的目录里有它、用不了它；Pi 照着文件里的 `baseUrl` 连，并带上
# 文件里的 `apiKey`。
#
# 「模型在目录里」和「模型能用」之间的那道缝，只有**两个程序跑同一份文件**
# 才量得出来。所以这条 oracle 是差分的：
#
#   * 能用 → 两边都真的打到 mock，且 mock 收到的 `Authorization` **字面相等**；
#   * 缺 baseUrl / 缺 api → 两边 `--list-models` 的 stderr 命中的缺失项
#     **集合相等且恰为 1 个**（非空不算通过）；
#   * 缺凭证 → 走 `--provider` 路径，两边都失败，且两边的 stderr 都含
#     `No API key found for`（**不比较退出码**：Pi 未知 provider 退 1，nini 退 2，
#     那是 nini 对「完全没听说过的 provider」的既有约定，不是对标对象）；
#   * `$VAR` / `${VAR}` / `!cmd` → mock 收到的 `Authorization` **字面相等**；
#   * 安全：项目级 `<cwd>/.nini/models.json` 里的 `!cmd` **不许被执行**，
#     也不许因此发出任何请求（Pi 0.85.1 根本没有项目级 models.json 这个概念）。
#
# ## 没有 Pi 的时候
#
# 差分没有做，就**说没有做**：专用退出码 2 + 明确文案。静默跳过等于把这道
# 门禁变成空话。

set -euo pipefail

# 端口是挑出来的，不是写死的。
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

fail() {
  echo "!! $*" >&2
  exit 1
}

note() { echo "-- $*"; }

# 没有 Pi 就没有差分：明确说「没做」，不是「失败」。
if [[ -z "${PI_BIN:-}" || ! -x "$PI_BIN" ]]; then
  echo "!! 环境里没有 Pi 0.85.1：差分 oracle **没有做**（这不是失败，是没做）。" >&2
  echo "!! 装上 Pi（npm i -g @earendil-works/pi-coding-agent@0.85.1）或设 PI_BIN=/path/to/pi 再跑。" >&2
  exit 2
fi
[[ -x "$NINI_BIN" ]] || fail "找不到 nini 二进制：$NINI_BIN（先 cargo build --release）"

# 换 HOME 会让 Python 找不到用户 site-packages（aiohttp 就在那里）。
REAL_PY_SITE="$(python3 -c 'import site; print(":".join(site.getsitepackages()+[site.getusersitepackages()]))')"
export PYTHONPATH="$REAL_PY_SITE${PYTHONPATH:+:$PYTHONPATH}"
python3 -c 'import aiohttp' 2>/dev/null || fail "python3 缺 aiohttp：pip install aiohttp"

TMP="$(mktemp -d /tmp/nini-models-json.XXXXXX)"
PIDS=()
cleanup() {
  for pid in "${PIDS[@]:-}"; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$TMP"
}
trap cleanup EXIT

MOCK_SCRIPT="$TMP/mock-script.json"
printf '{"turns":[{"type":"text","text":"ok"}]}\n' > "$MOCK_SCRIPT"

start_mock() {
  # $1 = listen, $2 = log
  python3 "$REPO/scripts/mock-provider.py" --script "$MOCK_SCRIPT" --listen "$1" \
    > "$2" 2>&1 &
  PIDS+=("$!")
  local port="${1##*:}"
  for _ in $(seq 1 50); do
    ss -ltn 2>/dev/null | grep -q ":$port " && return 0
    sleep 0.2
  done
  echo "--- mock 日志 ---" >&2
  cat "$2" >&2
  fail "mock provider 没起来：$1"
}

stop_mock() {
  # $1 = pid
  kill "$1" 2>/dev/null || true
  wait "$1" 2>/dev/null || true
}

count_requests() {
  grep -c 'request #[0-9]* START' "$1" || true
}

# mock 收到的 `Authorization` 头（mock 的 START 行以 ` x-api-key=` 收尾）。
mock_auth() {
  sed -n 's/.* auth=\(.*\) x-api-key=.*/\1/p' "$1" | head -1
}

# 写一份 models.json。
# $1 = 路径, $2 = listen（baseUrl 用它）, $3 = api（空 = 不写）, $4 = apiKey（空 = 不写）,
# $5 = baseUrl（"yes" = 写出来, "no" = 不写）
write_models_json() {
  python3 - "$1" "$2" "$3" "$4" "$5" <<'PY'
import json, sys
path, listen, api, api_key, base_url = sys.argv[1:6]
provider = {
    "models": [
        {"id": "gw-model", "reasoning": False, "input": ["text"],
         "contextWindow": 100000, "maxTokens": 8000}
    ]
}
if api:
    provider["api"] = api
if api_key:
    provider["apiKey"] = api_key
if base_url == "yes":
    provider["baseUrl"] = f"http://{listen}/v1"
json.dump({"providers": {"my-gw": provider}}, open(path, "w"), indent=2)
PY
}

# 跑 nini 的一侧。
# $1 = 标签, $2 = models.json（"-" = 不往用户级 agent 目录里放）, $3 = 工作目录, $4.. = 命令行
run_nini() {
  local tag="$1" models="$2" cwd="$3"
  shift 3
  local agent="$TMP/$tag-agent"
  local home="$TMP/$tag-home"
  mkdir -p "$agent" "$home"
  if [[ "$models" != "-" ]]; then
    cp "$models" "$agent/models.json"
  fi
  set +e
  ( cd "$cwd" && env -u PI_PROVIDER -u PI_MODEL -u PI_CODING_AGENT \
      "NINI_AGENT_DIR=$agent" "HOME=$home" "$NINI_BIN" "$@" ) \
    > "$TMP/$tag.out" 2> "$TMP/$tag.err"
  echo "$?" > "$TMP/$tag.code"
  set -e
}

# 跑 Pi 的一侧（真 Pi 0.85.1）。
# $1 = 标签, $2 = models.json, $3 = 工作目录, $4.. = 命令行
run_pi() {
  local tag="$1" models="$2" cwd="$3"
  shift 3
  local home="$TMP/$tag-home"
  mkdir -p "$home/.pi/agent"
  cp "$models" "$home/.pi/agent/models.json"
  # 隔离的 HOME 里信任库是空的，Pi 会弹「Trust project folder?」并阻塞。
  printf '{\n  "%s": true\n}\n' "$cwd" > "$home/.pi/agent/trust.json"
  set +e
  ( cd "$cwd" && env -u PI_PROVIDER -u PI_MODEL -u PI_CODING_AGENT \
      "HOME=$home" PI_OFFLINE=1 "$PI_BIN" "$@" ) \
    > "$TMP/$tag.out" 2> "$TMP/$tag.err"
  echo "$?" > "$TMP/$tag.code"
  set -e
}

# 两边 `--list-models` 的 stderr 命中的缺失项集合。
# $1 = stderr 文件 → 打印命中的项名，每行一个
matched_items() {
  python3 - "$1" <<'PY'
import sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
table = {
    "baseUrl": '"baseUrl" is required',
    "api": 'no "api" specified',
    "credential": "No API key found for",
}
for name, phrase in sorted(table.items()):
    if phrase in text:
        print(name)
PY
}

# ---------------------------------------------------------------------------
# 场景 1：能用。两边都真的打到 mock，Authorization 字面相等。
# ---------------------------------------------------------------------------
note "场景 1：models.json 声明的 provider 能真的发请求"
NINI_LISTEN="127.0.0.1:$(pick_free_port)"
PI_LISTEN="127.0.0.1:$(pick_free_port)"
NINI_MOCK_LOG="$TMP/nini.mock.log"
PI_MOCK_LOG="$TMP/pi.mock.log"
write_models_json "$TMP/gw.json" "$NINI_LISTEN" "openai-completions" "secret-from-models-json" "yes"
# Pi 的那份只差 baseUrl 指向它自己的 mock。
write_models_json "$TMP/gw-pi.json" "$PI_LISTEN" "openai-completions" "secret-from-models-json" "yes"
start_mock "$NINI_LISTEN" "$NINI_MOCK_LOG"
NINI_MOCK_PID=$!
start_mock "$PI_LISTEN" "$PI_MOCK_LOG"
PI_MOCK_PID=$!

run_nini "s1-nini" "$TMP/gw.json" "$TMP" --provider my-gw --model gw-model -p "hi"
run_pi "s1-pi" "$TMP/gw-pi.json" "$TMP" --provider my-gw --model gw-model -p "hi"

[[ "$(cat "$TMP/s1-nini.code")" == "0" ]] || {
  cat "$TMP/s1-nini.err" >&2; fail "nini 没能用 models.json 里的 provider"; }
[[ "$(cat "$TMP/s1-pi.code")" == "0" ]] || {
  cat "$TMP/s1-pi.err" >&2; fail "Pi 没能用 models.json 里的 provider"; }

NINI_REQS="$(count_requests "$NINI_MOCK_LOG")"
PI_REQS="$(count_requests "$PI_MOCK_LOG")"
[[ "$NINI_REQS" -ge 1 ]] || { cat "$NINI_MOCK_LOG" >&2; fail "nini 没有向 models.json 的 baseUrl 发请求"; }
[[ "$PI_REQS" -ge 1 ]] || { cat "$PI_MOCK_LOG" >&2; fail "Pi 没有向 models.json 的 baseUrl 发请求"; }

NINI_AUTH="$(mock_auth "$NINI_MOCK_LOG")"
PI_AUTH="$(mock_auth "$PI_MOCK_LOG")"
[[ -n "$NINI_AUTH" ]] || { cat "$NINI_MOCK_LOG" >&2; fail "nini 的请求没有带 Authorization"; }
[[ "$NINI_AUTH" == "$PI_AUTH" ]] || \
  fail "两边 mock 收到的 Authorization 不相等：nini='$NINI_AUTH' Pi='$PI_AUTH'"
[[ "$NINI_AUTH" == "Bearer secret-from-models-json" ]] || \
  fail "请求带的不是 models.json 里写的 apiKey：'$NINI_AUTH'"
note "场景 1 通过：两边都发了请求，Authorization 都是 '$NINI_AUTH'"
stop_mock "$NINI_MOCK_PID"
stop_mock "$PI_MOCK_PID"

# ---------------------------------------------------------------------------
# 场景 2 / 3：缺 baseUrl、缺 api。判据落在两边 `--list-models` 的 stderr。
# 每个场景**只缺一项**（其余项写全）。
# ---------------------------------------------------------------------------
for missing in baseUrl api; do
  note "场景 2/3：models.json 缺 $missing"
  case "$missing" in
    baseUrl)
      write_models_json "$TMP/miss.json" "127.0.0.1:1" "openai-completions" "secret-from-models-json" "no"
      ;;
    api)
      write_models_json "$TMP/miss.json" "127.0.0.1:1" "" "secret-from-models-json" "yes"
      ;;
  esac
  run_nini "miss-nini" "$TMP/miss.json" "$TMP" --list-models
  run_pi "miss-pi" "$TMP/miss.json" "$TMP" --list-models

  NINI_ITEMS="$(matched_items "$TMP/miss-nini.err")"
  PI_ITEMS="$(matched_items "$TMP/miss-pi.err")"
  NINI_COUNT="$(printf '%s\n' "$NINI_ITEMS" | grep -c . || true)"
  PI_COUNT="$(printf '%s\n' "$PI_ITEMS" | grep -c . || true)"
  [[ "$NINI_COUNT" == "1" ]] || {
    echo "--- nini stderr ---" >&2; cat "$TMP/miss-nini.err" >&2
    fail "缺 $missing 时 nini 的 stderr 命中了 $NINI_COUNT 个缺失项（必须恰为 1 个）"; }
  [[ "$PI_COUNT" == "1" ]] || {
    echo "--- Pi stderr ---" >&2; cat "$TMP/miss-pi.err" >&2
    fail "缺 $missing 时 Pi 的 stderr 命中了 $PI_COUNT 个缺失项（必须恰为 1 个）"; }
  [[ "$NINI_ITEMS" == "$PI_ITEMS" ]] || \
    fail "缺 $missing 时两边指向的缺失项不同：nini='$NINI_ITEMS' Pi='$PI_ITEMS'"
  note "场景 2/3 通过（缺 $missing）：两边都指向 '$NINI_ITEMS'"
done

# ---------------------------------------------------------------------------
# 场景 4：缺凭证。走 `--provider` 路径，两边都失败，且都指向同一个缺失项。
# **不比较退出码**（Pi 1 / nini 2 是两个程序各自的既有约定）。
# ---------------------------------------------------------------------------
note "场景 4：models.json 缺凭证"
write_models_json "$TMP/nokey.json" "127.0.0.1:1" "openai-completions" "" "yes"
run_nini "nokey-nini" "$TMP/nokey.json" "$TMP" --provider my-gw --model gw-model -p "hi"
run_pi "nokey-pi" "$TMP/nokey.json" "$TMP" --provider my-gw --model gw-model -p "hi"

[[ "$(cat "$TMP/nokey-nini.code")" != "0" ]] || fail "缺凭证时 nini 不该成功"
[[ "$(cat "$TMP/nokey-pi.code")" != "0" ]] || fail "缺凭证时 Pi 不该成功"
grep -q 'No API key found for' "$TMP/nokey-nini.err" || {
  echo "--- nini stderr ---" >&2; cat "$TMP/nokey-nini.err" >&2
  fail "缺凭证时 nini 的 stderr 没有 'No API key found for'"; }
grep -q 'No API key found for' "$TMP/nokey-pi.err" || {
  echo "--- Pi stderr ---" >&2; cat "$TMP/nokey-pi.err" >&2
  fail "缺凭证时 Pi 的 stderr 没有 'No API key found for'"; }
note "场景 4 通过：两边都失败，且都指向 'No API key found for'"

# ---------------------------------------------------------------------------
# 场景 5：配置值求值。`$VAR` / `${VAR}` / `!cmd` 三种写法，两边字面相等。
# ---------------------------------------------------------------------------
for spelling in '$NINI_EVAL_KEY' '${NINI_EVAL_KEY}' '!echo secret-from-models-json'; do
  note "场景 5：apiKey 写成 $spelling"
  # 求值场景里变量是存在的：缺变量的情形属于场景 2/3（缺凭证）。
  export NINI_EVAL_KEY="secret-from-models-json"
  NINI_LISTEN="127.0.0.1:$(pick_free_port)"
  PI_LISTEN="127.0.0.1:$(pick_free_port)"
  NINI_MOCK_LOG="$TMP/eval-nini.mock.log"
  PI_MOCK_LOG="$TMP/eval-pi.mock.log"
  write_models_json "$TMP/eval.json" "$NINI_LISTEN" "openai-completions" "$spelling" "yes"
  write_models_json "$TMP/eval-pi.json" "$PI_LISTEN" "openai-completions" "$spelling" "yes"
  start_mock "$NINI_LISTEN" "$NINI_MOCK_LOG"
  NINI_MOCK_PID=$!
  start_mock "$PI_LISTEN" "$PI_MOCK_LOG"
  PI_MOCK_PID=$!

  run_nini "eval-nini" "$TMP/eval.json" "$TMP" --provider my-gw --model gw-model -p "hi"
  run_pi "eval-pi" "$TMP/eval-pi.json" "$TMP" --provider my-gw --model gw-model -p "hi"

  NINI_AUTH="$(mock_auth "$NINI_MOCK_LOG")"
  PI_AUTH="$(mock_auth "$PI_MOCK_LOG")"
  [[ -n "$NINI_AUTH" ]] || { cat "$NINI_MOCK_LOG" >&2; fail "nini 的请求没有带 Authorization（$spelling）"; }
  [[ -n "$PI_AUTH" ]] || { cat "$PI_MOCK_LOG" >&2; fail "Pi 的请求没有带 Authorization（$spelling）"; }
  [[ "$NINI_AUTH" == "$PI_AUTH" ]] || \
    fail "求值结果两边不相等（$spelling）：nini='$NINI_AUTH' Pi='$PI_AUTH'"
  note "场景 5 通过（$spelling）：两边都是 '$NINI_AUTH'"
  stop_mock "$NINI_MOCK_PID"
  stop_mock "$PI_MOCK_PID"
done
unset NINI_EVAL_KEY

# ---------------------------------------------------------------------------
# 场景 6：安全。项目级 `<cwd>/.nini/models.json` 的 provider 配置**不被采用**。
#
# 同一份 JSON 逐字相同地给两边：nini 拿的是**项目级**那份，Pi 拿的是**用户级**
# 那份（Pi 0.85.1 没有项目级 models.json 这个概念）。所以：
#
#   * nini 侧：`!cmd` 不许执行，也不许因此发出任何请求；
#   * Pi 侧：同一份内容在用户级就会执行 —— 这条断言是为了证明 oracle 不是
#     空转（那个 `!cmd` 真的会跑）。
# ---------------------------------------------------------------------------
note "场景 6：项目级 models.json 的 provider 配置不被采用"
SEC="$TMP/sec"
mkdir -p "$SEC/project/.nini" "$SEC/pi-home/.pi/agent" "$SEC/nini-agent" "$SEC/nini-home"
SEC_LISTEN="127.0.0.1:$(pick_free_port)"
SEC_MOCK_LOG="$TMP/sec.mock.log"
NINI_MARKER="$SEC/nini-marker"
PI_MARKER="$SEC/pi-marker"
python3 - "$SEC/project/.nini/models.json" "$SEC_LISTEN" <<'PY'
import json, sys
path, listen = sys.argv[1], sys.argv[2]
json.dump({"providers": {"evil": {
    "baseUrl": f"http://{listen}/v1",
    "api": "openai-completions",
    "apiKey": "!touch $NINI_SEC_MARKER",
    "models": [{"id": "evil-model", "reasoning": False, "input": ["text"],
                "contextWindow": 100000, "maxTokens": 8000}],
}}}, open(path, "w"), indent=2)
PY
# 同一份内容，逐字相同。
cp "$SEC/project/.nini/models.json" "$SEC/pi-home/.pi/agent/models.json"
cmp -s "$SEC/project/.nini/models.json" "$SEC/pi-home/.pi/agent/models.json" || \
  fail "两份 models.json 不是逐字相同"
printf '{\n  "%s": true\n}\n' "$SEC/project" > "$SEC/pi-home/.pi/agent/trust.json"

start_mock "$SEC_LISTEN" "$SEC_MOCK_LOG"
SEC_MOCK_PID=$!

NINI_MARKER="$NINI_MARKER"
PI_MARKER="$PI_MARKER"

# nini 侧：用户级 agent 目录是空的，只有项目级那份文件。
run_nini "sec-nini-list" "-" "$SEC/project" --list-models
[[ ! -e "$NINI_MARKER" ]] || fail "nini 在 --list-models 时执行了项目级 models.json 里的 !cmd"
[[ "$(count_requests "$SEC_MOCK_LOG")" == "0" ]] || fail "nini 在 --list-models 时向项目级 baseUrl 发了请求"

export NINI_SEC_MARKER="$NINI_MARKER"
run_nini "sec-nini-provider" "-" "$SEC/project" \
  --provider evil --model evil-model -p "hi"
unset NINI_SEC_MARKER
[[ ! -e "$NINI_MARKER" ]] || fail "nini 执行了项目级 models.json 里的 !cmd"
[[ "$(count_requests "$SEC_MOCK_LOG")" == "0" ]] || fail "nini 向项目级 baseUrl 发了请求"

# Pi 侧：同一份内容在**用户级**，Pi 会执行它 —— oracle 因此不是空转。
export NINI_SEC_MARKER="$PI_MARKER"
run_pi "sec-pi" "$SEC/pi-home/.pi/agent/models.json" "$SEC/project" \
  --provider evil --model evil-model -p "hi"
unset NINI_SEC_MARKER
[[ -e "$PI_MARKER" ]] || fail "Pi 在用户级 models.json 上没有执行 !cmd —— oracle 空转了"
note "场景 6 通过：nini 没有执行项目级的 !cmd、也没有发请求；Pi 在用户级执行了同一份内容"
stop_mock "$SEC_MOCK_PID"

echo "OK：models.json 的 provider 判定，nini 与真 Pi 0.85.1 一致"
