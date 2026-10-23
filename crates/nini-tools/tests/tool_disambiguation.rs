//! Oracle: `find`, `ls` and `bash` must be distinguishable from each other.
//! invariant: 545fd548
//!
//! # 这条不变量的形状变了（2026-10-23，T2）
//!
//! 原���言：nini **没有** `ls`，所以「列一个目录」只能靠 bash，而 find 的
//! 描述曾经写着 "Find files by glob pattern …"——正是这句话让模型把
//! "列目录"选成 find（实测：`arguments: null`）。修法是给 find 的描述加一条
//! **否定式指引**（"not for listing … use bash"）。
//!
//! 现在 nini 有 `ls` 了（Pi 0.85.1 也有），而且三个工具的描述都按 Pi 逐字对齐。
//! Pi 的做法**不是**否定式指引，而是**职责互斥的精确描述**：
//!
//! * `ls`   —— "List directory contents. Returns entries sorted alphabetically…"
//! * `find` —— "Search for files by glob pattern. Returns matching file paths…"
//! * `bash` —— "Execute a bash command in the current working directory…"
//!
//! 不变的是**性质**（模型必须能把「列目录 / 按 glob 找文件 / 执行命令」
//! 三种请求分到三个工具上），变的是**载体**（否定式指引 → 互斥的职责描述）。
//! 护住性质，放弃载体——否则就是在保一条已经不成立的规则。

use nini_core::tool::Tool;
use nini_tools::{BashTool, FindTool, LsTool};

fn norm(s: &str) -> String {
    s.to_lowercase()
}

/// The system prompt contribution, flattened: snippet + every guideline.
fn contribution_text(tool: &dyn Tool) -> String {
    let c = tool
        .system_prompt_contribution()
        .expect("tool must contribute a system prompt section");
    format!("{}\n{}", c.snippet, c.guidelines.join("\n")).to_lowercase()
}

/// Everything the model reads about a tool: its description + its prompt section.
fn model_facing_text(tool: &dyn Tool) -> String {
    format!(
        "{}\n{}",
        norm(&tool.spec().description),
        contribution_text(tool)
    )
}

#[test]
fn ls_owns_listing_and_find_owns_the_glob() {
    let ls_text = model_facing_text(&LsTool::new());
    let find_text = model_facing_text(&FindTool::new());

    // `ls` 必须自报家门：列目录
    assert!(
        ls_text.contains("list directory contents"),
        "ls must say it lists directory contents: {ls_text}"
    );
    // `find` 必须自报家门：按 glob 找文件
    assert!(
        find_text.contains("glob pattern"),
        "find must say it matches files by glob: {find_text}"
    );
    // `find` 不得**抢占**列目录的活——那正是当初那个缺陷
    assert!(
        !find_text.contains("list directory contents"),
        "find must not claim the listing capability that `ls` owns: {find_text}"
    );
    assert!(
        !ls_text.contains("glob pattern"),
        "ls must not claim the glob capability that `find` owns: {ls_text}"
    );
}

#[test]
fn the_three_tools_present_different_guidance() {
    let find_text = model_facing_text(&FindTool::new());
    let ls_text = model_facing_text(&LsTool::new());
    let bash_text = model_facing_text(&BashTool::new());

    assert_ne!(find_text, ls_text, "find and ls present the same guidance");
    assert_ne!(
        find_text, bash_text,
        "find and bash present the same guidance"
    );
    assert_ne!(ls_text, bash_text, "ls and bash present the same guidance");

    // bash 仍然自报「执行命令」——它是 shell，不是文件浏览器
    assert!(
        bash_text.contains("bash command"),
        "bash must advertise command execution: {bash_text}"
    );
}

#[test]
fn a_listing_request_has_an_exact_owner() {
    // 端到端症状：「List files in crates/nini-tui/src/, just first 5」
    // 曾经选中 find（arguments: null）。现在必须有一个工具**精确地**承担
    // 「列目录」，模型才有信号。
    let ls_text = model_facing_text(&LsTool::new());
    let find_text = model_facing_text(&FindTool::new());

    assert!(
        ls_text.contains("list directory contents") && ls_text.contains("sorted alphabetically"),
        "ls must describe exactly what a listing request asks for: {ls_text}"
    );
    assert!(
        !find_text.contains("list"),
        "find must not contain the word that would attract a listing request: {find_text}"
    );
}
