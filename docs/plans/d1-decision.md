# D1 判定书：Pi 有、nini 无的一批终端/诊断小项

> 台账条目：`scripts/parity-gaps.json` 的 `D1`（P3，round W2）。
> 本文件是该条目的**逐条判定书**：6 个小项，每一项都写清楚「做什么 / 为什么不做什么 /
> 理由」，并给出 Pi 与 nini 两边的 `file:line`。
>
> 退出判据（原文）：
> > 本条 6 个小项逐条在 crates/ 里有对应符号，或在本台账里被标记为「明确不做」并写明理由
> > （每条都必须可 grep 到符号或台账条目）。
>
> 结论：**3 项做（符号已就位，待接线进运行时）**，**3 项明确不做（理由见下）**。

---

## 0. 一览

| # | Pi 的能力 | Pi 位置（0.85.1 快照） | 决定 | nini 落点 |
|---|---|---|---|---|
| 1 | `updateTerminalTitle` | `src/ui/interactive-mode.ts#updateTerminalTitle`（调用点 4 处） | **做** | `crates/nini-tui/src/terminal_title.rs` |
| 2 | `checkTmuxKeyboardSetup` | `src/ui/interactive-mode.ts#checkTmuxKeyboardSetup`（调用点 1 处） | **做** | `crates/nini-tui/src/tmux_setup.rs` |
| 3 | `onRightClickPaste` | `src/ui/interactive-mode.ts#handleRightClickPaste`；终端层 `src/tui/tui-alt-screen.ts#handleRightClickPaste` | **明确不做** | 台账条目 + 本文件 §3 |
| 4 | `addCacheMissNotice` | `src/ui/interactive-mode.ts#addCacheMissNotice`（调用点 2 处）；判定在 `core/cache-stats.ts`（**该文件不在快照里**） | **明确不做** | 台账条目 + 本文件 §4 |
| 5 | `checkForPackageUpdates` | `src/ui/interactive-mode.ts#checkForPackageUpdates`（调用点 1 处） | **明确不做** | 台账条目 + 本文件 §5 |
| 6 | `formatDiagnostics` | `src/ui/interactive-mode.ts#formatDiagnostics`（调用点 4 处） | **做（缩小版）** | `crates/nini-tui/src/tmux_setup.rs`（`format_diagnostics`） |

快照路径：`references/pi-0.85.1/`（`SNAPSHOT.lock` 锁定的 0.85.1）。

---

## 1. `updateTerminalTitle` —— 做

### Pi 的行为

`references/pi-0.85.1/src/ui/interactive-mode.ts#updateTerminalTitle`：

```ts
private updateTerminalTitle(): void {
    const cwdBasename = path.basename(this.sessionManager.getCwd());
    const sessionName = this.sessionManager.getSessionName();
    if (sessionName) {
        this.ui.terminal.setTitle(`${APP_TITLE} - ${sessionName} - ${cwdBasename}`);
    } else {
        this.ui.terminal.setTitle(`${APP_TITLE} - ${cwdBasename}`);
    }
}
```

即标题的形状是 `<产品名> - <会话名> - <cwd basename>`，没有会话名时退化成两段。
`APP_TITLE` 从 `config.js` 导入（`interactive-mode.ts` 顶部的 import），**该模块不在快照里**，
所以字面值不可从快照验证；可验证的是**形状**，nini 复刻的也正是形状。

### nini 的做法与偏差

`crates/nini-tui/src/terminal_title.rs`：

| 符号 | 位置 | 作用 |
|---|---|---|
| `APP_TITLE` | `terminal_title.rs:51` | `"nini"`，对应 Pi 的 `APP_TITLE` |
| `cwd_basename` | `terminal_title.rs:69` | Node `path.basename` 的语义（不是 `Path::file_name`） |
| `terminal_title` | `terminal_title.rs:98` | 纯函数：算出标题文本 |
| `update_terminal_title` | `terminal_title.rs:114` | 把标题交给**注入的写入器**（`FnMut(&str)`） |
| `write_title_to_terminal` | `terminal_title.rs:127` | 真正的落盘口：crossterm `SetTitle` |
| `set_terminal_title` | `terminal_title.rs:139` | 两者的组合 |
| `TitleRecorder` | `terminal_title.rs:148` | 测试用记录器 |

两处**有理由的偏差**：

1. **会话标识用 session id 代替 session name。**
   nini 根本没有「会话名」这个概念：会话的身份是 `u_<uuid>`
   （`crates/nini-session/src/lib.rs:166`，`SessionHeader::new`）。
   Pi 的 session name 是用户可改的标题；nini 没有对应物，
   拿 id 近似是诚实的——它唯一、它就是磁盘上那个文件的名字，
   而且不会凭空造一个用户在别处看不到的名字。
   没有会话时（会话文件还没落盘）退化成 Pi 的第二形态 `nini - <cwd basename>`。
2. **`cwd_basename` 手写而不复用 `Path::file_name`。**
   `Path::file_name("/")` 是 `None`，Node 的 `path.basename("/")` 是 `"/"`；
   尾斜杠、空段的行为也不同。规则写在自己的函数里，测试逐条钉住。

### 为什么可测

TUI 运行期间不得写 stdout（`crates/nini-tui/tests/no_terminal_writes.rs` 是硬不变式），
所以「写入」这一步被拆成**注入的写入器**：测试传一个记录闭包，
断言拿到的是**算出来的文本**；真正的 crossterm 落盘口
（`crossterm::execute!(stdout, SetTitle(..))`，crossterm 0.28.1
`src/terminal.rs:386`）在测试里只取地址、不调用。

---

## 2. `checkTmuxKeyboardSetup` —— 做

### Pi 的行为

`references/pi-0.85.1/src/ui/interactive-mode.ts#checkTmuxKeyboardSetup`，规则逐条照抄：

* 不在 tmux 里（`process.env.TMUX` 为空）→ 不检查、不警告（`:1163`）；
* `tmux show -gv extended-keys` 拿不到（tmux 不存在 / 沙箱 / 超时）→
  **不警告**（`:1195`，Pi 的注释原话是 "If we couldn't query tmux (timeout, sandbox, etc.), don't warn"）；
* `extended-keys` 既不是 `on` 也不是 `always` → 第一条警告（`:1198`）；
* `extended-keys-format` 是 `xterm` → 第二条警告（`:1203`）；
* 查询有 2 秒超时（`:1172`）。

两条警告的原文（第二句里的 "Pi" 换成 "nini"，其余一字不改）：

```text
tmux extended-keys is off. Modified Enter keys may not work. Add `set -g extended-keys on` to ~/.tmux.conf and restart tmux.
tmux extended-keys-format is xterm. nini works best with csi-u. Add `set -g extended-keys-format csi-u` to ~/.tmux.conf and restart tmux.
```

### 为什么这条对 nini 有实际价值

不是「Pi 有所以 nini 也要有」。nini 的 A4 条目把 **Alt+Enter 绑成 `followUp`**
（一条 pending 队列拆成 steering 与 follow-up 两队列），
而 **modified Enter 正是 tmux 在 `extended-keys` 关闭时会吞掉的键**。
用户按 Alt+Enter，什么都没发生，刚打的那句 follow-up 就没了——
这是一次静默的数据丢失，而修法是 `~/.tmux.conf` 里的一行。
警告的价值正落在这里。

### nini 的做法

`crates/nini-tui/src/tmux_setup.rs`：

| 符号 | 位置 | 作用 |
|---|---|---|
| `TmuxKeyboardStatus` | `tmux_setup.rs:81` | 结论枚举：`NotApplicable` / `Ok` / `ExtendedKeysOff` / `XtermFormat` / `Unknown` |
| `classify` | `tmux_setup.rs:104` | **纯函数**：`(in_tmux, extended_keys, extended_keys_format)` → 结论 |
| `warning_message` | `tmux_setup.rs:132` | 结论 → Pi 的警告文案（该警告就返回 `Some`） |
| `TmuxOptionQuery` | `tmux_setup.rs:153` | 查询接缝：测试注入假 tmux，不真跑 tmux |
| `TmuxCli` | `tmux_setup.rs:161` | 真 tmux：`tmux show -gv <option>`，2 秒超时（`DEFAULT_TIMEOUT`，`:169`） |
| `run_with_timeout` | `tmux_setup.rs:213` | 带超时的子进程执行：超时即 kill |
| `check_tmux_keyboard_setup` | `tmux_setup.rs:261` | 查询 + 分类（`in_tmux` 由调用方给，不去改进程的环境） |
| `check_tmux_keyboard_setup_env` | `tmux_setup.rs:279` | 运行时用的入口：读 `TMUX` 环境变量 + `TmuxCli` |

**超时是实的**：超时分支会把子进程 kill 掉。
测试不是拿假的执行器假装超时，而是拿一条**真的会挂住**的命令（`sleep 30`）去撞 200ms 的预算，
断言它在 2 秒内返回 `None`。

---

## 3. `onRightClickPaste` —— 明确不做

### Pi 的行为

`references/pi-0.85.1/src/ui/interactive-mode.ts#handleRightClickPaste`（字段与实现），
终端层在 `references/pi-0.85.1/src/tui/tui-alt-screen.ts#handleRightClickPaste`：

```ts
private handleRightClickPaste(event: SgrMouseEvent): boolean {
    if (
        !this.onRightClickPaste ||
        process.platform !== "win32" ||          // ← 只有 win32
        process.env.TERM_PROGRAM?.toLowerCase() === "vscode" ||
        event.release ||
        event.button !== 2
    ) {
        return false;
    }
    ...
}
```

### 不做的理由

1. **Pi 自己的这一项就是 win32 专有的。**
   `process.platform !== "win32"` 直接 `return false`——
   在 macOS 和 Linux 上，Pi 的右键粘贴**根本不存在**。
   复刻一个只在 Windows 上存在的能力，对 nini 的用户没有意义。
2. **nini 只发 Linux / macOS。**
   `install.sh:38`：`unsupported OS: nini currently ships Linux + macOS binaries`。
3. **nini 没有剪贴板「读」的能力。**
   `crates/nini-tui/src/clipboard.rs` 只有 `copy`（写），没有 read；
   `arboard` 依赖也只开了 `image-data` 特性。
   Pi 那一项要读剪贴板文本再 bracketed paste 注入，nini 现在做不到。

**可 grep 的台账条目**：`scripts/parity-gaps.json` 的 `D1.nini_state` 里
「明确不做 ①」一段。

---

## 4. `addCacheMissNotice` —— 明确不做

### Pi 的行为

`references/pi-0.85.1/src/ui/interactive-mode.ts#addCacheMissNotice`（定义与 2 处调用）：
缓存未命中累计到一定量（≥ 20k token 或 $0.1）时，在转录里插一条黄色提示。
判定逻辑在 `core/cache-stats.ts`——**该文件不在快照里**（快照的 `src/core/` 只有
`settings-manager.ts`），所以阈值只能从 `interactive-mode.ts` 的调用点看到。

### 不做的理由

**nini 从不发缓存标记，所以没有「未命中」可报。**
全仓 grep `cache_control` / `cache_creation`（请求侧）：零命中；
`crates/nini-ai/src/anthropic.rs:475` 附近的 `cache_creation_input_tokens`
只是**响应里的计数字段**（读回来记账），不是请求里发的 `cache_control` 断点。
没有断点就没有未命中，没有未命中就没有这条提示——
这不是「没做」，是**没有那个量**。

**可 grep 的台账条目**：`scripts/parity-gaps.json` 的 `D1.nini_state` 里
「明确不做 ②」一段。

---

## 5. `checkForPackageUpdates` —— 明确不做

### Pi 的行为

`references/pi-0.85.1/src/ui/interactive-mode.ts#checkForPackageUpdates`：
启动时用 npm 查扩展包有没有新版本，有就弹一条更新提示
（注意它还有 `PI_OFFLINE` 开关，`:1144`）。

### 不做的理由

1. **与 nini 的定位直接冲突。**
   `README.md:50-51`：「**No account, no cloud, no phone-home.** There is no login, no
   telemetry, no sync, **no updater**.」——`checkForPackageUpdates` 就是 updater 的一半。
2. **它要联网。** nini 的网络面只对模型/provider 开放；
   为了报一个「你的扩展包有新版」去敲 npm  registry，是一次与用户无关的外部请求。
3. **nini 没有 npm 扩展包这个概念。** nini 的扩展是本机的 `cdylib`
   （`crates/nini-ext/src/loader.rs:35` 扫 `*.so` / `*.dylib` / `*.dll`），
   不存在「包版本」这回事。

**可 grep 的台账条目**：`scripts/parity-gaps.json` 的 `D1.nini_state` 里
「明确不做 ③」一段。

---

## 6. `formatDiagnostics` —— 做（缩小版）

### Pi 的行为

`references/pi-0.85.1/src/ui/interactive-mode.ts#formatDiagnostics`：

* 诊断分两类：`collision`（同名冲突，有 `collision.name` / `winnerPath` / `loserPath`）
  与其它（`error` / `warning`，可带 `path`）；
* **冲突按名字分组**（`:1592-1602`，`Map` 按插入序），
  每组一个块：标题 `  "<name>" collision:`、胜者 `    ✓ <path>`、
  每个落败者 `    ✗ <path> (skipped)`；
* 其余诊断在冲突之后按原序输出：有 path 的一行 path、一行 message；
  没有 path 的只有一行 message（`:1640-1648`）；
* 最后 `lines.join("\n")`。

Pi 覆盖四类资源（skills / prompts / themes / extensions）。

### nini 的缩小版

nini **没有扩展运行时**（`crates/nini-ext` 未被 `nini-tui` 依赖，
Pi 的 extensions 在 nini 这边不存在），
所以只覆盖 nini 启动时真正加载的两类资源：

* **skills** —— `crates/nini-core/src/skills.rs`（`load_skills_from_roots`，
  `:271` 按名字去重，落败者被静默丢弃）；
* **prompts** —— `crates/nini-core/src/prompt_template.rs`
  （`PromptTemplateDiagnostic`，`:47`；`LoadResult.diagnostics`，`:106`）。

落点：`crates/nini-tui/src/tmux_setup.rs`：

| 符号 | 位置 | 作用 |
|---|---|---|
| `ResourceKind` | `tmux_setup.rs:290` | `Skill` / `Prompt`——**只有这两类**，这是「缩小」的边界 |
| `DiagnosticSeverity` | `tmux_setup.rs:309` | `Error` / `Warning` |
| `ResourceDiagnostic` | `tmux_setup.rs:318` | `Collision { kind, name, winner, loser }` / `Load { kind, severity, path, message }` |
| `format_diagnostics` | `tmux_setup.rs:367` | 纯函数：诊断列表 → 可读文本 |

**与 Pi 的两处有理由的偏差**：

1. **分组键是 `(kind, name)`，不是 `name`。**
   skills 与 prompts 是**两个独立命名空间**：一个叫 `review` 的 skill
   和一个叫 `review` 的 prompt 模板**并不冲突**。
   按名字混在一组，等于告诉用户一个假冲突。
2. **标题里带 kind**：`  "<name>" collision (skill):`。
   理由同上——两个命名空间可以出现同一个名字，
   两个一模一样的标题说不清哪个是哪个。

输出是纯文本（Pi 那里带颜色，nini 的格式化器不负责上色，交给调用方）。

---

## 7. 剩下的那一步：接线

三个「做」的符号**都已就位，但还没有接线进运行时**——
`crates/nini-tui/src/runtime.rs` 是别的会话的地盘，本轮**没有改它**。
所以 D1 的 `status` 仍然是 `open`，不是 `fixed`。

具体的接线点（留给 runtime 的主人）：

| 符号 | 建议接线位置 | 备注 |
|---|---|---|
| `terminal_title::set_terminal_title` | `crates/nini-tui/src/runtime.rs:757`（`execute!(stdout, EnterAlternateScreen)?` 之后） | 启动时设置一次；会话 id 产生后（第一个 turn 落盘会话文件时）再设一次；`/cd` 之类改变 cwd 时再设一次 |
| `tmux_setup::check_tmux_keyboard_setup_env` | `crates/nini-tui/src/runtime.rs:725` 附近（ambiguous-width 探测之后、alternate screen 之前） | 结果经 `warning_message` 转成文本，用 `push_notice(&shared, …)`（`runtime.rs:99`）进转录——**不要**直接写终端，那会破坏 no-terminal-writes 不变式 |
| `tmux_setup::format_diagnostics` | 加载 skills / prompts 之后（CLI 侧或 TUI 侧 whichever 负责加载） | 冲突与加载错误汇总成一段文本；nini 的 skills 加载器目前**静默丢弃**同名落败者（`crates/nini-core/src/skills.rs`），要出这条提示还得让它把落败者报出来 |

**接线完成前，D1 不能标 fixed。**

---

## 8. 本轮实际执行的命令与退出码

| 命令 | 退出码 |
|---|---|
| `cargo test -p nini-tui --test d1_terminal_diagnostics` | 0（8 条） |
| `cargo test -p nini-tui --lib` | 0（362 条） |
| `python3 scripts/parity-gap.py --json` | 0 |
| `cargo fmt --all --check` | 0 |
| `cargo clippy -p nini-tui --all-targets -- -D warnings` | 0 |

### 变异注入（证明断言能红）

| 变异 | 变红的断言 |
|---|---|
| 标题分隔符 ` - ` → ` : ` | `terminal_title_has_the_pi_shape`、`the_title_is_handed_to_an_injected_writer` |
| `classify` 不再接受 `always` | `the_tmux_check_covers_every_pi_branch` |
| `format_diagnostics` 只按名字分组（忽略 kind） | `resource_diagnostics_are_grouped_by_name_and_readable` |
| `cwd_basename` 不去尾斜杠 | `cwd_basename_takes_the_last_path_segment` |
| 超时deadline 晚 2 秒 | `a_tmux_query_that_never_answers_does_not_warn_and_does_not_hang` |
