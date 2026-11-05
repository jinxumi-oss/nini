//! v0.11 (R13): 主题槽的接线 —— thinking 档位接到输入框外框，旧的自创槽删掉。
//!
//! # 三件事，各有各的失败方式
//!
//! 1. **thinking 档位的外框颜色**（`theme.rs` 的七个 `thinking*` 槽）。
//!    Pi 把 thinking 档位接到编辑器的边框颜色上
//!    （`references/pi-0.85.1/src/ui/interactive-mode.ts:4166-4172`，
//!    `updateEditorBorderColor`）：bash mode 用 `bashMode`，否则用当前档位。
//!    nini 早就有输入框外框，只是外框永远是 `borderMuted` —— 七个槽在
//!    `COLOR_NAMES` 里躺着，没有任何生产代码读它们。这里断言：给定档位，
//!    外框那一行用的就是对应的槽。
//!
//! 2. **六个旧的自创槽**（`background` / `code` / `foreground` / `header` /
//!    `info` / `link`）。Pi 没有这些键，nini 的生产代码也一个都没读过。
//!    它们被删掉之后，用户 `theme.json` 里的同名键会被解析器的过滤器
//!    **静默**丢掉 —— 用户改了颜色，屏幕上什么也没发生。所以这里断言：
//!    它们不在 `COLOR_NAMES` 里，同时**有提示**告诉用户被忽略了。
//!
//! 3. **不能误删**：`customMessage*` 三个槽和 `border` / `borderAccent`
//!    本轮不动（`customMessage*` 堵在扩展运行时上，`border*` 等新的盒式
//!    UI）。删多了就是回归，所以也断言它们还在。
//!
//! 断言读的是**画出来的帧**（外框那一行的 fg 颜色、提示出现在屏幕上），
//! 不是槽名表本身 —— 槽名对了但没接线，屏���上还是看不出来。

use nini_tui::render::render_frame_with_theme;
use nini_tui::state::AppState;
use nini_tui::theme::{Theme, COLOR_NAMES, LEGACY_SLOT_NAMES};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use ratatui::Terminal;

/// nini 早期版本自创、Pi 没有、现在被删掉的六个槽。
const LEGACY: [&str; 6] = ["background", "code", "foreground", "header", "info", "link"];

/// 本轮**不动**的槽：扩展运行时（`customMessage*`）和新的盒式 UI
/// （`border` / `borderAccent`）还没到位，删了就是提前拆房子。
const KEPT: [&str; 5] = [
    "customMessageBg",
    "customMessageText",
    "customMessageLabel",
    "border",
    "borderAccent",
];

/// 画一帧，返回缓冲区。
fn frame(state: &AppState, theme: &Theme) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|f| render_frame_with_theme(f, state, theme))
        .unwrap();
    terminal.backend().buffer().clone()
}

/// 帧里的全部文字（按行拼接）。
///
/// 宽字符（CJK）在 ratatui 的缓冲区里占两格，第二格是"被占掉"的空格。
/// 直接把每个单元格的符号拼起来，CJK 的字之间就会多出空格（"里 的"），
/// 所以这里按字符的显示宽度跳过那一格 —— 拼出来的才是用户真正看到的字。
fn text(buf: &Buffer) -> String {
    let area = *buf.area();
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        let mut x = area.left();
        while x < area.right() {
            let cell = buf.cell((x, y)).expect("cell in range");
            let symbol = cell.symbol();
            let width = nini_tui::width::display_width(symbol).max(1);
            out.push_str(symbol);
            x += width as u16;
        }
        out.push('\n');
    }
    out
}

/// 输入框外框那一行的 fg 颜色。
///
/// 外框的顶边带着标题 `─ input ─`，所以找带 "input" 的那一行；那一行是
/// 一整段 Span 画的（`Paragraph::new(Span::styled(top_line, border_style))`），
/// 所以行内任意一个单元格的 fg 就是外框的颜色。
fn prompt_border_fg(buf: &Buffer) -> Color {
    let area = *buf.area();
    for y in area.top()..area.bottom() {
        let row: String = (area.left()..area.right())
            .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect();
        if !row.contains("input") {
            continue;
        }
        for x in area.left()..area.right() {
            let cell = buf.cell((x, y)).expect("cell in range");
            if cell.symbol() != " " {
                return cell.fg;
            }
        }
    }
    panic!("the frame has no prompt border row");
}

/// 一个把七个 thinking 槽、borderMuted、bashMode 都设成不同颜色的主题。
/// 断言的是"用了哪一个槽"，所以每个槽的颜色必须能互相区分。
fn probe_theme() -> Theme {
    Theme::parse(
        r##"{"name":"probe","colors":{
            "thinkingOff":"#010101",
            "thinkingMinimal":"#020202",
            "thinkingLow":"#030303",
            "thinkingMedium":"#040404",
            "thinkingHigh":"#050505",
            "thinkingXhigh":"#060606",
            "thinkingMax":"#070707",
            "borderMuted":"#080808",
            "bashMode":"#090909"
        }}"##,
    )
    .expect("the probe theme parses")
}

// ---- ① thinking 档位 → 输入框外框 ----

/// 七个档位，挨个来一遍。
///
/// `off` 和 `max` 最容易漏：档位表是 `off|minimal|low|medium|high|xhigh|max`
/// （`nini-core/src/provider.rs` 的 `ThinkingLevel::ALL`），少一个就是少一种
/// 颜色，所以这里七个都写出来，不写成循环里的下标。
#[test]
fn the_input_border_follows_the_thinking_level() {
    let theme = probe_theme();
    let cases: [(&str, Color); 7] = [
        ("off", Color::Rgb(1, 1, 1)),
        ("minimal", Color::Rgb(2, 2, 2)),
        ("low", Color::Rgb(3, 3, 3)),
        ("medium", Color::Rgb(4, 4, 4)),
        ("high", Color::Rgb(5, 5, 5)),
        ("xhigh", Color::Rgb(6, 6, 6)),
        ("max", Color::Rgb(7, 7, 7)),
    ];
    for (level, expected) in cases {
        let mut state = AppState::new("test-model");
        state.model_state.thinking_level = Some(level.to_string());
        let buf = frame(&state, &theme);
        assert_eq!(
            prompt_border_fg(&buf),
            expected,
            "thinking level {level:?} did not reach the input border"
        );
    }
}

/// 档位 → 槽名的映射本身也要钉住：渲染那一层是从这里取的。
#[test]
fn every_thinking_level_maps_to_its_own_slot() {
    let expected = [
        ("off", "thinkingOff"),
        ("minimal", "thinkingMinimal"),
        ("low", "thinkingLow"),
        ("medium", "thinkingMedium"),
        ("high", "thinkingHigh"),
        ("xhigh", "thinkingXhigh"),
        ("max", "thinkingMax"),
    ];
    for (level, slot) in expected {
        assert_eq!(
            nini_tui::theme::thinking_border_slot(level),
            Some(slot),
            "thinking level {level:?} maps to the wrong slot"
        );
    }
    // 认不出来的档位不能把外框弄成没有颜色。
    assert_eq!(nini_tui::theme::thinking_border_slot("turbo"), None);
}

/// 没有档位 → 回到 `borderMuted`（v0.11 之前外框一直是这个颜色）。
#[test]
fn no_thinking_level_keeps_the_muted_border() {
    let theme = probe_theme();
    let state = AppState::new("test-model");
    assert!(state.model_state.thinking_level.is_none());
    let buf = frame(&state, &theme);
    assert_eq!(
        prompt_border_fg(&buf),
        Color::Rgb(8, 8, 8),
        "the input border lost its default colour"
    );
}

/// 认不出来的档位 → 同样是 `borderMuted`，不是没有颜色。
#[test]
fn an_unknown_thinking_level_falls_back_to_the_muted_border() {
    let theme = probe_theme();
    let mut state = AppState::new("test-model");
    state.model_state.thinking_level = Some("turbo".to_string());
    let buf = frame(&state, &theme);
    assert_eq!(prompt_border_fg(&buf), Color::Rgb(8, 8, 8));
}

/// bash mode 优先于 thinking 档位 —— 和 Pi 的 `updateEditorBorderColor`
/// 一样的顺序。
#[test]
fn bash_mode_wins_over_the_thinking_level() {
    let theme = probe_theme();
    let mut state = AppState::new("test-model");
    state.model_state.thinking_level = Some("max".to_string());
    state.input.text = "!ls -la".to_string();
    let buf = frame(&state, &theme);
    assert_eq!(
        prompt_border_fg(&buf),
        Color::Rgb(9, 9, 9),
        "bash mode did not take the border colour"
    );
}

// ---- ② 旧的自创槽：删掉，但不能静默 ----

/// 六个旧槽不在 `COLOR_NAMES` 里了。
#[test]
fn the_legacy_slots_are_gone_from_color_names() {
    for slot in LEGACY {
        assert!(
            !COLOR_NAMES.contains(&slot),
            "{slot} is still a colour slot"
        );
    }
    assert_eq!(
        LEGACY_SLOT_NAMES.len(),
        LEGACY.len(),
        "the legacy list and the removed slots disagree"
    );
    for slot in LEGACY {
        assert!(
            LEGACY_SLOT_NAMES.contains(&slot),
            "{slot} was removed but is not in LEGACY_SLOT_NAMES"
        );
    }
}

/// 内置调色板里也不能有它们的默认色（否则是悬空的 `m.insert(...)`）。
#[test]
fn the_legacy_slots_have_no_palette_entry() {
    for theme in [Theme::dark(), Theme::light()] {
        for slot in LEGACY {
            assert!(
                !theme.colors.contains_key(slot),
                "{slot} still has a default colour"
            );
        }
    }
}

/// 删槽的代价：用户 `theme.json` 里的这些键会被过滤器丢掉。所以解析时必须
/// 把它们记下来。
#[test]
fn a_user_theme_records_the_legacy_keys_it_had() {
    let theme = Theme::parse(
        r##"{"name":"user","colors":{
            "background":"#101010",
            "code":"#202020",
            "accent":"#303030"
        }}"##,
    )
    .expect("the user theme parses");
    assert_eq!(
        theme.ignored_slots,
        vec!["background".to_string(), "code".to_string()],
        "the legacy keys were dropped without being recorded"
    );
    // 值确实没有进调色板 —— 它们不再是槽。
    assert!(!theme.colors.contains_key("background"));
    assert!(!theme.colors.contains_key("code"));
    // 正常槽不受影响。
    assert_eq!(theme.color("accent"), Color::Rgb(0x30, 0x30, 0x30));
}

/// 提示要说清"这些键不再被使用"，并且把键名列出来。
#[test]
fn the_notice_names_every_ignored_key() {
    let mut colors = String::new();
    for (i, slot) in LEGACY.iter().enumerate() {
        if i > 0 {
            colors.push(',');
        }
        colors.push_str(&format!("\"{slot}\":\"#010101\""));
    }
    let json = format!("{{\"name\":\"user\",\"colors\":{{{}}}}}", colors);
    let theme = Theme::parse(&json).expect("the user theme parses");
    assert_eq!(theme.ignored_slots.len(), LEGACY.len());
    let notice = theme.ignored_slot_notice().expect("there is a notice");
    for slot in LEGACY {
        assert!(
            notice.contains(slot),
            "the notice does not mention {slot}: {notice}"
        );
    }
    // 措辞：这些键被忽略了，不是在报错。
    assert!(
        notice.contains("已忽略") && notice.contains("不再"),
        "the notice does not say the keys are ignored: {notice}"
    );
    assert!(
        !notice.contains("错误") && !notice.contains("invalid"),
        "the notice calls the keys an error: {notice}"
    );
}

/// 提示要真的出现在屏幕上，而不是只存在于一个没人调用的函数里。
#[test]
fn the_ignored_keys_are_reported_on_screen() {
    let theme =
        Theme::parse(r##"{"name":"user","colors":{"background":"#101010","code":"#202020"}}"##)
            .expect("the user theme parses");
    let state = AppState::new("test-model");
    let buf = frame(&state, &theme);
    let screen = text(&buf);
    assert!(
        screen.contains("background"),
        "the screen says nothing about the ignored key:\n{screen}"
    );
    assert!(
        screen.contains("code"),
        "the screen says nothing about the ignored key:\n{screen}"
    );
    assert!(
        screen.contains("已忽略"),
        "the screen does not say the keys were ignored:\n{screen}"
    );
    // 只出现一次：不是每帧追加一行。
    assert_eq!(
        screen.matches("background").count(),
        1,
        "the notice is repeated on the screen:\n{screen}"
    );
}

/// 没有旧键的用户主题不该被提示。
#[test]
fn a_theme_without_legacy_keys_is_not_announced() {
    let theme = Theme::parse(r##"{"name":"user","colors":{"accent":"#303030"}}"##)
        .expect("the user theme parses");
    assert!(theme.ignored_slot_notice().is_none());
    let state = AppState::new("test-model");
    let screen = text(&frame(&state, &theme));
    assert!(
        !screen.contains("已忽略"),
        "a clean theme produced a notice:\n{screen}"
    );
}

/// 内置主题没有旧键，所以永远不提示。
#[test]
fn the_built_in_themes_never_produce_a_notice() {
    for theme in [Theme::dark(), Theme::light(), Theme::default()] {
        assert!(theme.ignored_slot_notice().is_none());
    }
}

// ---- ③ 不能误删的槽 ----

/// `customMessage*` 和 `border` / `borderAccent` 本轮不动。
#[test]
fn the_slots_that_are_not_this_rounds_business_are_still_there() {
    for slot in KEPT {
        assert!(
            COLOR_NAMES.contains(&slot),
            "{slot} was removed, but this round is not about it"
        );
    }
    // 内置调色板里也得有它们的值。
    for slot in KEPT {
        assert!(
            Theme::dark().colors.contains_key(slot),
            "{slot} lost its default colour"
        );
    }
}

/// 槽数：61 → 55（删掉六个旧的自创槽）。
#[test]
fn the_slot_count_is_fifty_five() {
    assert_eq!(COLOR_NAMES.len(), 55);
}
