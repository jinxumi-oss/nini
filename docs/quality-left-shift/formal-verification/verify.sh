#!/usr/bin/env bash
# 阶段 3 局部形式化验证一键脚本：Lean 证明 + PlusPy 状态枚举
cd "$(dirname "$0")"

LEAN=/home/jin/toolchains/lean/bin/lean
PLUSPY_PY=/home/jin/toolchains/pluspy-venv/bin/python
PLUSPY=/home/jin/toolchains/tla-pluspy/pluspy.py
TLA_DIR=/home/jin/nini/docs/quality-left-shift/formal-verification
TOOLCHAIN=/home/jin/toolchains/tla-pluspy

echo "== Lean: concurrency-invariant.lean (证明无丢失更新不变量) =="
"$LEAN" concurrency-invariant.lean
echo "lean exit code: $?"

echo "== PlusPy: state-spec-shared-state.tla (-c 20 枚举状态) =="
( cd "$TOOLCHAIN" && "$PLUSPY_PY" "$PLUSPY" -c 20 "$TLA_DIR/state-spec-shared-state.tla" )
echo "pluspy shared-state exit code: $?"

echo "== PlusPy: state-spec-truncation-state-machine.tla (-c 20 枚举状态) =="
( cd "$TOOLCHAIN" && "$PLUSPY_PY" "$PLUSPY" -c 20 "$TLA_DIR/state-spec-truncation-state-machine.tla" )
echo "pluspy truncation exit code: $?"
