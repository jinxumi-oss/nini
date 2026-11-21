//! 文本式工具调用（text-mode tool call）的解析。
//!
//! # 为什么需要它
//!
//! provider 侧的模型（Qwen / MiniMax / 讯飞星辰这一族）**会随机**把工具调用
//! 写进正文，而不是放进结构化的 `tool_calls` 字段：
//!
//! ```text
//! <function=bash>
//! <parameter=command>cargo test -p nini-core</parameter>
//! </function>
//! ```
//!
//! 同一个请求反复发，模型会在「结构化 tool_calls」与「正文里的标记」之间
//! 摇摆（实测约一半概率走后者）。nini 此前把这段正文当散文：工具不会执行、
//! 模型等一个永远不会来的 `tool_result`、于是反复重发同一条命令 —— 用户看到
//! 的就是「完全没动静」（静默中断）。
//!
//! 证据：会话 `~/.nini/agent/sessions/nini/20261119-181606_13b7f0639.jsonl`
//! 第 231-241 条 —— 助手消息里只有这段文本，没有结构化 `toolCall` 块。
//!
//! # 边界（安全面）
//!
//! * 本模块是**纯函数**：不碰状态、不执行任何东西、不知道工具注册表。
//! * 解析出来的调用**只在该工具已注册时才会被执行**（判定在调用方，
//!   `agent.rs`）：正文里提到一个不存在的工具名，不会被执行。
//! * 只认**闭合**的标记块；没闭合的当作文案，不执行。

use std::ops::Range;

/// 文本式工具调用合成出来的 id 前缀。
///
/// 调用方（`agent.rs`）用它区分「这条调用是从正文里还原出来的」，
/// TUI 侧也靠它知道该把正文里的标记一并摘掉。
pub const TEXT_TOOL_CALL_ID_PREFIX: &str = "call_text_";

/// 工具调用的起始标记。
const FUNCTION_OPEN: &str = "<function=";
/// 参数的起始标记。
const PARAM_OPEN: &str = "<parameter=";

/// 块的闭合标记。不同模型给的不一样，都认：
///
/// * `</function>` —— MiniMax / Qwen 一族的规范写法；
/// * `</arg_value>` —— 同一族模型偶尔的简写；
/// * `</tool_call>` —— 带零宽空格的变体（同一个标记，模型手抖）。
const BLOCK_CLOSERS: [&str; 4] = [
    "</function>",
    "</arg_value>",
    "</\u{200b}tool_call>",
    "</tool_call>",
];

/// 一条从正文里解析出来的工具调用。
#[derive(Debug, Clone, PartialEq)]
pub struct TextToolCall {
    /// 工具名（`<function=NAME>` 里的 NAME）。
    pub name: String,
    /// 参数（`<parameter=KEY>VALUE` 逐个拼成的对象）。
    pub args: serde_json::Value,
    /// 这一整块在原文里的字节区间（用来把标记从正文里摘掉）。
    pub span: Range<usize>,
}

/// 正文里是否出现了工具调用标记（无论能不能解析成一条合法调用）。
///
/// 调用方用它区分「模型没有要调工具」与「模型要调工具但我们没认出来」——
/// 后者必须说出来，否则又是一次静默卡死。
pub fn has_tool_call_marker(text: &str) -> bool {
    text.contains(FUNCTION_OPEN) || text.contains(PARAM_OPEN)
}

/// 解析正文里的文本式工具调用。
///
/// 只返回**闭合**、且有名字的调用；参数值原样当字符串收下（值本身是 JSON
/// 字面量时按 JSON 解析，这样对象/数组参数的工具也能用）。
pub fn parse_text_tool_calls(text: &str) -> Vec<TextToolCall> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = text[cursor..].find(FUNCTION_OPEN) {
        let open_at = cursor + rel;
        let after_open = open_at + FUNCTION_OPEN.len();
        // 工具名：到 '>' 或空白为止。
        let name_len = text[after_open..]
            .find(|c: char| c == '>' || c.is_whitespace())
            .unwrap_or(text.len() - after_open);
        let name = text[after_open..after_open + name_len].trim().to_string();
        let body_at = after_open + name_len;

        // 块尾：最近的闭合标记。找不到 → 这一块没闭合 → 不认。
        let body = &text[body_at..];
        let Some(block_len) = BLOCK_CLOSERS
            .iter()
            .filter_map(|closer| body.find(closer))
            .min()
        else {
            break;
        };

        let args = parse_parameters(&body[..block_len]);
        let span_end = body_at + block_len;
        if !name.is_empty() {
            out.push(TextToolCall {
                name,
                args: serde_json::Value::Object(args),
                span: open_at..span_end,
            });
        }
        cursor = span_end.max(open_at + FUNCTION_OPEN.len());
    }
    out
}

/// 把解析出来的标记从正文里摘掉。
///
/// 标记是**工具调用**，不是散文：留在正文里，模型下一轮会照着它再写一遍。
/// 摘掉之后正文里剩下的就是模型真正想说的话。
pub fn strip_text_tool_calls(text: &str, spans: &[Range<usize>]) -> String {
    if spans.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for span in spans {
        let start = span.start.min(text.len());
        let end = span.end.min(text.len()).max(start);
        if start > cursor {
            out.push_str(&text[cursor..start]);
        }
        cursor = cursor.max(end);
    }
    if cursor < text.len() {
        out.push_str(&text[cursor..]);
    }
    out
}

/// 解析 `<parameter=KEY>VALUE` 序列。
fn parse_parameters(block: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut args = serde_json::Map::new();
    let mut cursor = 0usize;
    while let Some(rel) = block[cursor..].find(PARAM_OPEN) {
        let key_at = cursor + rel + PARAM_OPEN.len();
        // 键名到 '>' 为止。
        let key_len = block[key_at..].find('>').unwrap_or(0);
        let key = block[key_at..key_at + key_len].trim().to_string();
        let value_at = key_at + key_len + usize::from(block[key_at + key_len..].starts_with('>'));
        // 值到下一个参数、参数闭合标记或块尾为止。
        let mut value_end = block.len();
        if let Some(x) = block[value_at..].find(PARAM_OPEN) {
            value_end = value_end.min(value_at + x);
        }
        if let Some(x) = block[value_at..].find("</parameter>") {
            value_end = value_end.min(value_at + x);
        }
        if !key.is_empty() {
            let raw = block[value_at..value_end].trim();
            args.insert(key, parse_value(raw));
        }
        cursor = value_end.max(value_at);
    }
    args
}

/// 参数值：JSON 字面量按 JSON 解析，其余按字符串收下。
fn parse_value(raw: &str) -> serde_json::Value {
    if raw.starts_with('{') || raw.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
            return v;
        }
    }
    serde_json::Value::String(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_minimax_style_block() {
        let text = "修复已应用。\n<function=bash>\n<parameter=command>cargo test -p nini-core</parameter>\n</function>";
        let calls = parse_text_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert_eq!(
            calls[0].args,
            serde_json::json!({"command": "cargo test -p nini-core"})
        );
    }

    #[test]
    fn parses_several_parameters_of_one_call() {
        let text = "<function=edit>\n<parameter=path>a.rs</parameter>\n<parameter=edits>[{\"old\":\"a\",\"new\":\"b\"}]</parameter>\n</function>";
        let calls = parse_text_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "edit");
        assert_eq!(calls[0].args["path"], serde_json::json!("a.rs"));
        // 值是 JSON 字面量 → 按 JSON 解析（对象/数组参数的工具才能用）。
        assert_eq!(
            calls[0].args["edits"],
            serde_json::json!([{"old":"a","new":"b"}])
        );
    }

    #[test]
    fn accepts_the_short_and_zero_width_closers() {
        for closer in ["</function>", "</arg_value>", "</\u{200b}tool_call>"] {
            let text = format!("<function=bash><parameter=command>ls{closer}");
            let calls = parse_text_tool_calls(&text);
            assert_eq!(calls.len(), 1, "closer {closer:?} 应被认出来");
            assert_eq!(calls[0].args["command"], serde_json::json!("ls"));
        }
    }

    #[test]
    fn an_unclosed_block_is_not_a_call() {
        let text = "<function=bash>\n<parameter=command>ls";
        assert!(parse_text_tool_calls(text).is_empty());
        // 但「有标记」这件事必须能被看见，否则又是一次静默卡死。
        assert!(has_tool_call_marker(text));
    }

    #[test]
    fn prose_without_markers_yields_nothing() {
        assert!(parse_text_tool_calls("今天把 TUI 的滚动条修好了。").is_empty());
        assert!(!has_tool_call_marker("今天把 TUI 的滚动条修好了。"));
    }

    #[test]
    fn stripping_removes_the_block_and_keeps_the_prose() {
        let text = "修复已应用。\n<function=bash>\n<parameter=command>ls</parameter>\n</function>";
        let calls = parse_text_tool_calls(text);
        let stripped = strip_text_tool_calls(
            text,
            &calls.iter().map(|c| c.span.clone()).collect::<Vec<_>>(),
        );
        assert!(stripped.contains("修复已应用。"));
        assert!(!stripped.contains("<function="));
        assert!(!stripped.contains("<parameter="));
    }

    #[test]
    fn a_value_containing_angle_brackets_survives() {
        let text =
            "<function=bash>\n<parameter=command>grep '<pattern>' file.rs</parameter>\n</function>";
        let calls = parse_text_tool_calls(text);
        assert_eq!(
            calls[0].args["command"],
            serde_json::json!("grep '<pattern>' file.rs")
        );
    }
}
