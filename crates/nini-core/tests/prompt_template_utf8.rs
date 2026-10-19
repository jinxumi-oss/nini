//! Oracle: `prompt_template` must not panic on multi-byte UTF-8.
//! invariant: 3a31fe9e

use nini_core::prompt_template::load_prompt_templates;
use tempfile::TempDir;

fn load_first_line_description(content: &str) -> String {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("probe.md"), content).unwrap();
    let result = load_prompt_templates([tmp.path()]);
    assert_eq!(
        result.templates.len(),
        1,
        "the template must load — a panic here is the bug, not a skip"
    );
    result.templates[0].description.clone()
}

#[test]
fn cjk_first_line_longer_than_sixty_bytes_does_not_panic() {
    // 20 CJK chars = 60 bytes, so byte index 60 lands mid-char. The original
    // code sliced `&first[..60]` and panicked at load time — before the user
    // could do anything at all.
    let line = "这是一段超过二十个汉字的很长的第一行描述文本用来触发字节切片崩溃";
    assert_eq!(line.len(), line.chars().count() * 3);
    assert!(line.len() > 60);

    let description = load_first_line_description(line);

    // The truncation is by **character**, not by byte: 60 chars + "...".
    let expected: String = line.chars().take(60).collect();
    assert_eq!(description, format!("{expected}..."));
    assert!(description.is_char_boundary(description.len()));
}

#[test]
fn emoji_first_line_does_not_panic() {
    // 4-byte chars: byte 60 lands in the middle of the 16th emoji, which is
    // a harder violation than the 3-byte CJK case above.
    let line = "🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀";
    assert!(line.len() > 60);

    let description = load_first_line_description(line);
    let expected: String = line.chars().take(60).collect();
    assert_eq!(description, format!("{expected}..."));
}

#[test]
fn mixed_width_first_line_truncates_on_a_char_boundary() {
    let line =
        "前言 ascii 混合 tail 尾巴结束：宽度不一的三字节与单字节字符交错排列直到超过六十字节";
    assert!(line.len() > 60);
    assert!(line.chars().count() < 60, "long in bytes, short in chars");

    let description = load_first_line_description(line);
    let expected: String = line.chars().take(60).collect();
    assert_eq!(description, format!("{expected}..."));
    // Round-trips through UTF-8 without loss or replacement characters.
    assert!(!description.contains('\u{FFFD}'));
}
