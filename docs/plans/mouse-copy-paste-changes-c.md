# Changes
- `crates/nini-tui/src/settings.rs:33` | 新增公开常量 `COPY_ON_SELECT_DEFAULT: bool = true`（Pi `tui-alt-screen.ts:272`：`copyOnSelect ?? true`，默认开启） | 计划步骤 R5 | 类型: 变更
- `crates/nini-tui/src/settings.rs:62-71` | `Settings` 结构体加 `copy_on_select: bool` 字段，带 `#[serde(default = "default_copy_on_select")]`（老 settings.json 缺该字段仍可解析） | 计划步骤 R5 | 类型: 变更
- `crates/nini-tui/src/settings.rs:86` | 新增默认函数 `fn default_copy_on_select() -> bool { true }`（与既有 `default_context_window` 等同风格） | 计划步骤 R5 | 类型: 变更
- `crates/nini-tui/src/settings.rs:99` | `Settings::default()` 的构造体补上 `copy_on_select: default_copy_on_select()`，保证 default 路径也一致 | 计划步骤 R5 | 类型: 变更
- `crates/nini-tui/src/settings.rs:121-126` | 新增自由函数 `pub fn copy_on_select_from_settings(path: Option<&std::path::Path>) -> bool`：走既有 `SettingsManager::load_from_disk`（缺失/解析失败/字段缺失 → `Self::default()`）；`path` 为 None 直接回退默认值，`load_from_disk` 收 `PathBuf` 故按 `PathBuf::from(p)` 传入 | 计划步骤 R5 | 类型: 变更
- `crates/nini-tui/src/settings.rs:527-582` | 新增 5 条单测：默认值为 true、文件缺失（None + 不存在路径）→ true、`copy_on_select:false` → false、`true` → true、旧 settings 文件（无该字段）→ true 且整份仍能正常解析（`context_window` 仍为默认 200000）、畸形文件（字段类型非法）→ true | 计划步骤 R5 §4 测试 C | 类型: 变更

## Commands
- `cargo fmt -p nini-tui` → 退出码 0
- `cargo fmt -p nini-tui -- --check` → 退出码 0
- `cargo clippy -p nini-tui --all-targets -- -D warnings` → 退出码 0
- `cargo test -p nini-tui` → 退出码 0
- `cargo test -p nini-tui copy_on_select` → 退出码 0（改动前该命令匹配 0 个测试，即功能未实现）

## Reproduction
- 复现命令: `cargo test -p nini-tui copy_on_select` → 退出码 0（红→绿）：改动前 0 个匹配测试（未实现），改动后 5 个测试全过（`settings::theme_resolution_tests` / 新增 5 条 copy_on_select 用例，0 failed）。

## Blocked
none
