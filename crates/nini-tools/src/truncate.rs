//! 工具输出的**统一**截断（v0.10.8 / T4）。
//!
//! 此前 nini 的截断是各工具各写各的：bash 1 MiB/2000 行、grep 按行数、
//! read 直接拒绝大文件、ls 自己一套字节截断。于是**同一条命令在 nini 与 Pi
//! 两侧占用的上下文差着二十倍**——而上下文是长任务里最稀缺的资源。
//!
//! 这里实现 Pi 0.85.1 `dist/core/tools/truncate.js` 的语义：
//!
//! * 两个独立上限，**先到者胜**：行数（默认 2000）与字节（默认 50 KiB）；
//! * `truncate_head`：保留**头部**的完整行（永不返回半行；首行本身就超限时
//!   返回空内容并置 `first_line_exceeds_limit`）；
//! * `truncate_tail`：保留**尾部**（首行放不下时允许**半行**，且按 UTF-8
//!   边界回退）；
//! * `truncate_line`：单行按**字符**截断（默认 500），尾部加 `... [truncated]`。
//!
//! 期望值不是手算的：`scripts/truncate-oracle.mjs` **执行 Pi 自己的代码**
//! 把结果冻进 `references/pi-0.85.1/truncate-oracle.json`，Rust 侧对着它断言。

/// Pi 的默认行数上限。
pub const DEFAULT_MAX_LINES: usize = 2000;
/// Pi 的默认字节上限（50 KiB）。
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;
/// Pi 的 grep 单行字符上限。
pub const GREP_MAX_LINE_LENGTH: usize = 500;

/// 截断结果。字段与 Pi 的返回对象一一对应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truncation {
    pub content: String,
    pub truncated: bool,
    /// `"lines"` / `"bytes"` / `None`（没截断）。
    pub truncated_by: Option<&'static str>,
    pub output_lines: usize,
    pub output_bytes: usize,
    pub total_lines: usize,
    pub total_bytes: usize,
    /// tail 截断时，最后一行可能只保留了一半。
    pub last_line_partial: bool,
    /// head 截断时，首行本身就超限（内容为空）。
    pub first_line_exceeds_limit: bool,
}

/// 按 `\n` 切分；末尾的换行不产生一个空行（与 Pi 的 splitLinesForCounting 一致）。
fn split_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// 保留头部的完整行。
pub fn truncate_head(content: &str, max_lines: usize, max_bytes: usize) -> Truncation {
    let total_bytes = content.len();
    let lines = split_lines(content);
    let total_lines = lines.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return Truncation {
            content: content.to_string(),
            truncated: false,
            truncated_by: None,
            output_lines: total_lines,
            output_bytes: total_bytes,
            total_lines,
            total_bytes,
            last_line_partial: false,
            first_line_exceeds_limit: false,
        };
    }

    // 首行本身就超限 → 空内容（Pi 的行为：宁可空，也不要半行）
    let first_line_bytes = lines.first().map(|l| l.len()).unwrap_or(0);
    if first_line_bytes > max_bytes {
        return Truncation {
            content: String::new(),
            truncated: true,
            truncated_by: Some("bytes"),
            output_lines: 0,
            output_bytes: 0,
            total_lines,
            total_bytes,
            last_line_partial: false,
            first_line_exceeds_limit: true,
        };
    }

    let mut out: Vec<String> = Vec::new();
    let mut out_bytes = 0usize;
    let mut truncated_by = "lines";
    for (i, line) in lines.iter().enumerate() {
        if i >= max_lines {
            break;
        }
        // +1 是换行本身（第一行没有）
        let line_bytes = line.len() + usize::from(i > 0);
        if out_bytes + line_bytes > max_bytes {
            truncated_by = "bytes";
            break;
        }
        out.push((*line).to_string());
        out_bytes += line_bytes;
    }
    if out.len() >= max_lines && out_bytes <= max_bytes {
        truncated_by = "lines";
    }

    let joined = out.join("\n");
    Truncation {
        output_bytes: joined.len(),
        content: joined,
        truncated: true,
        truncated_by: Some(truncated_by),
        output_lines: out.len(),
        total_lines,
        total_bytes,
        last_line_partial: false,
        first_line_exceeds_limit: false,
    }
}

/// 保留尾部（可以带半行）。
pub fn truncate_tail(content: &str, max_lines: usize, max_bytes: usize) -> Truncation {
    let total_bytes = content.len();
    let lines = split_lines(content);
    let total_lines = lines.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return Truncation {
            content: content.to_string(),
            truncated: false,
            truncated_by: None,
            output_lines: total_lines,
            output_bytes: total_bytes,
            total_lines,
            total_bytes,
            last_line_partial: false,
            first_line_exceeds_limit: false,
        };
    }

    let mut out: Vec<String> = Vec::new();
    let mut out_bytes = 0usize;
    let mut truncated_by = "lines";
    let mut last_line_partial = false;
    for line in lines.iter().rev() {
        if out.len() >= max_lines {
            break;
        }
        let line_bytes = line.len() + usize::from(!out.is_empty());
        if out_bytes + line_bytes > max_bytes {
            truncated_by = "bytes";
            if out.is_empty() {
                // 一行都放不下：保留该行的**尾部**，按 UTF-8 边界回退
                let partial = truncate_string_to_bytes_from_end(line, max_bytes);
                out_bytes = partial.len();
                out.insert(0, partial);
                last_line_partial = true;
            }
            break;
        }
        out.insert(0, (*line).to_string());
        out_bytes += line_bytes;
    }
    if out.len() >= max_lines && out_bytes <= max_bytes {
        truncated_by = "lines";
    }

    let joined = out.join("\n");
    Truncation {
        output_bytes: joined.len(),
        content: joined,
        truncated: true,
        truncated_by: Some(truncated_by),
        output_lines: out.len(),
        total_lines,
        total_bytes,
        last_line_partial,
        first_line_exceeds_limit: false,
    }
}

/// 从尾部保留 `max_bytes` 字节，按 UTF-8 字符边界回退（Pi 的
/// truncateStringToBytesFromEnd）。
fn truncate_string_to_bytes_from_end(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut start = bytes.len() - max_bytes;
    // 跳过 UTF-8 的续接字节（0b10xxxxxx）
    while start < bytes.len() && (bytes[start] & 0xC0) == 0x80 {
        start += 1;
    }
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// 截断单行到 `max_chars` 个字符（按 **UTF-16 码元**计，与 JS 的
/// `String.prototype.slice` 一致），并加上 `... [truncated]`。
pub fn truncate_line(line: &str, max_chars: usize) -> (String, bool) {
    let units = line.encode_utf16().count();
    if units <= max_chars {
        return (line.to_string(), false);
    }
    let cut: String = line
        .encode_utf16()
        .take(max_chars)
        .map(|u| char::from_u32(u as u32).unwrap_or('\u{FFFD}'))
        .collect();
    (format!("{cut}... [truncated]"), true)
}

/// 人类可读的字节大小（Pi 的 formatSize）。
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_content_is_untouched() {
        let t = truncate_head("a\nb\nc\n", DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert!(!t.truncated);
        assert_eq!(t.content, "a\nb\nc\n");
        assert_eq!(t.output_lines, 3);
    }

    #[test]
    fn head_keeps_whole_lines_only() {
        let content = (0..10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let t = truncate_head(&content, 3, DEFAULT_MAX_BYTES);
        assert_eq!(t.truncated_by, Some("lines"));
        assert_eq!(t.content, "line 0\nline 1\nline 2");
    }

    #[test]
    fn tail_may_keep_a_partial_line() {
        // 一行就超过字节上限 → 只能保留它的尾部（半行）
        let content = "z".repeat(100);
        let t = truncate_tail(&content, DEFAULT_MAX_LINES, 20);
        assert!(t.truncated);
        assert!(t.last_line_partial);
        assert!(t.content.len() <= 20);
        // UTF-8 边界：不能切在多字节字符中间
        let cjk = "中".repeat(100);
        let t2 = truncate_tail(&cjk, DEFAULT_MAX_LINES, 21);
        assert!(t2.last_line_partial);
        assert!(std::str::from_utf8(t2.content.as_bytes()).is_ok());
    }

    #[test]
    fn first_line_over_the_limit_yields_empty_content() {
        let content = "z".repeat(100) + "\nshort";
        let t = truncate_head(&content, DEFAULT_MAX_LINES, 10);
        assert!(t.first_line_exceeds_limit);
        assert_eq!(t.content, "");
    }

    #[test]
    fn line_truncation_marks_itself() {
        let (text, cut) = truncate_line(&"a".repeat(600), GREP_MAX_LINE_LENGTH);
        assert!(cut);
        assert!(text.ends_with("... [truncated]"));
        let (text2, cut2) = truncate_line("short", GREP_MAX_LINE_LENGTH);
        assert!(!cut2);
        assert_eq!(text2, "short");
    }
}
