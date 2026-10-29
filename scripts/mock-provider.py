#!/usr/bin/env python3
"""脚本化的假 OpenAI provider —— TUI 对照 oracle 的上游。

## 为什么需要它

TUI 的对照 oracle 不能用真模型：模型输出不确定，录下来的画面就不可复现，
于是它顶多算"某次真实运行的样子"，不能当断言的基准。

而 Pi 只认协议不认上游：`models.json` 里一个 provider 就是
`{api: "openai-completions", baseUrl, apiKey, models}`。把 `baseUrl`
指向本进程，Pi 就会对着脚本跑。

于是同一份脚本可以同时喂给：
  * 真 Pi 0.84.2（`pi`）
  * nini（`--provider openai-compat`，同一个 baseUrl）

两边的画面差异就成了 TUI 的对照 oracle —— 确定性、零成本、可离线复算。

## 脚本格式

    {
      "turns": [
        {"type": "text",     "text": "..."},
        {"type": "thinking", "text": "..."},
        {"type": "tool_call","name": "bash", "arguments": {"command": "echo hi"}},
        {"type": "error",    "message": "..."}
      ]
    }

第 N 次请求返回 `turns[N]`（原子计数）。tool_call 之后 agent 会再发一次
请求拿最终回答，所以脚本里 tool_call 通常紧跟一条 text。

## 形态刻意与真实 provider 一致

`tool_call` 的 arguments **按 ~8 字节切块增量下发**，首块只带 `{`，
这正是 `pi-0.84.2-openai-inline-args-toolcall.sse` 里抓到的真实形态。
mock 若发完整 body，它就验证不了任何东西——链② 的全部意义就在这里。
"""

from __future__ import annotations

import argparse
import asyncio
import itertools
import json
import time
from pathlib import Path

from aiohttp import web

# 单块 arguments 的目标字节数。真实流实测约 8–40 字节。
CHUNK = 12


def sse(event: dict) -> str:
    return f"data: {json.dumps(event, ensure_ascii=False)}\n\n"


def chunk_args(raw: str) -> list[str]:
    return [raw[i : i + CHUNK] for i in range(0, len(raw), CHUNK)]


def render_turn(turn: dict, model: str, cid: str) -> str:
    now = int(time.time())
    base = {
        "id": cid,
        "object": "chat.completion.chunk",
        "created": now,
        "model": model,
    }
    out: list[str] = []

    def emit(delta: dict, finish=None) -> None:
        body = dict(base)
        body["choices"] = [{"index": 0, "delta": delta, "finish_reason": finish}]
        out.append(sse(body))

    kind = turn.get("type")

    if kind == "error":
        body = dict(base)
        body["error"] = {"message": turn.get("message", "mock error"), "type": "mock"}
        out.append(sse(body))
        return "".join(out)

    if kind == "thinking":
        # 真实流把 reasoning 放在 delta.reasoning_content
        emit({"role": "assistant"})
        for piece in _pieces(turn.get("text", "")):
            emit({"reasoning_content": piece})
        emit({}, "stop")
        return "".join(out)

    if kind == "tool_call":
        name = turn["name"]
        raw = json.dumps(turn.get("arguments", {}), ensure_ascii=False)
        emit({"role": "assistant"})
        parts = chunk_args(raw)
        for i, piece in enumerate(parts):
            fn: dict = {"arguments": piece}
            if i == 0:
                fn["name"] = name  # inline-args：name 与首段 args 同一事件
            emit(
                {
                    "tool_calls": [
                        {
                            "index": 0,
                            "type": "function",
                            # id 只在首块出现，后续增量必须继承
                            **({"id": f"call_mock_{cid[-6:]}"} if i == 0 else {}),
                            "function": fn,
                        }
                    ]
                }
            )
        emit({}, "tool_calls")
        return "".join(out)

    emit({"role": "assistant"})
    for piece in _pieces(turn.get("text", "")):
        emit({"content": piece})
    emit({}, "stop")
    return "".join(out)


def _pieces(text: str, n: int = 3) -> list[str]:
    if len(text) <= n:
        return [text] if text else []
    size = max(1, len(text) // n)
    return [text[i : i + size] for i in range(0, len(text), size)]


async def completions(request: web.Request) -> web.StreamResponse:
    app = request.app
    try:
        body = await request.json()
    except Exception:
        body = {}

    idx = next(app["counter"])
    turns = app["turns"]
    turn = turns[min(idx, len(turns) - 1)] if turns else {"type": "text", "text": ""}

    model = body.get("model") or app["model"]
    cid = f"mockcmpl{idx:04d}{int(time.time()):06d}"
    payload = render_turn(turn, model, cid)
    prompt_tokens = 40 + 6 * len(body.get("messages") or [])

    # usage-only 事件：choices 为空数组。真实流有这个形态，
    # 遍历 choices[0] 的代码在这里会 panic —— 必须保留。
    usage = {
        "id": cid,
        "object": "chat.completion.chunk",
        "created": int(time.time()),
        "model": model,
        "choices": [],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": 24,
            "total_tokens": prompt_tokens + 24,
        },
    }

    # v0.10.8: the request's **start** is stamped, not just its arrival.
    # "Two requests overlapped" is the only machine-checkable signature of a
    # second agent spawned while the first was still streaming (A1) — and
    # overlap needs both ends of both intervals, so the end is stamped too.
    started = time.time()
    # v0.13: the credential the request carried is logged too. The
    # oracle needs to say "the request carried the key the file
    # declares", and a mock that drops every header cannot answer that:
    # "no header" and "the wrong header" look the same.
    #
    # The fields are **appended**: the existing ones, and their order,
    # are what the other oracles count on.
    auth = request.headers.get("authorization", "-")
    x_api_key = request.headers.get("x-api-key", "-")
    print(
        f"[mock] request #{idx} START {time.strftime('%H:%M:%S', time.localtime(started))}."
        f"{int((started % 1) * 1000):03d} turns={len(body.get('messages') or [])}msgs "
        f"tools={len(body.get('tools') or [])} → {turn.get('type')} "
        f"auth={auth} x-api-key={x_api_key}",
        flush=True,
    )

    resp = web.StreamResponse(
        status=200,
        headers={"Content-Type": "text/event-stream", "Cache-Control": "no-cache"},
    )
    await resp.prepare(request)
    try:
        if app["split_bytes"]:
            # 真实网络在**任意字节边界**切断一个 TCP 流，包括多字节字符
            # （中文 3 字节）的中间。mock 之前把整段一次写完，所以它验证不了
            # 任何与"字符被切断"有关的东西 —— 而那正是用户在真实 provider 上
            # 遇到的失败：解析器把每个 chunk 单独解码，切断的字符直接让整个
            # 回合以 "invalid utf-8" 死掉。
            raw = payload.encode()
            for i in range(0, len(raw), app["split_bytes"]):
                await resp.write(raw[i : i + app["split_bytes"]])
                await asyncio.sleep(app["chunk_delay"])
        else:
            await resp.write(payload.encode())
            if app["chunk_delay"] > 0:
                # Even a single write takes time to be *read* by a slow client;
                # sleeping here is what makes "the turn is still streaming" a
                # real window instead of a race.
                await asyncio.sleep(app["chunk_delay"])
    except (ConnectionResetError, asyncio.CancelledError, RuntimeError) as e:
        # v0.10.8: the client hung up — that is what an interrupt looks like
        # from the server's side. Without this line the request has **no
        # end marker at all**, and a timeline with a hole in it cannot be
        # used to decide "did two turns overlap?" — the hole has to be
        # marked, or the judgement is guesswork.
        cut = time.time()
        print(
            f"[mock] request #{idx} CUT   {time.strftime('%H:%M:%S', time.localtime(cut))}."
            f"{int((cut % 1) * 1000):03d} dur={cut - started:.3f}s ({type(e).__name__})",
            flush=True,
        )
        raise
    # usage 必须是一个完整 JSON 行。曾经按字节切片后逐片加 `data:` 前缀，
    # Pi 直接报 "Could not parse message into JSON"——SSE 的分片单位是
    # **一个完整事件**，不是任意字节边界。
    await resp.write(("data: " + json.dumps(usage, ensure_ascii=False) + "\n\n").encode())
    await resp.write(b"data: [DONE]\n\n")
    await resp.write_eof()
    ended = time.time()
    print(
        f"[mock] request #{idx} END   {time.strftime('%H:%M:%S', time.localtime(ended))}."
        f"{int((ended % 1) * 1000):03d} dur={ended - started:.3f}s",
        flush=True,
    )
    return resp


async def models(request: web.Request) -> web.StreamResponse:
    return web.json_response(
        {"object": "list", "data": [{"id": request.app["model"], "object": "model"}]}
    )


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--listen", default="127.0.0.1:9880")
    ap.add_argument("--script", required=True, help="JSON 脚本路径")
    ap.add_argument("--model", default="mock-tui-model")
    ap.add_argument(
        "--split-bytes",
        type=int,
        default=0,
        help="write the stream in chunks of N bytes (0 = one write). A non-zero "
        "value cuts multi-byte characters in half, the way a real network does.",
    )
    ap.add_argument(
        "--chunk-delay",
        type=float,
        default=0.002,
        help="seconds to sleep between chunks. This is what makes a turn "
        "last long enough to be interrupted (the e2e interrupt test needs a "
        "window of seconds, not milliseconds).",
    )
    args = ap.parse_args()

    turns = json.loads(Path(args.script).read_text())["turns"]
    host, _, port = args.listen.partition(":")

    app = web.Application()
    app["turns"] = turns
    app["model"] = args.model
    app["counter"] = itertools.count()
    app["split_bytes"] = args.split_bytes
    app["chunk_delay"] = args.chunk_delay
    app.router.add_post("/v1/chat/completions", completions)
    app.router.add_get("/v1/models", models)

    print(f"mock provider → http://{args.listen}  {len(turns)} turns", flush=True)
    for i, t in enumerate(turns):
        print(f"  [{i}] {t.get('type')}: {str(t.get('text') or t.get('name') or '')[:60]}")
    web.run_app(app, host=host, port=int(port), print=None)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
