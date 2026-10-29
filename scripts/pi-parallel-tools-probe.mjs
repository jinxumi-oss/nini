// Pi 0.85.1 的**工具并发峰值**探针。
//
// 为什么需要它
// ------------
// nini 给一次 run 的工具并发设过上限（默认 8），Pi 0.85.1 没有：它的并行
// 分支是无上限的 `Promise.all`
// （`references/pi-0.85.1/src/loop/agent-loop.ts:547-549`）。这个差异只能靠
// **两个程序跑同一份剧本**量得出来——「都跑完了」分不出「8 个同时跑」和
// 「20 个同时跑」。
//
// 剧本
// ----
// 一轮里发 K 个 tool_call，每个工具睡固定 `--sleep-ms` 毫秒，工具内部数自己
// 的 in-flight 峰值。两侧跑的是**同一个剧本**（同样的 K、同样的 sleep）。
//
// 峰值为什么等于 K（与 sleep 长短无关）
// ------------------------------------
// Pi 的并行分支在**同一个同步 map** 里调用每个工具：`Promise.all(finalizedCalls.map(...))`
// （`agent-loop.ts:547-549`）。map 的回调是同步的，于是每个 `tool.execute(...)`
// 在自己的第一个 await 之前就进了工具体——20 个 in-flight 在同一拍里同时存在。
// 所以 peak 恒等于 K，**不取决于 sleep 有多长**；sleep 只用来保证 in-flight
// 窗口不塌（换成真 bash 那种要 fork 的工具时才有意义）。
//
// 输出
// ----
//   `PI_PEAK=<n>`
//
// 退出码：0 成功；2 环境不可用（没装 node / 找不到 Pi 0.85.1 / 加载失败）。

import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const args = process.argv.slice(2);
function arg(name, fallback) {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
}

// 取包路径的顺序：**环境变量 PI_ROOT → 命令行 --pi-root → 本机 nvm 默认路径**。
// 默认路径照 `scripts/pi-tool-dump.mjs` 的写法。
const PI_ROOT =
  process.env.PI_ROOT ||
  arg(
    "--pi-root",
    "/home/jin/.nvm/versions/node/v25.9.0/lib/node_modules/@earendil-works/pi-coding-agent",
  );

const CALLS = Number(arg("--calls", "20"));
const SLEEP_MS = Number(arg("--sleep-ms", "60"));

async function importFromPi(relPath, what) {
  const modPath = resolve(PI_ROOT, relPath);
  try {
    return await import(pathToFileURL(modPath).href);
  } catch (e) {
    console.error(`无法加载 Pi 的${what}：${modPath}\n${e.message}`);
    console.error("（本机 Pi 必须是 0.85.1；否则快照与运行时不符）");
    process.exit(2);
  }
}

const { runAgentLoop } = await importFromPi(
  "node_modules/@earendil-works/pi-agent-core/dist/agent-loop.js",
  "agent loop 模块",
);
const { createAssistantMessageEventStream } = await importFromPi(
  "node_modules/@earendil-works/pi-ai/dist/utils/event-stream.js",
  "事件流模块",
);

// ---------------------------------------------------------------------------
// 假工具：数自己有多少个 body 同时在跑。
// ---------------------------------------------------------------------------
let live = 0;
let peak = 0;

const probeTool = {
  name: "probe",
  description: "Counts peak concurrency (probe only).",
  parameters: { type: "object", properties: {}, additionalProperties: false },
  label: "Probe",
  async execute() {
    live += 1;
    if (live > peak) peak = live;
    await new Promise((r) => setTimeout(r, SLEEP_MS));
    live -= 1;
    return { content: [{ type: "text", text: "probe done" }], details: {} };
  },
};

// ---------------------------------------------------------------------------
// 假流：注入 K 个 tool_call。
// ---------------------------------------------------------------------------
const USAGE = {
  input: 0,
  output: 0,
  cacheRead: 0,
  cacheWrite: 0,
  totalTokens: 0,
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
};

let turn = 0;
function fakeStreamFn() {
  turn += 1;
  const stream = createAssistantMessageEventStream();
  const content =
    turn === 1
      ? Array.from({ length: CALLS }, (_, i) => ({
          type: "toolCall",
          id: `call_${i}`,
          name: "probe",
          arguments: {},
        }))
      : [];
  const message = {
    role: "assistant",
    content,
    api: "openai-completions",
    provider: "openai",
    model: "probe-model",
    usage: USAGE,
    stopReason: turn === 1 ? "toolUse" : "stop",
    timestamp: Date.now(),
  };
  stream.push({ type: "start", partial: message });
  stream.push({ type: "done", message });
  return stream;
}

// ---------------------------------------------------------------------------
// 跑一轮。
// ---------------------------------------------------------------------------
const userMessage = {
  role: "user",
  content: [{ type: "text", text: "go" }],
  timestamp: Date.now(),
};

const config = {
  model: {
    id: "probe-model",
    name: "probe-model",
    api: "openai-completions",
    provider: "openai",
    baseUrl: "",
    reasoning: false,
    input: [],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    contextWindow: 0,
    maxTokens: 0,
  },
  convertToLlm: (messages) =>
    messages.filter(
      (m) => m.role === "user" || m.role === "assistant" || m.role === "toolResult",
    ),
  // Pi 的默认就是 parallel（`agent-loop.js:134` 的 `toolExecution ?? "parallel"`）。
  // 这里显式写出来，是为了让「本探针量的是并发」这件事不依赖默认值。
  toolExecution: "parallel",
};

await runAgentLoop(
  [userMessage],
  { systemPrompt: "", messages: [], tools: [probeTool] },
  config,
  () => {},
  undefined,
  fakeStreamFn,
);

console.log(`PI_PEAK=${peak}`);
