//! 截断语义与 **Pi 0.85.1** 的逐条比对（T4 的 oracle）。
//!
//! 期望值不是手算的：`scripts/truncate-oracle.mjs` **执行 Pi 自己的
//! `truncate.js`** 把结果冻进 `references/pi-0.85.1/truncate-oracle.json`。
//! 本测试把 nini 的实现对着那份冻结的期望值断言——**逐条**：
//! 内容、行数、字节数、截断原因（lines / bytes）、以及两个边界标记
//! （首行超限 / 末行半行）。
//!
//! 为什么必须是这样：截断决定「工具交还给模型多少东西」，也就决定了长任务里
//! 上下文的构成。nini 此前是各工具各写各的（bash 1 MiB、grep 按行数、
//! read 拒绝大文件），同一条命令在两侧占用的上下文差二十倍，而且没有任何
//! 东西会红。

use nini_tools::truncate::{
    format_size, truncate_head, truncate_line, truncate_tail, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
    GREP_MAX_LINE_LENGTH,
};
use serde_json::Value;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../references/pi-0.85.1/truncate-oracle.json"
);

fn fixture() -> Value {
    let raw = std::fs::read_to_string(FIXTURE)
        .unwrap_or_else(|e| panic!("读不到截断黄金值：{FIXTURE}（{e}）"));
    serde_json::from_str(&raw).expect("fixture 不是合法 JSON")
}

#[test]
fn the_constants_match_pi() {
    let fx = fixture();
    let c = &fx["constants"];
    assert_eq!(
        DEFAULT_MAX_LINES,
        c["DEFAULT_MAX_LINES"].as_u64().unwrap() as usize,
        "行数上限与 Pi 不一致"
    );
    assert_eq!(
        DEFAULT_MAX_BYTES,
        c["DEFAULT_MAX_BYTES"].as_u64().unwrap() as usize,
        "字节上限与 Pi 不一致"
    );
    assert_eq!(
        GREP_MAX_LINE_LENGTH,
        c["GREP_MAX_LINE_LENGTH"].as_u64().unwrap() as usize,
        "grep 单行上限与 Pi 不一致"
    );
}

#[test]
fn head_truncation_matches_pi_case_by_case() {
    let fx = fixture();
    for case in fx["cases"].as_array().expect("cases 必须是数组") {
        let id = case["id"].as_str().unwrap_or("?");
        let input = case["input"].as_str().unwrap_or_default();
        let max_lines = case["maxLines"].as_u64().unwrap_or(2000) as usize;
        let max_bytes = case["maxBytes"].as_u64().unwrap_or(51200) as usize;
        let want = &case["head"];

        let got = truncate_head(input, max_lines, max_bytes);
        assert_eq!(
            got.content,
            want["content"].as_str().unwrap_or_default(),
            "样本 {id}：head 内容与 Pi 不一致"
        );
        assert_eq!(
            got.truncated,
            want["truncated"].as_bool().unwrap_or(false),
            "样本 {id}：truncated 标记不一致"
        );
        let want_by: Option<String> = want["truncatedBy"].as_str().map(|s| s.to_string());
        assert_eq!(
            got.truncated_by.map(|s| s.to_string()),
            want_by,
            "样本 {id}：截断原因不一致"
        );
        assert_eq!(
            got.output_lines,
            want["outputLines"].as_u64().unwrap() as usize,
            "样本 {id}：输出行数不一致"
        );
        assert_eq!(
            got.output_bytes,
            want["outputBytes"].as_u64().unwrap() as usize,
            "样本 {id}：输出字节数不一致"
        );
        assert_eq!(
            got.total_lines,
            want["totalLines"].as_u64().unwrap() as usize,
            "样本 {id}：总行数不一致"
        );
        assert_eq!(
            got.total_bytes,
            want["totalBytes"].as_u64().unwrap() as usize,
            "样本 {id}：总字节数不一致"
        );
        assert_eq!(
            got.first_line_exceeds_limit,
            want["firstLineExceedsLimit"].as_bool().unwrap_or(false),
            "样本 {id}：首行超限标记不一致"
        );
    }
}

#[test]
fn tail_truncation_matches_pi_case_by_case() {
    let fx = fixture();
    for case in fx["cases"].as_array().expect("cases 必须是数组") {
        let id = case["id"].as_str().unwrap_or("?");
        let input = case["input"].as_str().unwrap_or_default();
        let max_lines = case["maxLines"].as_u64().unwrap_or(2000) as usize;
        let max_bytes = case["maxBytes"].as_u64().unwrap_or(51200) as usize;
        let want = &case["tail"];

        let got = truncate_tail(input, max_lines, max_bytes);
        assert_eq!(
            got.content,
            want["content"].as_str().unwrap_or_default(),
            "样本 {id}：tail 内容与 Pi 不一致"
        );
        assert_eq!(
            got.truncated,
            want["truncated"].as_bool().unwrap_or(false),
            "样本 {id}：tail truncated 标记不一致"
        );
        assert_eq!(
            got.output_lines,
            want["outputLines"].as_u64().unwrap() as usize,
            "样本 {id}：tail 输出行数不一致"
        );
        assert_eq!(
            got.output_bytes,
            want["outputBytes"].as_u64().unwrap() as usize,
            "样本 {id}：tail 输出字节数不一致"
        );
        assert_eq!(
            got.last_line_partial,
            want["lastLinePartial"].as_bool().unwrap_or(false),
            "样本 {id}：末行半行标记不一致"
        );
    }
}

#[test]
fn line_truncation_matches_pi_case_by_case() {
    let fx = fixture();
    for case in fx["lineCases"].as_array().expect("lineCases 必须是数组") {
        let id = case["id"].as_str().unwrap_or("?");
        let input = case["input"].as_str().unwrap_or_default();
        let max_chars = case["maxChars"].as_u64().unwrap_or(500) as usize;
        let (text, cut) = truncate_line(input, max_chars);
        assert_eq!(
            text,
            case["text"].as_str().unwrap_or_default(),
            "样本 {id}：单行截断结果与 Pi 不一致"
        );
        assert_eq!(
            cut,
            case["wasTruncated"].as_bool().unwrap_or(false),
            "样本 {id}：是否截断不一致"
        );
    }
}

#[test]
fn size_formatting_matches_pi() {
    let fx = fixture();
    for case in fx["formatSize"].as_array().expect("formatSize 必须是数组") {
        let bytes = case["bytes"].as_u64().unwrap() as usize;
        assert_eq!(
            format_size(bytes),
            case["text"].as_str().unwrap_or_default(),
            "format_size({bytes}) 与 Pi 不一致"
        );
    }
}
