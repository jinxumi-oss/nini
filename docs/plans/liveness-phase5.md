# 存活指示器 第五期：/settings 能用 + 阈值热生效 + 独立验收的修复

> 日期：2026-11-21
> 来源：第四期之后剩下的三件事（/settings 是惰性的、阈值要 /reload 才生效、配置生效链路有洞）
> 执行方式：主 agent 直改 + 2 个 agent（一个实现 /settings 接线，一个独立验收）
> 状态：完成

---

## 0. 本轮三件事

| # | 事项 | 结果 |
|---|---|---|
| A | **/settings 从惰性变成能用** | 选中分类 → 打开对应子选择器（model / thinking / theme），选中即生效；新增只读行「存活阈值」 |
| B | **阈值热生效** | 每轮开始重读 settings.json 的 `liveness` 段 —— 改完文件不用重启，也不用 /reload |
| C | **独立验收发现的缺陷** | 3 个新缺陷（其中 1 个是我自己刚写的代码）+ 1 个既有缺陷，修掉 |

---

## 1. A：/settings 接上子选择器

**之前**：`crates/nini-tui/src/selectors/settings.rs` 的模块 doc 自承 “picking a category
does nothing” —— 选了 model / theme / thinking 什么都不发生。

**现在**：

```
/settings
  model              (unset)          → Enter → 打开模型选择器（与 /model 同一目录）
  theme              light            → Enter → 打开主题选择器（dark / light + 自定义主题文件）
  thinking           (unset)          → Enter → 打开思考档位选择器
  存活阈值           provider 响应头 60s • 流空闲 120s • 等模型 60s • 等工具 180s（改 settings.…）
                                        → Enter → 推一条 notice 指向 settings.json（只读，本轮不做交互式数值编辑）
```

实现：新增 `selectors/theme.rs`（`ThemeSelector`）；runtime 里把「分类 → 子选择器」
抽成可测的 `chained_selector()`，接上子选择器后**不再**走 `apply_selector_result`。

主题写盘走 `SettingsManager::load_from_disk` → `set_theme` → `flush()`（merge 式，
用户自己写的 `retry` / `liveness` 等键保留）。

## 2. B：阈值热生效

`submit_user_input_inner` 在**锁外**重读一次 settings.json 的 `liveness` 段：

* 只在**确实有 settings 文件**时才重读 —— `load_settings` 在文件缺失时返回全默认，
  照搬会把用户已配的阈值抹成默认；
* 写回 RunState 的是**清洗后**的生效值（见 §3 的 E-1）；
* 校验警告只推**新增**的那些（见 §3 的 E-2）。

读在锁外：settings.json 很小，但持锁做文件 IO 会把整个 TUI 卡住。

## 3. C：独立验收（verifier agent）发现的缺陷与修复

验收报告：[`docs/reviews/liveness-verification-2026-11-20.md`](../reviews/liveness-verification-2026-11-20.md)。
它用 tmux + 挂死的本地 provider 实测，并做了**断言有效性**验证（把热生效写回
短路成 `if false && …`，同一复现命令不再出现告警 → 证明测的不是空跑）。

| 编号 | 严重性 | 缺陷 | 修法 |
|---|---|---|---|
| E-1 | **高** | 非法值 `0` 泄漏进渲染阈值：`runtime.rs` 与 `/reload` 把**未清洗**的 `lv` 写进 `RunState` → 界面立刻永久 `⚠ no progress`，而 `/status` 显示 60s（自相矛盾） | 写进 RunState 的一律取 `nini_core::liveness::liveness_settings()`（`set_liveness_settings` 内部已清洗），`/status` 与界面从此同源 |
| E-2 | 中 | 校验警告每轮重复刷屏（比较用「原值 vs 清洗后全局」，0 值永远不等 → 每次都 set + 推全部警告） | 每次 set 后只推**新增**的警告（与 set 前的警告集合做差集） |
| E-3 | 中 | TUI 模式下同一句警告既走 stderr 又进转录 | stderr 只在**非 TUI 模式**打印（判据：`--print` 或子命令）；TUI 模式由 `AppState::with_settings` 推进转录 |
| E-5 | 中（既有） | `model_state.settings_path` 生产路径上恒为 `None` → `/reload` 永远回答「no settings file to reload」；`/settings` 显示编造值；写进磁盘的主题下次启动不保留 | nini-cli 的 bootstrap 里给出 `settings_path` 并从文件加载 `settings_snapshot` → `/reload` 有效、`/settings` 显示真值、启动主题正确（实测状态栏 `[light]`） |
| 附带（既有） | — | 选模型不落盘：`apply_selector_result` 里建 `SettingsManager::default()`、写进去、丢掉 | 改成读真实 settings 文件 → 改 → `flush()`；实测 `settings.json` 出现 `"default_model": "anthropic/claude-opus-4-7"` |

## 4. 验收证据（实测）

| 项 | 证据 |
|---|---|
| 热生效 | 改 settings.json → 新一轮 **3.12s** 出现 `⚠ no progress 3s · waiting for model`（断言有效性已验） |
| 默认不误报 | 12s 内不出现 `⚠ no progress`（默认等模型 60s） |
| 0 值不泄漏 | `no_progress_model_secs: 0` → 底部 `⠋ waiting for model · 7s`（按 60s 跑，不是立刻报警），启动时只报一次警告 |
| TUI 不重复 | TUI 画面里 `nini: ` 出现 0 次 |
| /settings 显示真值 | `theme light`（来自文件，不是编造的 dark）；选中 model → 打开「Switch model」 |
| 主题持久 | 状态栏右侧 `[light]`（启动即读文件） |
| 模型落盘 | `settings.json` 出现 `"default_model": "anthropic/claude-opus-4-7"` |

门禁：`cargo test --workspace` **2337 passed / 0 failed**；`fmt --check` 0 差异；
`clippy --workspace --all-targets -D warnings` 干净。

## 5. 明确不做（边界）

* `/settings` 里**交互式编辑数值**（存活阈值只读 + 指路）—— 需要一套数值输入 UI，
  与本轮主题（状态可见）无关。
* **文件监视**（改 settings.json 自动生效，不等下一轮）—— 目前是「每轮开始重读」，
  进行中的回合改配置要等下一轮才生效。
* **`/settings` 之外的新命令**。
* Pi 的 camelCase 设置键（`defaultModel` 等）在 nini-tui 的 Settings 里读不到
  （nini-core 那套有 alias，nini-tui 这套没有）—— 既有兼容缺口，单独一轮。
