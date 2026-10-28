//! crates/nini-core/tests/ledger_anchors.rs —— 差距台账的**第二套实现**。
//!
//! 差距 C2 的病根是「计划文档与代码失真：文档里的状态字段不能指向代码符号」。
//! `scripts/parity-gaps.json` 里每条 gap 的 `nini_state` 都写着一些代码锚点
//! （文件路径 + 符号名），而**没有任何一条自动检查**能发现：
//!
//!   * 锚点指向的文件已经不存在了；
//!   * 锚点里的符号已经被改名/删掉了；
//!   * `nini_state` 退化成了纯散文（一个锚点都没有，机器无法核对）。
//!
//! Python 侧有 `scripts/ledger-anchors.py`；**这里是同一套规则的两个实现**，
//! 互为对照：一个实现瞎了，另一个还能抓住。
//!
//! 规则（与 Python 侧一一对应）：
//!
//!   R1  `nini_state` 里出现的每个 `crates/…` 路径都必须存在。
//!   R2  符号从**反引号**里抽（`identifier`）；只认「像 Rust 符号」的：
//!       大驼峰 / 全大写 / 含下划线的 snake_case；专有名词（Pi、LLM、JSON…）
//!       与单词（bool、copy）不算。
//!   R3  符号配给**紧挨着它**的文件引用（中间只有连接词）：「`path` 的 `sym`」
//!       与「`sym`（`path`）」两种写法都认。
//!   R4  配上了的显式符号必须能在**它配的那个文件**里 grep 到。
//!   R5  配不上的显式符号必须在整个 `crates/` 树里 grep 得到（外部符号除外）。
//!   R6  每条 gap 至少要有一个**能解析的锚点**（文件存在 + 符号在文件里）。
//!
//! 故意做坏的样例必须被抓出来——抓不到坏的检查等于没有检查，见
//! `a_bogus_anchor_must_be_rejected`。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value;

// ── 规则常量（与 scripts/ledger-anchors.py 保持一致） ────────────────────

/// 专有名词 / 缩写：不是 nini 代码里的符号。
const STOPWORDS: &[&str] = &[
    "AGENTS", "ANSI", "API", "ASCII", "BSD", "CD", "CI", "CLAUDE", "CPU", "CSS", "DLL", "EOF",
    "GPU", "GPT", "HTML", "HTTP", "HTTPS", "IDE", "IO", "JSON", "Known", "LICENSE", "LLM", "M1",
    "M2b", "MCP", "MIT", "MVC", "NPM", "OS", "OSC", "Pi", "PR", "RAM", "README", "RUST", "Rust",
    "SDK", "SQL", "SSH", "SSD", "TCP", "TOML", "TUI", "TTY", "UDP", "URI", "URL", "UTF", "VIM",
    "WASM", "XML", "YAML", "ESC",
];

/// **外部**符号（ratatui / crossterm 的 API）：不在 crates/ 里，但台账里
/// 合理地会提到，所以不进「全树 grep 不到 → failure」那条判据。
const EXTERNAL_SYMBOLS: &[&str] = &[
    // ratatui 的私有 API：crates/ 里当然没有（Cell / Buffer / Terminal /
    // Modifier / HYPERLINK 这些名字 nini 自己也用得到，所以只有这一个需要
    // 单独豁免）。
    "set_stringn",
];

/// 「紧挨着」的判定：符号与文件引用之间只允许出现这些连接词/标点。
const CONNECTOR_CHARS: &str = " \t的与和、：:（()），,。·—–-";

/// 配对窗口（字符）。
const PAIR_WINDOW: usize = 60;

/// 行号漂移的容差（行）。
const LINE_TOLERANCE: usize = 40;

// ── 小工具 ───────────────────────────────────────────────────────────────

fn repo_root() -> PathBuf {
    // crates/nini-core -> crates -> <repo>
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn is_symbol_like(t: &str) -> bool {
    if t.len() < 3 {
        return false;
    }
    if STOPWORDS.contains(&t) {
        return false;
    }
    let has_lower = t.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = t.chars().any(|c| c.is_ascii_uppercase());
    let has_underscore = t.contains('_');
    if !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    if has_upper && !has_lower {
        // SCREAMING_SNAKE（或全大写的缩写）
        return t.starts_with(|c: char| c.is_ascii_uppercase());
    }
    if has_lower && has_upper && !has_underscore {
        // UpperCamelCase
        return t.starts_with(|c: char| c.is_ascii_uppercase());
    }
    if has_lower && has_underscore && !has_upper {
        // snake_case（必须含下划线，否则就是普通英文单词）
        return t.starts_with(|c: char| c.is_ascii_lowercase());
    }
    false
}

/// 从一个反引号片段里抽出符号名：`A::B::C` 取最后一段。
fn segment_symbols(content: &str) -> Vec<String> {
    let content = match content.rsplit_once("::") {
        Some((_, last)) => last,
        None => content,
    };
    let ident = Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").unwrap();
    let mut out: Vec<String> = Vec::new();
    for m in ident.find_iter(content) {
        let tok = m.as_str();
        if is_symbol_like(tok) && !out.iter().any(|x| x == tok) {
            out.push(tok.to_string());
        }
    }
    out
}

/// 反引号里包的是**文件路径**时，它不是符号。
fn path_like(content: &str) -> bool {
    let c = content.trim();
    if !c.contains('.') {
        return false;
    }
    let last = c.rsplit('/').next().unwrap_or(c);
    match last.rsplit_once('.') {
        Some((_, ext)) => !ext.is_empty() && ext.chars().all(|ch| ch.is_ascii_alphanumeric()),
        None => false,
    }
}

/// 这段文字里只有连接词/标点（以及别的反引号片段）吗？
fn connector_only(text: &str) -> bool {
    let tick = Regex::new(r"`[^`\n]*`").unwrap();
    let stripped = tick.replace_all(text, "");
    // 反引号本身不算连接词（片段可能是半个，替换不掉）
    stripped
        .chars()
        .filter(|c| *c != '`')
        .all(|c| CONNECTOR_CHARS.contains(c))
}

// ── 抽取 ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Token {
    text: String,
    start: usize,
    end: usize,
    line: Option<usize>,
    is_crates_path: bool,
}

#[derive(Debug, Clone)]
struct Symbol {
    span_start: usize,
    span_end: usize,
    inner_start: usize,
    inner_end: usize,
    name: String,
    explicit: bool,
}

fn extract_tokens(text: &str) -> Vec<Token> {
    // 注：Rust 的 regex 不支持 look-around，所以不用 Python 侧那个
    // `(?<![\w./-])` 前瞻；字符类不含空格与中文，词首自然落在路径开头。
    let re = Regex::new(r"([A-Za-z0-9_.\-/]*?\.rs)(?::(\d+))?").unwrap();
    let mut out = Vec::new();
    for caps in re.captures_iter(text) {
        let whole = caps.get(0).unwrap();
        out.push(Token {
            text: caps.get(1).unwrap().as_str().to_string(),
            start: whole.start(),
            end: whole.end(),
            line: caps.get(2).and_then(|m| m.as_str().parse().ok()),
            is_crates_path: caps.get(1).unwrap().as_str().starts_with("crates/"),
        });
    }
    out
}

fn extract_symbols(text: &str) -> Vec<Symbol> {
    let tick = Regex::new(r"`([^`\n]+)`").unwrap();
    let ident = Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").unwrap();
    let mut out: Vec<Symbol> = Vec::new();
    let mut tick_spans: Vec<(usize, usize)> = Vec::new();

    for caps in tick.captures_iter(text) {
        tick_spans.push((caps.get(0).unwrap().start(), caps.get(0).unwrap().end()));
        let content = caps.get(1).unwrap().as_str();
        if path_like(content) {
            continue; // 文件路径，不是符号
        }
        let base = caps.get(1).unwrap().start();
        for sym in segment_symbols(content) {
            let idx = content.find(&sym).unwrap_or(0);
            out.push(Symbol {
                span_start: caps.get(0).unwrap().start(),
                span_end: caps.get(0).unwrap().end(),
                inner_start: base + idx,
                inner_end: base + idx + sym.len(),
                name: sym,
                explicit: true,
            });
        }
    }

    // 隐式（弱）锚点：裸标识符，且紧跟在「的 / 与 / 和 / 、 / ：」之后。
    let triggers = "的和、与：:（(,";
    for m in ident.find_iter(text) {
        let s = m.start();
        let e = m.end();
        if tick_spans.iter().any(|(a, b)| *a <= s && e <= *b) {
            continue;
        }
        let tok = m.as_str();
        if !is_symbol_like(tok) {
            continue;
        }
        let before = text[..s].trim_end();
        if before.is_empty() || !triggers.contains(before.chars().last().unwrap()) {
            continue;
        }
        out.push(Symbol {
            span_start: s,
            span_end: e,
            inner_start: s,
            inner_end: e,
            name: tok.to_string(),
            explicit: false,
        });
    }
    out
}

/// 配对：给每个符号配一个文件引用。
fn pair_symbols(text: &str, symbols: &[Symbol], tokens: &[Token]) -> Vec<(String, usize, Token)> {
    let mut out = Vec::new();
    for sym in symbols {
        let mut tiers: [Vec<(usize, Token)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for tok in tokens {
            let (between, dist, after) = if sym.span_end <= tok.start {
                (
                    &text[sym.span_end..tok.start],
                    tok.start.saturating_sub(sym.inner_end),
                    true,
                )
            } else if sym.span_start >= tok.end {
                (
                    &text[tok.end..sym.span_start],
                    sym.inner_start.saturating_sub(tok.end),
                    false,
                )
            } else {
                continue;
            };
            if dist > PAIR_WINDOW {
                continue;
            }
            if !connector_only(between) {
                continue;
            }
            let stripped: String = between
                .chars()
                .filter(|c| *c != '`')
                .collect::<String>()
                .trim()
                .to_string();
            if between.contains('的') {
                // 「…的 X」：X 属于**前面**那个路径
                if after {
                    continue;
                }
                tiers[0].push((dist, tok.clone()));
            } else if (stripped.starts_with('（') || stripped.starts_with('('))
                && stripped.chars().count() <= 8
            {
                // 「X（path）」：X 属于**后面**那个路径
                if !after {
                    continue;
                }
                tiers[1].push((dist, tok.clone()));
            } else {
                tiers[2].push((dist, tok.clone()));
            }
        }
        for tier in tiers.iter() {
            if let Some((_, tok)) = tier.iter().min_by_key(|(d, _)| *d) {
                out.push((sym.name.clone(), sym.inner_start, tok.clone()));
                break;
            }
        }
    }
    out
}

// ── 检查 ─────────────────────────────────────────────────────────────────

fn symbol_in_file(path: &Path, symbol: &str) -> (bool, Vec<usize>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (false, Vec::new());
    };
    let mut lines = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let mut from = 0usize;
        while let Some(pos) = line[from..].find(symbol) {
            let at = from + pos;
            let before_ok = at == 0
                || !line[..at]
                    .chars()
                    .last()
                    .map(|c| c.is_ascii_alphanumeric() || c == '_')
                    .unwrap_or(false);
            let after = at + symbol.len();
            let after_ok = after >= line.len()
                || !line[after..]
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_alphanumeric() || c == '_')
                    .unwrap_or(false);
            if before_ok && after_ok {
                lines.push(i + 1);
                break;
            }
            from = at + 1;
            if from >= line.len() {
                break;
            }
        }
    }
    (!lines.is_empty(), lines)
}

/// 整个 crates/ 树里有没有这个符号？
fn symbol_anywhere(all_rs: &[PathBuf], symbol: &str) -> bool {
    all_rs.iter().any(|p| symbol_in_file(p, symbol).0)
}

/// crates/ 下同名 .rs 文件 → 路径列表（解析裸文件名用）。
fn basename_index(root: &Path) -> (HashMap<String, Vec<PathBuf>>, Vec<PathBuf>) {
    let mut index: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut all: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                index.entry(name).or_default().push(path.clone());
                all.push(path);
            }
        }
    }
    (index, all)
}

/// 检查一条 gap 的 `nini_state`，返回失败原因（空 = 通过）。
fn check_gap(
    gap: &Value,
    root: &Path,
    basenames: &HashMap<String, Vec<PathBuf>>,
    all_rs: &[PathBuf],
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut failures: Vec<String> = Vec::new();
    let text = gap
        .get("nini_state")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    // R1：crates/ 路径必须存在。
    let path_re = Regex::new(
        r"\b(crates/[A-Za-z0-9_.\-/]*?\.(?:jsonl|json|toml|md|mjs|py|sh|rs))(?::(\d+))?",
    )
    .unwrap();
    for caps in path_re.captures_iter(text) {
        let rel = caps.get(1).unwrap().as_str();
        if !root.join(rel).is_file() {
            failures.push(format!("nini_state 指向的文件不存在：{rel}"));
        }
    }

    let tokens = extract_tokens(text);
    let symbols = extract_symbols(text);
    let pairs = pair_symbols(text, &symbols, &tokens);

    let mut paired_names: HashSet<String> = HashSet::new();
    let mut resolved = 0usize;

    for (name, _pos, tok) in &pairs {
        paired_names.insert(name.clone());
        let target = if tok.is_crates_path {
            root.join(&tok.text)
        } else {
            let base = Path::new(&tok.text)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match basenames.get(&base) {
                Some(v) if v.len() == 1 => v[0].clone(),
                _ => continue, // 裸文件名无法唯一解析 → 不当作锚点
            }
        };
        let (found, lines) = symbol_in_file(&target, name);
        if found {
            resolved += 1;
            if let Some(line) = tok.line {
                if !lines.iter().any(|l| l.abs_diff(line) <= LINE_TOLERANCE) {
                    // 行号漂了：只警告，不判死（行号本来就会随代码变动）——
                    // 但**要说出来**，否则「行号还在，只是指错了地方」这件事
                    // 只有人肉看代码才发现。
                    warnings.push(format!(
                        "{}:{} 的行号漂了（`{}` 现在在 {:?}）",
                        target.strip_prefix(root).unwrap_or(&target).display(),
                        line,
                        name,
                        lines
                    ));
                }
            }
        } else if symbols.iter().any(|s| &s.name == name && s.explicit) {
            failures.push(format!(
                "{} 里没有符号 `{}`",
                target.strip_prefix(root).unwrap_or(&target).display(),
                name
            ));
        }
    }

    // R5：配不上的显式符号，整个 crates/ 树里也必须存在。
    for sym in &symbols {
        if !sym.explicit || paired_names.contains(&sym.name) {
            continue;
        }
        if EXTERNAL_SYMBOLS.contains(&sym.name.as_str()) {
            continue;
        }
        if !symbol_anywhere(all_rs, &sym.name) {
            failures.push(format!(
                "nini_state 里的符号 `{}` 在 crates/ 全树里都 grep 不到",
                sym.name
            ));
        }
    }

    // R6：至少一个能解析的锚点。
    if tokens.is_empty() {
        failures.push("nini_state 里没有任何 crates/ 路径锚点（纯散文，无法机器核对）".into());
    } else if resolved == 0 {
        failures.push(
            "nini_state 里没有一个能解析的锚点（文件存在 + 符号在该文件里 grep 得到）".into(),
        );
    }

    failures
}

fn load_ledger(root: &Path) -> Value {
    let text = std::fs::read_to_string(root.join("scripts/parity-gaps.json"))
        .expect("scripts/parity-gaps.json must exist");
    serde_json::from_str(&text).expect("scripts/parity-gaps.json must be valid JSON")
}

// ── 测试 ─────────────────────────────────────────────────────────────────

#[test]
fn every_gap_nini_state_has_a_resolvable_code_anchor() {
    let root = repo_root();
    let ledger = load_ledger(&root);
    let basenames = basename_index(&root);
    let gaps = ledger
        .get("gaps")
        .and_then(|v| v.as_array())
        .expect("gaps array");

    let mut failures: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for gap in gaps {
        let id = gap.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        for f in check_gap(gap, &root, &basenames.0, &basenames.1, &mut warnings) {
            failures.push(format!("{id}: {f}"));
        }
    }
    // 报告（不判死）：行号漂了要说出来——「行号还在，只是指错了地方」
    // 这种事只有人肉看代码才发现。
    for w in &warnings {
        println!("行号漂移（不判死）：{w}");
    }
    assert!(
        failures.is_empty(),
        "台账的 nini_state 里有锚点对不上代码（{} 条）：\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn a_bogus_anchor_must_be_rejected() {
    // 故意做坏的样例：这个检查抓不到错的话，它就只是装饰。
    let root = repo_root();
    let basenames = basename_index(&root);

    let cases: Vec<(&str, &str)> = vec![
        (
            "指向的文件不存在",
            "crates/nini-tui/src/there_is_no_such_file.rs 的 `cmd_clone`",
        ),
        (
            "符号在文件里不存在",
            "crates/nini-core/src/paths.rs 的 `no_such_symbol_at_all`",
        ),
        ("纯散文，没有任何锚点", "已经修好了，行为符合 Pi 的语义。"),
        (
            "文件存在但一个符号都没有",
            "crates/nini-core/src/paths.rs 里的字段都改了",
        ),
    ];

    for (label, state) in cases {
        let gap = serde_json::json!({
            "id": "T-BOGUS",
            "status": "open",
            "nini_state": state,
            "oracle": "none",
            "exit_criterion": "合成样本",
            "machine_checkable": true,
        });
        let failures = check_gap(&gap, &root, &basenames.0, &basenames.1, &mut Vec::new());
        assert!(
            !failures.is_empty(),
            "故意做坏的样例没有被抓住（{label}）：{state}"
        );
    }
}

#[test]
fn a_good_anchor_must_be_accepted() {
    // 正样本：检查不能变成「什么都报错」。
    let root = repo_root();
    let basenames = basename_index(&root);
    let good = serde_json::json!({
        "id": "T-OK",
        "status": "open",
        "nini_state": "crates/nini-core/src/paths.rs 的 `new_session_path`",
        "oracle": "none",
        "exit_criterion": "合成样本",
        "machine_checkable": true,
    });
    let failures = check_gap(&good, &root, &basenames.0, &basenames.1, &mut Vec::new());
    assert!(failures.is_empty(), "正样本竟然失败了：{failures:?}");

    // 符号写在路径前面（中文常见的「`符号`（`路径`）」写法）也要认。
    let backward = serde_json::json!({
        "id": "T-OK-2",
        "status": "open",
        "nini_state": "`new_session_path`（`crates/nini-core/src/paths.rs`）",
        "oracle": "none",
        "exit_criterion": "合成样本",
        "machine_checkable": true,
    });
    let failures = check_gap(
        &backward,
        &root,
        &basenames.0,
        &basenames.1,
        &mut Vec::new(),
    );
    assert!(
        failures.is_empty(),
        "正样本（符号在前）竟然失败了：{failures:?}"
    );
}

#[test]
fn the_ledger_still_has_the_same_number_of_gaps() {
    // 台账条数变了，说明有人加/删了差距 —— 那次改动应该顺手更新本文件
    // 里的期望，而不是让这条断言一直绿着。
    let root = repo_root();
    let ledger = load_ledger(&root);
    let n = ledger
        .get("gaps")
        .and_then(|v| v.as_array())
        .map(|v| v.len())
        .unwrap_or(0);
    assert_eq!(n, 31, "台账的 gap 条数变了（现在是 {n}）——请同步更新本测试");
}
