// Pi 0.85.1 工具契约的**运行时**转储。
//
// 为什么必须跑起来而不是读源码
// ------------------------------
// Pi 的工具描述是**模板字符串**，里面留着占位符：
//
//   `Output is truncated to ${DEFAULT_LIMIT} entries or ${DEFAULT_MAX_BYTES / 1024}KB`
//
// 直接读源码拿到的是带 `${...}` 的字面量，不是模型真正看到的那句话。
// 而 nini 侧存的是 Rust 字符串字面量。拿"带占位符的源码"和"Rust 字面量"比对，
// 比的是**两种书写形式**，不是**同一份契约**——那样的 oracle 是假的。
//
// 所以：执行 Pi 自己的代码，把**运行时值**转储成 JSON，冻进仓库；
// Rust 侧再拿自己的 spec 去对那份 JSON。
//
// 用法：
//   node scripts/pi-tool-dump.mjs \
//     --pi-root /home/jin/.nvm/versions/node/v25.9.0/lib/node_modules/@earendil-works/pi-coding-agent \
//     --out references/pi-0.85.1/tool-contracts.json
//
// 退出码：0 成功；2 环境不可用（没装 node / 找不到 Pi 包）。

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const args = process.argv.slice(2);
function arg(name, fallback) {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
}

const piRoot = arg(
  "--pi-root",
  "/home/jin/.nvm/versions/node/v25.9.0/lib/node_modules/@earendil-works/pi-coding-agent",
);
const out = arg("--out", "references/pi-0.85.1/tool-contracts.json");

const modPath = resolve(piRoot, "dist/core/tools/index.js");
let mod;
try {
  mod = await import(pathToFileURL(modPath).href);
} catch (e) {
  console.error(`无法加载 Pi 的工具模块：${modPath}\n${e.message}`);
  console.error("（本机 Pi 必须是 0.85.1；否则快照与运行时不符）");
  process.exit(2);
}

// typebox 的 schema 在运行时是一个普通对象（type 是字符串，[Kind] 是 symbol，
// JSON 序列化会丢掉 symbol）——我们要的正是**可序列化的那部分**：
// 参数名、类型、描述、默认值、是否必填。
function dumpSchema(schema) {
  if (!schema || typeof schema !== "object") return { params: {}, required: [] };
  const props = schema.properties ?? {};
  const params = {};
  for (const [key, value] of Object.entries(props)) {
    const entry = {
      type: typeof value?.type === "string" ? value.type : null,
      description: value?.description ?? null,
      default: value?.default ?? null,
      minimum: value?.minimum ?? null,
      maximum: value?.maximum ?? null,
    };
    // 嵌套一层：edit 的 edits[] 是数组，元素里还有 oldText/newText 的描述——
    // 那些描述同样给模型看，同样要比对。
    if (value?.items && typeof value.items === "object") {
      const itemProps = value.items.properties ?? {};
      const items = {};
      for (const [ik, iv] of Object.entries(itemProps)) {
        items[ik] = {
          type: typeof iv?.type === "string" ? iv.type : null,
          description: iv?.description ?? null,
        };
      }
      entry.items = items;
      entry.itemsRequired = value.items.required ?? [];
    }
    params[key] = entry;
  }
  return { params, required: schema.required ?? [] };
}

const names = [...mod.allToolNames].sort();
const tools = {};
for (const name of names) {
  let def;
  try {
    def = mod.createToolDefinition(name, "/tmp", {});
  } catch (e) {
    console.error(`工具 ${name} 的定义创建失败：${e.message}`);
    process.exit(2);
  }
  const { params, required } = dumpSchema(def.parameters);
  tools[name] = {
    name: def.name,
    label: def.label ?? null,
    description: def.description ?? null,
    promptSnippet: def.promptSnippet ?? null,
    promptGuidelines: def.promptGuidelines ?? [],
    renderShell: def.renderShell ?? null,
    hasCustomRender: typeof def.renderCall === "function" || typeof def.renderResult === "function",
    params,
    required,
  };
}

const payload = {
  piVersion: "0.85.1",
  generatedBy: "scripts/pi-tool-dump.mjs（执行 Pi 自己的代码，取运行时值）",
  note:
    "description 是**插值后的运行时值**（源码里的 ${...} 已被替换）。" +
    "promptSnippet 为 null 的工具，按 Pi 的规则不会出现在 system prompt 的 " +
    "\"Available tools\" 段里；promptGuidelines 进 \"Guidelines\" 段。" +
    "renderShell/hasCustomRender 记录 Pi 的渲染归属（内置工具统一渲染，扩展工具可自带）。",
  tools,
};

const outPath = resolve(process.cwd(), out);
mkdirSync(dirname(outPath), { recursive: true });
writeFileSync(outPath, JSON.stringify(payload, null, 2) + "\n", "utf-8");
console.log(`工具契约（运行时）→ ${out}`);
for (const [name, t] of Object.entries(tools)) {
  console.log(
    `  ${name.padEnd(12)} snippet=${t.promptSnippet ? "有" : "无"}  ` +
      `params=${Object.keys(t.params).join(",") || "-"}`,
  );
}
