// Pi 0.85.1 的**截断**黄金值：执行 Pi 自己的 truncate.js，把结果冻成 JSON。
//
// 为什么
// ----
// 截断是「工具交还给模型多少东西」的唯一决定因素，也是长任务里上下文
// 构成的直接来源。nini 此前是**各工具各写各的**（bash 1 MiB/2000 行、
// grep 按行数、read 直接拒绝大文件）——所以「同一条命令在两侧占多少上下文」
// 差着二十倍。
//
// 对齐它需要**同一份期望值**。手算不行（UTF-8 边界、行数/字节两个独立
// 上限、tail 的半行情形……），所以：执行 Pi 的代码，把结果冻进仓库，
// Rust 侧再拿自己的实现去对那份 JSON。
//
// 用法：
//   node scripts/truncate-oracle.mjs \
//     --pi-root /home/jin/.nvm/versions/node/v25.9.0/lib/node_modules/@earendil-works/pi-coding-agent \
//     --out references/pi-0.85.1/truncate-oracle.json
//
// 退出码：0 成功；2 环境不可用。

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const args = process.argv.slice(2);
const arg = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
};

const piRoot = arg(
  "--pi-root",
  "/home/jin/.nvm/versions/node/v25.9.0/lib/node_modules/@earendil-works/pi-coding-agent",
);
const out = arg("--out", "references/pi-0.85.1/truncate-oracle.json");

let mod;
try {
  mod = await import(
    pathToFileURL(resolve(piRoot, "dist/core/tools/truncate.js")).href
  );
} catch (e) {
  console.error(`无法加载 Pi 的 truncate 模块：${e.message}`);
  process.exit(2);
}
const { truncateHead, truncateTail, truncateLine, formatSize, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES, GREP_MAX_LINE_LENGTH } =
  mod;

// ── 样本 ────────────────────────────────────────────────────────────────
const asciiLines = Array.from({ length: 30 }, (_, i) => `line ${i} content padding`);
const cases = [
  {
    id: "short",
    content: "a\nb\nc\n",
  },
  {
    id: "exactly-at-limits",
    content: "x".repeat(100) + "\n",
  },
  {
    id: "many-lines",
    content: asciiLines.join("\n") + "\n",
    maxLines: 5,
  },
  {
    id: "byte-limit-head",
    content: "y".repeat(500) + "\n" + asciiLines.join("\n") + "\n",
    maxBytes: 300,
  },
  {
    id: "first-line-exceeds",
    content: "z".repeat(900) + "\n" + "tail line\n",
    maxBytes: 100,
  },
  {
    id: "utf8-boundary",
    content: "中".repeat(200) + "\n" + asciiLines.slice(0, 3).join("\n") + "\n",
    maxBytes: 301,
  },
  {
    id: "tail-byte-limit",
    content: asciiLines.join("\n") + "\n" + "z".repeat(900) + "\n",
    maxBytes: 400,
  },
  {
    id: "no-trailing-newline",
    content: "one\ntwo\nthree",
  },
];

const results = [];
for (const c of cases) {
  const opts = {};
  if (c.maxLines !== undefined) opts.maxLines = c.maxLines;
  if (c.maxBytes !== undefined) opts.maxBytes = c.maxBytes;
  const head = truncateHead(c.content, opts);
  const tail = truncateTail(c.content, opts);
  results.push({
    id: c.id,
    input: c.content,
    maxLines: c.maxLines ?? DEFAULT_MAX_LINES,
    maxBytes: c.maxBytes ?? DEFAULT_MAX_BYTES,
    head: {
      content: head.content,
      truncated: head.truncated,
      truncatedBy: head.truncatedBy,
      outputLines: head.outputLines,
      outputBytes: head.outputBytes,
      totalLines: head.totalLines,
      totalBytes: head.totalBytes,
      firstLineExceedsLimit: head.firstLineExceedsLimit,
    },
    tail: {
      content: tail.content,
      truncated: tail.truncated,
      truncatedBy: tail.truncatedBy,
      outputLines: tail.outputLines,
      outputBytes: tail.outputBytes,
      totalLines: tail.totalLines,
      totalBytes: tail.totalBytes,
      lastLinePartial: tail.lastLinePartial,
    },
  });
}

// 单行截断
const lineCases = [
  { id: "short-line", line: "hello", maxChars: 500 },
  { id: "exact-line", line: "a".repeat(500), maxChars: 500 },
  { id: "long-line", line: "b".repeat(700), maxChars: 500 },
  { id: "utf8-line", line: "中".repeat(300), maxChars: 500 },
  { id: "custom-limit", line: "c".repeat(50), maxChars: 10 },
];
const lineResults = lineCases.map((c) => {
  const r = truncateLine(c.line, c.maxChars);
  return { id: c.id, input: c.line, maxChars: c.maxChars, text: r.text, wasTruncated: r.wasTruncated };
});

const payload = {
  piVersion: "0.85.1",
  generatedBy: "scripts/truncate-oracle.mjs（执行 Pi 自己的 truncate.js）",
  constants: {
    DEFAULT_MAX_LINES,
    DEFAULT_MAX_BYTES,
    GREP_MAX_LINE_LENGTH,
  },
  formatSize: [
    { bytes: 512, text: formatSize(512) },
    { bytes: 51200, text: formatSize(51200) },
    { bytes: 1048576, text: formatSize(1048576) },
  ],
  cases: results,
  lineCases: lineResults,
};

const outPath = resolve(process.cwd(), out);
mkdirSync(dirname(outPath), { recursive: true });
writeFileSync(outPath, JSON.stringify(payload, null, 2) + "\n", "utf-8");
console.log(
  `截断黄金值 → ${out}（${results.length} 个内容样本 + ${lineResults.length} 个单行样本）`,
);
