//! OSC 8 hyperlink auto-detection and wrapping.
//!
//! Mirrors Pi's `hyperlink()` helper (pi-tui/dist/terminal-image.js) +
//! `auto_link` behavior used by `renderAssistantMessage` in pi-coding-agent.
//!
//! OSC 8 format: `ESC ]8;<params>;<URL> ESC \TEXT ESC ]8;; ESC \`
//! Supported by iTerm2 (since 3.1+), Kitty (0.20+), WezTerm, Ghostty, Konsole.
//! Unsupported terminals render the TEXT portion only \u2014 still readable.
//!
//! Capability detection (cached on first call):
//!   - iTerm2:  `TERM_PROGRAM=iTerm.app`
//!   - Kitty:   `KITTY_WINDOW_ID` set
//!   - WezTerm: `TERM_PROGRAM=WezTerm`
//!   - Ghostty: `TERM_PROGRAM=ghostty`
//!   - VS Code: `TERM_PROGRAM=vscode` (the integrated terminal forwards links)
//!   - Konsole: `KONSOLE_VERSION` set
//!   - foot:    `TERM=foot` / `foot-extra`
//!   - Windows Terminal: `WT_SESSION` set
//!
//! All other terminals get a fallback that emits raw URLs without OSC 8.

use std::env;
use std::sync::OnceLock;

/// Detect OSC 8 capability exactly once per process.
fn supports_osc8() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(detect_osc8_support)
}

fn detect_osc8_support() -> bool {
    if let Ok(tp) = env::var("TERM_PROGRAM") {
        if matches!(
            tp.as_str(),
            "iTerm.app" | "WezTerm" | "ghostty" | "vscode" | "Apple_Terminal"
        ) {
            return true;
        }
    }
    if env::var("KITTY_WINDOW_ID").is_ok() {
        return true;
    }
    if env::var("KONSOLE_VERSION").is_ok() {
        return true;
    }
    if env::var("WT_SESSION").is_ok() {
        return true;
    }
    if let Ok(term) = env::var("TERM") {
        if term.starts_with("foot") || term.starts_with("xterm-kitty") || term == "alacritty" {
            return true;
        }
    }
    false
}

/// Wrap a URL in OSC 8 hyperlinks when supported, otherwise pass through.
///
/// `text` is the visible text; `url` is what the terminal opens on click.
pub fn wrap(text: &str, url: &str) -> String {
    if supports_osc8() {
        format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
    } else {
        text.to_string()
    }
}

/// Find bare URLs in `s` and wrap them in OSC 8 hyperlinks.
///
/// Supports http(s)://, ftp://, file://. Does not touch URLs already
/// inside markdown link syntax `[label](url)` \u2014 that's handled by the
/// Markdown renderer separately.
pub fn auto_link(text: &str) -> String {
    // v0.10.8 (TUI bug)：**不再注入 OSC 8**。
    //
    // 之前这里把裸 URL 包成 `ESC ]8;;<url> ESC \` —— 那是 Pi 的做法（Pi 自己
    // 往终端写字节，能正确地发出这个序列）。
    //
    // 但 nini 的 transcript 是通过 **ratatui 的 cell 缓冲区**画的：
    // ratatui 0.29 **没有** hyperlink 支持（没有 `Modifier::HYPERLINK`），
    // 于是那段转义序列会变成**字面的 cell 文本**——终端收到 `ESC ]8;;` 后
    // 把它当 OSC 开始，把后面的 URL 当**可见文字**画出来：那一行比屏幕宽，
    // **框破了**；而且宽度计算把零宽的转义算成了可见字符，行被**提前截断**
    // （实测吃掉 139 格）。
    //
    // 所以这里只能**明说不做**：URL 以纯文本呈现（Pi 在不支持 OSC 8 的终端上
    // 同样是纯文本）。这是能力差距，记在台账里（差距 D3）。
    let _ = detect_osc8_support();
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v0.10.8 (TUI bug)：`auto_link` **不再注入 OSC 8**。
    ///
    /// 注入的那段转义会变成字面的 cell 文本 —— 终端把 URL 当可见文字画出来，
    /// 那一行比屏幕宽，**框破了**；而且宽度计算把零宽转义算成可见字符，
    /// 行被提前截断（实测吃掉 139 格）。
    #[test]
    fn auto_link_never_injects_escape_sequences() {
        for text in [
            "see https://example.com/page for details",
            "https://example.com",
            "no url here",
        ] {
            let out = auto_link(text);
            assert!(
                !out.contains('\u{1b}'),
                "auto_link 绝不能注入转义序列：{out:?}"
            );
            assert_eq!(out, text, "auto_link 现在是恒等变换");
        }
    }
}
