//! 工具契约与 **Pi 0.85.1** 的逐条比对。
//!
//! 真相不是「Pi 的源码」，而是 `references/pi-0.85.1/tool-contracts.json` ——
//! 那是 `scripts/pi-tool-dump.mjs` **执行 Pi 自己的代码**产出的**运行时值**。
//!
//! 为什么必须是运行时值：Pi 的工具描述是模板字符串，源码里留着占位符
//! （`Output is truncated to ${DEFAULT_MAX_BYTES / 1024}KB`）。拿带占位符的
//! 源码去比 nini 的 Rust 字面量，比的是**两种书写形式**，不是同一份契约——
//! 那种 oracle 是假的，会在两边都"看起来对"的时候悄悄放过真正的差异。
//!
//! 比什么：
//!   1. `description` —— 模型看到的那句话，逐字
//!   2. 参数集：名字、类型、**每个参数自己的描述**（也是模型看的）
//!   3. `required`
//!   4. `promptSnippet`（进 system prompt 的 "Available tools" 段）
//!   5. `promptGuidelines`（进 "Guidelines" 段）
//!
//! 不比什么：实现细节（nini 是 Rust，Pi 是 TS）、渲染外观、错误文案。
//!
//! `powershell` 是**有意不做**的（Windows 专用），在 README 的 known
//! limitations 里申报；因此它不在比对集合里——但它必须被显式排除，而不是
//! 「碰巧没实现」。

use nini_core::tool::Tool;
use nini_tools::{BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool, WriteTool};
use std::sync::Arc;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../references/pi-0.85.1/tool-contracts.json"
);

/// Pi 有、nini **有意不做**的工具（README 的 known limitations 里申报）。
const EXCLUDED: &[(&str, &str)] = &[(
    "powershell",
    "Windows 专用；nini 不提供，见 README known limitations",
)];

fn fixture() -> serde_json::Value {
    let raw = std::fs::read_to_string(FIXTURE)
        .unwrap_or_else(|e| panic!("读不到 Pi 的工具契约 fixture：{FIXTURE}（{e}）"));
    serde_json::from_str(&raw).expect("fixture 不是合法 JSON")
}

fn nini_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(BashTool::new()),
        Arc::new(ReadTool::new()),
        Arc::new(WriteTool::new()),
        Arc::new(EditTool::new()),
        Arc::new(GrepTool::new()),
        Arc::new(FindTool::new()),
        Arc::new(LsTool::new()),
    ]
}

#[test]
fn every_tool_contract_matches_pi_0_85_1() {
    let fx = fixture();
    let pi_tools = fx["tools"]
        .as_object()
        .expect("fixture 的 tools 必须是对象");

    for tool in nini_tools() {
        let name = tool.name();
        let pi = pi_tools
            .get(name)
            .unwrap_or_else(|| panic!("Pi 0.85.1 没有工具 {name}——nini 多了一个工具？"));
        let spec = tool.spec();

        // 1. description：逐字
        assert_eq!(
            spec.description,
            pi["description"].as_str().unwrap_or_default(),
            "工具 {name} 的 description 与 Pi 0.85.1 不一致（这是模型直接看到的那句话）"
        );

        // 2. 参数集：名字、类型、每个参数自己的描述
        let props = spec.input_schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} 的 input_schema 没有 properties"));
        let pi_props = pi["params"].as_object().expect("fixture 的 params");

        let nini_names: Vec<&String> = props.keys().collect();
        let pi_names: Vec<&String> = pi_props.keys().collect();
        assert_eq!(
            nini_names, pi_names,
            "工具 {name} 的参数集与 Pi 0.85.1 不一致（顺序也算：模型按顺序读）"
        );

        for pname in pi_names {
            let np = &props[pname];
            let pp = &pi_props[pname];
            assert_eq!(
                np["type"].as_str().unwrap_or_default(),
                pp["type"].as_str().unwrap_or_default(),
                "工具 {name} 的参数 {pname} 的类型与 Pi 0.85.1 不一致"
            );
            assert_eq!(
                np["description"].as_str().unwrap_or_default(),
                pp["description"].as_str().unwrap_or_default(),
                "工具 {name} 的参数 {pname} 的描述与 Pi 0.85.1 不一致"
            );
        }

        // 3. required
        let nini_required: Vec<&str> = spec.input_schema["required"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let pi_required: Vec<&str> = pi["required"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(
            nini_required, pi_required,
            "工具 {name} 的 required 与 Pi 0.85.1 不一致"
        );

        // 4. promptSnippet：进 system prompt 的 "Available tools" 段
        let contrib = tool.system_prompt_contribution();
        let nini_snippet = contrib.as_ref().map(|c| c.snippet.as_str()).unwrap_or("");
        assert_eq!(
            nini_snippet,
            pi["promptSnippet"].as_str().unwrap_or_default(),
            "工具 {name} 的 promptSnippet 与 Pi 0.85.1 不一致（它决定模型在 \
             \"Available tools\" 里看到什么）"
        );

        // 5. promptGuidelines：进 "Guidelines" 段
        let nini_guidelines: Vec<&str> = contrib
            .as_ref()
            .map(|c| c.guidelines.iter().map(|g| g.as_str()).collect())
            .unwrap_or_default();
        let pi_guidelines: Vec<&str> = pi["promptGuidelines"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(
            nini_guidelines, pi_guidelines,
            "工具 {name} 的 promptGuidelines 与 Pi 0.85.1 不一致"
        );
    }
}

#[test]
fn nini_has_every_pi_tool_except_the_documented_exclusions() {
    let fx = fixture();
    let pi_tools = fx["tools"]
        .as_object()
        .expect("fixture 的 tools 必须是对象");
    let nini_names: Vec<String> = nini_tools().iter().map(|t| t.name().to_string()).collect();

    for name in pi_tools.keys() {
        if EXCLUDED.iter().any(|(n, _)| n == name) {
            assert!(
                !nini_names.contains(name),
                "{name} 被列为「有意不做」，但 nini 实现了它——要么改 README，要么改这里"
            );
            continue;
        }
        assert!(
            nini_names.contains(name),
            "Pi 0.85.1 有工具 {name}，nini 没有——这是 parity 缺口（台账 B11）"
        );
    }

    for name in &nini_names {
        assert!(
            pi_tools.contains_key(name),
            "nini 有工具 {name}，Pi 0.85.1 没有——这是 nini 的扩展，不是 parity"
        );
    }
}

#[test]
fn the_excluded_tools_are_still_declared() {
    // 「有意不做」必须被**显式**登记，而不是碰巧没实现：
    // 如果 Pi 将来加了新工具而这里没更新，测试会失败（上面的用例）。
    let fx = fixture();
    let pi_tools = fx["tools"]
        .as_object()
        .expect("fixture 的 tools 必须是对象");
    for name in pi_tools.keys() {
        let excluded = EXCLUDED.iter().any(|(n, _)| n == name);
        if !excluded {
            continue;
        }
        assert!(
            !name.is_empty(),
            "排除项必须写明理由（README known limitations 要有对应的一条）"
        );
    }
}
