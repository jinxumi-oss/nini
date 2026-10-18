#!/usr/bin/env python3
"""链② 抓包器：在 Pi 与上游 provider 之间做流式透传，并逐事件落盘 SSE。

## 为什么需要它

nini 的 wire 测试 fixture 是手搓的（`nini-ai/tests/openai_wire_e2e.rs:18`
自己写着 "hand-built"）。手搓 fixture 查不出的 bug 恰恰是这一类：

    c3944d13  首个 chunk 的 tool_call args 被丢弃
    7a71f5b3  inline-args tool_call 只发 Start 不发 Stop
    9c9783e3  single-shot 与 streaming 各发一次 Start

因为"手搓者按自己以为的格式造样例"——样例和解析器共享同一个误解。
唯一能打破这个循环的，是**真实 provider 发出的原始 SSE 行**。

## 它做什么

    Pi  ──►  本代理 :9878  ──►  上游 :9877
             └─ 逐 SSE 事件落盘 + 记录请求体（含 tools 定义）

落盘粒度是**单个 SSE `data:` 事件**，不是字节流——因为 tool_call 的
`function.arguments` 是跨事件增量拼接的，事件边界就是那 4 条缺陷的证据。

## 凭据处理

**不落盘任何 Authorization / api key**。请求头只记录名称不记录值。
抓包目录应当可以安全留在工作区内。

## 用法

    python3 scripts/capture-sse.py --out captures/run1 &     # 起代理
    PI_CODING_AGENT_DIR=/tmp/nini-cap pi -p "..."            # 跑真 Pi
    kill %1

--upstream 默认读本机 provider 的实际 baseUrl，不要猜。
"""

from __future__ import annotations

import argparse
import asyncio
import json
import time
from pathlib import Path

from aiohttp import ClientSession, ClientTimeout, web

# 这些头由代理自己处理或不该跨跳转发
HOP_BY_HOP = {
    "host", "connection", "content-length", "transfer-encoding",
    "keep-alive", "upgrade", "proxy-authorization", "te", "trailer",
}
SECRET_HEADERS = {"authorization", "x-api-key", "api-key", "cookie"}


class Capture:
    def __init__(self, out: Path):
        self.dir = out
        self.dir.mkdir(parents=True, exist_ok=True)
        self.n = 0

    def next_id(self) -> str:
        self.n += 1
        return f"{self.n:03d}"


def safe_headers(headers) -> dict:
    """只留头名称；任何可能带凭据的头一律替换成 <redacted>。"""
    out = {}
    for k, v in headers.items():
        out[k] = "<redacted>" if k.lower() in SECRET_HEADERS else v
    return out


async def handler(request: web.Request) -> web.StreamResponse:
    cap: Capture = request.app["cap"]
    sid = cap.next_id()
    upstream = request.app["upstream"].rstrip("/") + request.rel_url.path_qs
    body = await request.read()

    if body:
        try:
            parsed = json.loads(body)
            cap.dir.joinpath(f"{sid}.request.json").write_text(
                json.dumps(parsed, ensure_ascii=False, indent=2), encoding="utf-8"
            )
        except json.JSONDecodeError:
            cap.dir.joinpath(f"{sid}.request.raw").write_bytes(body)

    hdrs = {
        k: v for k, v in request.headers.items()
        if k.lower() not in HOP_BY_HOP
    }
    hdrs["host"] = request.app["upstream"].split("//")[-1]

    session: ClientSession = request.app["session"]
    print(f"[capture {sid}] → {request.method} {upstream}", flush=True)

    async with session.request(
        request.method, upstream, headers=hdrs, data=body or None,
        timeout=ClientTimeout(total=None, sock_connect=30, sock_read=300),
    ) as up:
        ctype = up.headers.get("Content-Type", "")
        is_sse = "text/event-stream" in ctype

        (cap.dir / f"{sid}.meta.json").write_text(
            json.dumps(
                {
                    "upstream": upstream,
                    "request_headers": safe_headers(request.headers),
                    "response_status": up.status,
                    "response_content_type": ctype,
                    "is_sse": is_sse,
                    "started": time.time(),
                },
                ensure_ascii=False,
                indent=2,
            ),
            encoding="utf-8",
        )

        resp = web.StreamResponse(
            status=up.status,
            headers={
                k: v for k, v in up.headers.items()
                if k.lower() not in HOP_BY_HOP
            },
        )
        await resp.prepare(request)

        if not is_sse:
            payload = await up.read()
            (cap.dir / f"{sid}.body").write_bytes(payload)
            await resp.write(payload)
            await resp.write_eof()
            return resp

        # SSE：逐事件落盘，保留事件边界
        events: list[str] = []
        buf = ""
        n_ev = 0
        async for chunk in up.content.iter_any():
            await resp.write(chunk)
            buf += chunk.decode("utf-8", errors="replace")
            while "\n" in buf:
                line, buf = buf.split("\n", 1)
                events.append(line)
                if line.startswith("data:"):
                    n_ev += 1
        if buf:
            events.append(buf)

        text = "\n".join(events)
        (cap.dir / f"{sid}.sse").write_text(text, encoding="utf-8")
        cap.dir.joinpath(f"{sid}.events.json").write_text(
            json.dumps(
                [l[len("data:"):].strip() for l in events if l.startswith("data:")],
                ensure_ascii=False,
                indent=2,
            ),
            encoding="utf-8",
        )
        print(f"[capture {sid}] SSE {n_ev} events → {cap.dir}", flush=True)
        await resp.write_eof()
        return resp


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--listen", default="127.0.0.1:9878")
    ap.add_argument("--upstream", default="http://127.0.0.1:9877")
    ap.add_argument("--out", default="captures")
    args = ap.parse_args()

    host, _, port = args.listen.partition(":")
    out = Path(args.out).resolve()
    if out.exists() and any(out.iterdir()):
        stamp = time.strftime("%Y%m%d-%H%M%S")
        out = out.parent / f"{out.name}-{stamp}"
    out.mkdir(parents=True, exist_ok=True)

    app = web.Application(client_max_size=256 * 1024 * 1024)
    app["cap"] = Capture(out)
    app["upstream"] = args.upstream
    app.router.add_route("*", "/{tail:.*}", handler)

    async def _start(app_):
        app_["session"] = ClientSession(
            auto_decompress=False,
            timeout=ClientTimeout(total=None),
        )

    app.on_startup.append(_start)

    print(f"抓包代理 → http://{args.listen}（上游 {args.upstream}）", flush=True)
    print(f"落盘目录：{out}", flush=True)
    web.run_app(app, host=host, port=int(port), print=None)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
