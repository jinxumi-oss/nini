//! Oracle：**「链接不可点击」这件事被写下来了**（差距 **D3** 的收尾判据）。
//!
//! # 为什么是这条判据
//!
//! D3（Pi 的可点击链接 OSC 8）在 nini 上无法复刻。它的退出判据有两个分支：
//!
//! 1. **要么 ratatui 支持 hyperlink**（届时恢复注入并保持宽度计算正确）；
//! 2. **要么在 README 的 known limitations 里明说「链接不可点击」**。
//!
//! 第 1 条**已被上游证据排除**：ratatui 0.29 / 0.30.2 与 ratatui-core 0.1.2
//! 全部没有 hyperlink 的字样，crossterm 0.28 / 0.29 也没有；而且 ratatui 的
//! cell 缓冲区会**丢弃含控制字符的 grapheme**（ESC 是控制字符），差分重绘
//! 又只重画**变化的 cell**，而 OSC 8 是**有状态的转义对**（open/close 必须
//! 紧贴目标文字），在这套模型下无法保证配对。
//!
//! 于是只剩第 2 条：**把这件事写进 README**。
//!
//! # 这条 oracle 断言什么
//!
//! 「文档即契约」：README 里必须**同时**有
//!
//! - 英文版（`README.md`）在 **Known limitations** 段落里写明「URL 以纯文本
//!   呈现，**不是**可点击链接」；
//! - 中文版（`README.zh-CN.md`）在对应的段落里写明**同一件事**；
//! - 两边都给出**原因**（Pi 发 OSC 8 / nini 走 ratatui 的 cell 缓冲区）；
//! - 两边都指向差距 **D3**（写下来才能被找到）。
//!
//! 措辞一旦被改掉（例如把「不可点击」改成别的说法、或把这条从 known
//! limitations 里删掉），本测试就红——它是**故意**贴着措辞的：判据说的是
//! 「明说链接不可点击」，不是「随便提一句 URL」。

use std::path::{Path, PathBuf};

/// 仓库根目录（`crates/nini-tui` 的上两级）。
fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .expect("CARGO_MANIFEST_DIR 的上两级应当是仓库根目录")
        .to_path_buf()
}

fn read_readme(name: &str) -> String {
    let path = repo_root().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()))
}

/// 取出以 `heading` 开头的 `## ` 段落（到下一个 `## ` 为止）。
///
/// 找不到这个段落就 panic——「known limitations 段落被改名/删除」也是一种
/// 回归：判据要求限制写在**那个**段落里，不是散落在 README 任何地方。
fn section(text: &str, heading: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("## ") {
            if inside {
                break;
            }
            if line.starts_with(heading) {
                inside = true;
            }
            continue;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    assert!(inside, "README 里找不到以「{heading}」开头的 `## ` 段落");
    out
}

/// 把段落的换行与缩进压成单个空格——README 的正文是硬换行的，
/// 一句英文会被拆成 3 行。断言的是**句子**，不是**排版**。
fn flatten(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    out
}

#[test]
fn the_english_readme_says_urls_are_not_clickable_links() {
    let readme = read_readme("README.md");
    let known = flatten(&section(&readme, "## Known limitations"));

    assert!(
        known.contains("URLs in the transcript are shown as plain text, not as clickable links."),
        "README.md 的 Known limitations 里必须明说「URL 不是可点击链接」；\
         实际内容里没有这句话。若措辞变了，要么同步改本测试，要么把 README 改回去。\n\
         实际段落：\n{known}"
    );
}

#[test]
fn the_chinese_readme_says_the_same_thing() {
    let readme = read_readme("README.zh-CN.md");
    let known = flatten(&section(&readme, "## 已知限制"));

    assert!(
        known.contains("transcript 里的 URL 以纯文本呈现，不可点击。"),
        "README.zh-CN.md 的「已知限制」里必须明说「URL 不可点击」；\
         实际内容里没有这句话。若措辞变了，要么同步改本测试，要么把 README 改回去。\n\
         实际段落：\n{known}"
    );
}

#[test]
fn both_readmes_explain_why_the_osc_8_sequence_cannot_be_emitted() {
    let en = flatten(&section(&read_readme("README.md"), "## Known limitations"));
    let zh = flatten(&section(&read_readme("README.zh-CN.md"), "## 已知限制"));

    for (label, text) in [("README.md", &en), ("README.zh-CN.md", &zh)] {
        assert!(
            text.contains("OSC 8"),
            "{label} 的 known limitations 里必须点名 OSC 8（这是与 Pi 的差别所在）"
        );
        assert!(
            text.contains("ratatui"),
            "{label} 的 known limitations 里必须点名 ratatui（这是做不到的技术原因）"
        );
    }
}

#[test]
fn both_readmes_point_at_gap_d3() {
    let en = flatten(&section(&read_readme("README.md"), "## Known limitations"));
    let zh = flatten(&section(&read_readme("README.zh-CN.md"), "## 已知限制"));

    assert!(
        en.contains("D3"),
        "README.md 的这条限制必须指向差距 D3——写下来还不够，得找得到"
    );
    assert!(
        zh.contains("D3"),
        "README.zh-CN.md 的这条限制必须指向差距 D3——写下来还不够，得找得到"
    );
}
