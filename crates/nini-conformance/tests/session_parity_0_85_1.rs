//! Oracle: nini 必须能读 **Pi 0.85.1** 写的会话文件，一行不丢。
//!
//! # 这个 fixture 与旧的有什么不同
//!
//! 旧的 `pi-0.84.2-session.jsonl` 是 0.84.2 录的，而且**洗过值**：每个 entry id
//! 被换成 8 位哈希，却**没有**把改动同步到 `parentId`——于是父子链大面积悬空，
//! 它只能证明**行级 schema**，证明不了**图结构**（旧 fixture 自己的测试
//! `the_fixture_cannot_prove_anything_about_the_graph` 就把这件事钉住了）。
//!
//! 这个 fixture 是 **0.85.1** 录的，由 `scripts/pi-session-record.sh` 用
//! **脚本化的 mock provider 驱动真 Pi** 产生：**只洗路径**（cwd），**id 与 parentId
//! 原样保留**。所以它同时证明两件事：
//!
//! 1. **行级 schema**：每一条 Pi 写的行，nini 都要能解析；
//! 2. **图结构**：父子链必须**能解析**——这正是 nini 至今没做到的部分。
//!
//! 而且它包含**完整的回合形状**：user → assistant(toolCall) → toolResult →
//! assistant(text) → user → assistant(text)。工具调用与工具结果**在 Pi 的会话里
//! 是留痕的**——nini 自己的 P0.1 记录的是「nini 写的时候丢它们」，
//! 这里证明的是**读**的时候不能丢。
//!
//! # 复算
//!
//! ```console
//! bash scripts/pi-session-record.sh --out /tmp/check.jsonl
//! diff <(python3 -c "import json,sys;[print(json.dumps(json.loads(l),sort_keys=True)) for l in open(sys.argv[1]) if l.strip()]" /tmp/check.jsonl) \
//!      <(python3 -c "import json,sys;[print(json.dumps(json.loads(l),sort_keys=True)) for l in open(sys.argv[1]) if l.strip()]" crates/nini-conformance/fixtures/pi-0.85.1-session.jsonl)
//! ```
//!
//! 时间戳与 id 每次录制都会不同，所以上面比的是**结构**；要逐字节相同，
//! 就得连时间戳一起洗——而那正是这个 fixture 要避免的（洗掉 id 会毁掉图证据）。

use nini_conformance::{diagnose, fixture, summarise};

#[test]
fn reads_every_line_of_a_real_pi_0_85_1_session() {
    let path = fixture("pi-0.85.1-session.jsonl");
    assert!(path.exists(), "fixture missing: {}", path.display());

    let (verdicts, _) = diagnose(&path);
    let (total, failed, table) = summarise(&verdicts);

    assert!(
        failed == 0,
        "nini dropped {failed}/{total} lines of a session **Pi 0.85.1** wrote.\n\
         This is not a malformed-input case: every line below was written by Pi itself.\n\n\
         failures by Pi entry type:\n{}",
        if table.is_empty() {
            "  (none)".to_string()
        } else {
            table
                .iter()
                .map(|(t, n, e)| format!("  {n:>4} × {t:<24} {e}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    );
    assert!(
        total >= 9,
        "fixture 看起来不完整（只有 {total} 条）——重新录制，别放宽这条断言"
    );
}

/// 这个 fixture 与旧 fixture 最大的不同：**父子链是完整的**。
///
/// 旧 fixture 因为洗值时没同步 parentId，链大面积悬空，只能证明行级 schema。
/// 这个 fixture 保留了 id，所以它能证明**图**——而图正是 nini 的 session 层
/// 最容易出错的地方（`get_path()` 找叶子、fork 找父节点）。
#[test]
fn the_fixture_proves_the_graph_too() {
    let path = fixture("pi-0.85.1-session.jsonl");

    let mut ids: Vec<String> = Vec::new();
    let mut parents: Vec<String> = Vec::new();
    for line in std::fs::read_to_string(&path).unwrap().lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("session") {
            continue;
        }
        ids.push(
            v.get("id")
                .and_then(|i| i.as_str())
                .unwrap_or_default()
                .into(),
        );
        if let Some(p) = v.get("parentId").and_then(|p| p.as_str()) {
            parents.push(p.into());
        }
    }

    let known: std::collections::HashSet<&String> = ids.iter().collect();
    let resolving = parents.iter().filter(|p| known.contains(p)).count();

    assert!(
        !parents.is_empty(),
        "fixture 里没有 parentId 引用——它证明不了图"
    );
    assert_eq!(
        resolving,
        parents.len(),
        "只有 {resolving}/{} 个 parentId 能解析——录制时把 id 洗掉了，\
         这个 fixture 就退化成了旧 fixture（只能证明行级 schema）。\
         重新录制，不要放宽这条断言。",
        parents.len()
    );
}

/// 工具调用与工具结果**在 Pi 的会话里是留痕的**。
///
/// nini 自己的 P0.1 记录的是「nini **写**的时候丢 toolCall/toolResult」；
/// 这里证明的是**读**的时候不能丢：读都读不进去，就更谈不上写得像。
#[test]
fn the_fixture_contains_a_tool_call_and_its_result() {
    let path = fixture("pi-0.85.1-session.jsonl");
    let raw = std::fs::read_to_string(&path).unwrap();

    let mut tool_calls = 0usize;
    let mut tool_results = 0usize;
    for line in raw.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(msg) = v.get("message") else {
            continue;
        };
        // Pi 的形状：**工具调用**是 assistant 消息内容里的一个 `toolCall` 块；
        // **工具结果**是一条 role 为 "toolResult" 的消息（不是内容块）。
        if msg.get("role").and_then(|r| r.as_str()) == Some("toolResult") {
            tool_results += 1;
        }
        if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("toolCall") {
                    tool_calls += 1;
                }
            }
        }
    }

    assert!(
        tool_calls >= 1,
        "fixture 里没有 toolCall——它证明不了「工具调用在会话里留痕」"
    );
    assert!(
        tool_results >= 1,
        "fixture 里没有 toolResult——它证明不了「工具结果在会话里留痕」"
    );
}
