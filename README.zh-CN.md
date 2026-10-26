# nini

> 一个干净的 Pi agent，用 Rust 重写——作者力求保留 Pi agent 最初的原滋味。
>
> A clean Pi agent, rewritten in Rust — the author's aim is to keep Pi's
> original flavor.

[English](README.md) | **简体中文**

[![Status](https://img.shields.io/badge/status-v0.10.9-green)](https://github.com/jinxumi-oss/nini/releases)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-1633%20passing-green)](https://github.com/jinxumi-oss/nini/actions)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange)](https://www.rust-lang.org)

nini 是 [Pi](https://github.com/earendil-works/pi)（Mario Zechner 写的
AI coding agent）的 **clean-room Rust 重写版**。它读同一份 `~/.pi/agent/`
配置，写同一种 session JSONL v4，暴露同样的 23 条斜杠命令，外加 5 条自己的。
把它和 Pi 并排放着，你现有的 skills、settings 和 models.json 照常可用。

nini 不是想做一个"不一样的 agent"。它就是 **Pi，只不过用 Rust 写**——
作者的目标是保留 Pi 最初的原滋味：同一份配置、同样的 session 文件、
同样的命令、同样的手感，不多加东西，也不少拿东西。

> nini is not trying to be a different agent. It is Pi, in Rust — and the
> author's aim is to keep Pi's original flavor.

这里"干净"是两层意思，两层都是字面意义上的：它是 **clean-room** 重写
（照着 Pi 的可观察行为和文件格式写，不看 Pi 的源码），同时它也是一个
**干净** 的 agent：没有账号、没有云、没有遥测、没有功能膨胀，凡 Pi 没做的事
nini 也不做。

### 保留 Pi 最初的原滋味 (Keeping Pi's original flavor)

具体就是下面这些，不多也不少：

- **同一份 `~/.pi/agent/`。** 你的 skills、`settings.json`、`models.json`
  原样读取；nini 读写的都是同一套 session JSONL v4——**真实的 Pi 会话语料
  读完 0 行丢失**（conformance 套件实测；在 v0.10 之前 nini 会丢掉其中 48%）。
- **同样的 23 条斜杠命令**，同样的分发方式。其中 `/share` 还没实现，
  它会直说而不是假装实现了；另外 5 条 nini 自有命令是**增补，不是替换**。
- **没有账号、没有云、不会打电话回家。** 没有登录、没有遥测、没有同步、
  没有自动更新。唯一的网络流量就是你配置的模型 provider；`/login` 只是
  告诉你该设哪个环境变量。
- **一个可执行文件。** 不需要 Node 运行时、包管理器或守护进程——纯 MIT，
  无任何授权附加条款。
- **同样的手感。** Pi 的键位、流式节奏、命令面板、transcript：只是换
  ratatui 来画，画出来还是 Pi。

## 为什么用 nini

- **单个 6.4 MiB 可执行文件**（压缩后 2.4 MB），取代一整套 Node.js 工具链
- **无授权附加条款**——纯 MIT，不像某些带着 OpenAI/Anthropic 再分发限制的分支
- **1633 个单元 + 集成测试**，含基于属性的 SSE 模糊测试，以及会真的拉起
  HTTP 服务器的端到端线格式测试
- **内置工具可扩展**——bash、read、write、edit、grep、find
- **流式 TUI**，带斜杠命令自动补全

## 功能

| 领域 | 已实现 |
|---|---|
| **Provider** | Anthropic、OpenAI（Chat + Responses）、OpenAI-compatible（任何 `/v1/chat/completions`）、fixture（离线测试） |
| **默认模型** | `MiniMax-M2.7-highspeed`——用 `--model` 或 `/model` 覆盖 |
| **工具** | `bash`（进程树 kill、超时、SIGTERM→SIGKILL）、`read`、`write`（原子写）、`edit`（精确匹配）、`grep`（正则 + .gitignore）、`find`（glob + .gitignore） |
| **TUI** | ratatui + crossterm，readline 式编辑、kill ring、历史回溯、斜杠命令补全下拉、命令面板（Ctrl+Shift+K）、transcript 搜索（Ctrl+F）、外部编辑器（Ctrl+G） |
| **斜杠命令** | Pi 全部 23 条内建命令（`/model`、`/export`、`/new`、`/quit`……），外加 5 条 nini 独有（`/help`、`/status`、`/prompt`、`/debug`、`/editor`） |
| **Session** | JSONL v4 编解码 + 旧版 v3 读取；读 `~/.pi/agent/skills/`、`settings.json`、`models.json` |
| **压缩** | LLM 摘要 + 确定性兜底；上下文溢出时自动触发 |
| **Hooks（v0.7）** | 与 Pi 兼容的 5 方法 `AgentLoopHooks` trait：steering / followup / transform_context / convert_to_llm / should_stop_after_turn。工具执行前后的生命周期 hook。BashOperations + ReadOperations 让后端可 mock。 |

## 快速开始

### 安装

```bash
curl -fsSL https://raw.githubusercontent.com/jinxumi-oss/nini/main/install.sh | bash
```

安装脚本把 `nini`（以及给旧版 pi 安装用的 `nini` 启动器）放进
`~/.local/bin`。如果它还不在 `PATH` 里，把这个目录加进去。

### 不配 API key 先跑起来（fixture provider）

```bash
nini -p "echo hello"
nini demo "find TODOs and fix them"
```

fixture provider 返回写好的脚本回复，让你不花一个 token 也能走遍每条代码路径。

### 接真 provider

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
nini --provider anthropic -p "refactor the auth middleware"

# 或者用环境变量
export NINI_PROVIDER=openai
export OPENAI_API_KEY="sk-..."
nini -p "summarize this codebase"

# OpenAI-compatible（vLLM、OpenRouter、Together 等）
export NINI_PROVIDER=openai-compat
export OPENAI_BASE_URL="http://localhost:8000/v1"
export OPENAI_API_KEY="local"
nini -p "what does this function do?"
```

### 交互式 TUI

```bash
nini
```

你会得到一个四块式终端界面：状态栏、transcript、输入编辑器、按键提示。
输入 `/` 唤出斜杠命令补全。`Ctrl+C` 中断，`Ctrl+D` 退出。

## CLI 命令

| 命令 | 作用 |
|---|---|
| `nini` | 启动交互式 TUI（需要 TTY） |
| `nini -p "<task>"` | 单次 print 模式 |
| `nini demo [task]` | 脚本化自主演示（不需要 API key） |
| `nini info` | 显示已加载的 skills、settings 和 provider |
| `nini --provider <name>` | 指定 provider（`anthropic`、`openai`、`openai-responses`、`openai-compat`、`fixture`） |
| `nini --model <id>` | 覆盖默认模型 |
| `nini --help` / `--version` | 帮助 / 版本 |

## TUI 键位

| 按键 | 作用 |
|---|---|
| `Enter` | 发送输入 |
| `Shift+Enter` | 换行（多行输入） |
| `Esc` | 中断运行 / 清空输入 / 关闭弹层 |
| `Ctrl+C` | 中断当前操作 |
| `Ctrl+D` | 退出 nini（3 秒内按两次） |
| `Ctrl+L` | 切换模型 |
| `Ctrl+F` | 打开 transcript 搜索 |
| `Ctrl+G` | 用外部编辑器打开当前输入（$VISUAL/$EDITOR） |
| `Ctrl+Shift+K` | 打开命令面板 |
| `F1` | 切换短/长页脚 |
| `Ctrl+A` / `Ctrl+E` | 行首 / 行尾 |
| `Home` / `End` | transcript 跳到最旧 / 最新（与 Pi 一致） |
| `Ctrl+K` | 删到行尾 |
| `Ctrl+U` | 清空输入 |
| `Ctrl+W` | 向前删一个词 |
| `Alt+D` / `Alt+Y` | 向前删词 / 弹出删除 |
| `Ctrl+Y` / `Ctrl+/` | 粘贴 / **撤销** |
| `Ctrl+T` | 切换思考等级 |
| `Ctrl+P` / `Ctrl+Shift+P` | 下一个 / 上一个模型 |
| `Ctrl+O` | 折叠/展开工具输出 |
| `Ctrl+V` | 从剪贴板粘贴图片 |
| `PageUp` / `PageDown` / `Shift+End` | transcript 上翻 / 下翻 / 跳到最新 |
| `Ctrl+←` / `Ctrl+→` | 按词左右跳 |
| `↑` / `↓` | 在行间移动光标；到首/末行时回溯历史输入 |
| `Tab` | 接受选中的补全 |

## 斜杠命令

Pi `builtin.json`（v0.85.1）里的 23 条命令全部可识别，另有五条 nini 独有
的增补——`/help`、`/status`、`/prompt`、`/debug`、`/editor`——它们是
**扩展，不是对等**，不计入对等声明。标为 **stub** 的命令会输出"尚未实现"，
而不是假装干了什么。
nini 没有实现的开关或命令会**带着理由被拒绝**——绝不会收下然后丢掉。

| 命令 | 参数 | 状态 |
|---|---|---|
| `/settings` | — | 真实实现（选择器） |
| `/model` | `<provider/model>` | 部分实现（只改状态，不落盘） |
| `/tree` | — | 真实实现（session 的 ASCII 树） |
| `/thinking` | `<off\|minimal\|low\|medium\|high\|xhigh\|max>` | 部分实现（校验但不落盘） |
| `/scoped-models` | — | stub（没有按范围筛模型） |
| `/export` | — | 完整实现（写 HTML 到 `~/.pi/agent/exports/`） |
| `/import` | `<path>` | 真实实现 |
| `/share` | — | stub |
| `/copy` | — | 部分实现（打到 stdout，无剪贴板） |
| `/name` | `<name>` | 部分实现（只在内存，不落盘） |
| `/session` | — | 部分实现（内存状态，没有 session 文件） |
| `/changelog` | — | 真实实现 |
| `/hotkeys` | — | 完整实现（**运行时从键位表推导**） |
| `/fork` | `[n]` / `at n` | 真实实现（写出分支 session 文件） |
| `/clone` | — | 真实实现（复制 session 文件） |
| `/trust` | — | 部分实现（只在内存） |
| `/login` | `<provider>` | 列出 provider + 密钥该放哪 |
| `/logout` | — | 告诉你该取消哪个变量（无法改父 shell） |
| `/new` | — | 完整实现（清空 transcript） |
| `/compact` | — | 真实实现（session 层压缩） |
| `/resume` | — | 真实实现（session 选择器） |
| `/reload` | — | 完整实现（重读 skills） |
| `/quit` | — | 完整实现 |

## Pi 生态兼容性

nini 读写与 Pi v0.85.1 完全相同的文件格式：

| 内容 | 位置 |
|---|---|
| 用户级设置 | `~/.pi/agent/settings.json` |
| 项目级设置 | `./.pi/settings.json` |
| 用户级 skills | `~/.pi/agent/skills/<name>/SKILL.md` |
| 项目级 skills | `./.pi/skills/<name>/SKILL.md` |
| 模型 | `~/.pi/agent/models.json` 和 `./.pi/models.json` |
| 会话 | `~/.pi/agent/sessions/<project>/<timestamp>.jsonl` |
| 斜杠命令 | 名字与参数提示完全一致 |

你可以在一个终端跑 `nini -p`、另一个终端跑 `pi`——两边写的是同一种
session JSONL。

## 已知限制（截至 v0.10.4）

都是对着这个二进制验证过的，不是从旧 README 抄来的。项目目标与当前状态
写在 [`docs/GOALS.md`](docs/GOALS.md)——它是"nini 想成为什么"的权威；
本节只是面向用户的摘要。

- `/share` 是唯一一条返回"尚未实现"的命令。其余 23 条 Pi 命令都能分发；
  `/tree`、`/fork`、`/settings` 是真实实现
- 压缩用的是本地确定性摘要，不是基于 LLM 的摘要
- 命令行上的 `--continue-session` / `--session-id` / `--fork` 会被拒绝，
  并指向对应的 TUI 命令。**TUI 里开始的对话会落盘**：写在
  `~/.nini/agent/sessions/<project>/<timestamp>_<suffix>.jsonl`（Pi 的形状），
  footer 显示会话 id，`/resume` 能列出来。（v0.10.8 之前，对话退出即丢，
  除非先执行 `/new`——那是差距 **A5**）
- 没有基于 LLM 的**分支**摘要：`/fork` 会创建真正的分支，但 Pi 的
  `navigateTree`（在同一个文件里跳到另一个节点，并可选地总结被放弃的内容）
  尚未实现
- Pi 0.84.2 内置的 40 个 provider 里，nini 原生实现了 9 个（7 个品牌）；
  其余用 `models.json` 配置
- **你在 `models.json` 里声明的模型现在能被读懂了**（用 Pi 的拼写：
  `contextWindow`、`maxTokens`、`reasoning`），它们会出现在 `/model` 与
  `--list-models` 里，context window 用的就是你写下的那个数字。
  但**价格不随之而来**：`models.json` 不写费率，nini 也不会编一个——
  没有已知价格的模型不显示费用标签，读起来是「未知」，绝不是「免费」
- **内置工具集与 Pi 0.85.1 一致**——`read` / `write` / `edit` / `bash` / `grep` /
  `find` / `ls`，连**工具描述、参数 schema、prompt 贡献**都逐条对齐
  （对照物是 Pi 自己的工具定义在**运行时**的转储，见
  `references/pi-0.85.1/tool-contracts.json`）。唯一的例外是 **`powershell`**
  （Pi 里是 Windows 专用），nini 不提供；POSIX 下由 `bash` 覆盖同样的场景
- 不支持 OAuth / device-flow 授权——只用环境变量里的 API key
- 没有扩展运行时。这是**刻意**移出范围的：它当初被规定为 Rust cdylib，
  而 Pi 加载的是 TypeScript，两者没有共同的执行模型。见
  `docs/plans/v0.10-pi-parity-refactor.md` §26
- 提示词模板和 AGENTS.md / CLAUDE.md 上下文文件会被**带着理由拒绝**，
  而不是被悄悄忽略
- **没有语法高亮。** Pi 会给围栏代码块上色（`highlightCode`），write
  工具的 diff 里也一样；nini 只用基础样式渲染代码块。那九个 `syntax*`
  主题槽位存在但没人读——是预留，不是实现

## 架构

```
crates/
├── nini-core/   # Agent 循环、provider trait、tool trait、session、skills、
│                 #   settings、压缩、消息类型
├── nini-ai/      # Anthropic + OpenAI + OpenAI-Responses + OpenAI-compat +
│                 #   fixture provider + SSE 解析器
├── nini-tools/   # bash、read、write、edit、grep、find
├── nini-session/ # JSONL v4 编解码 + 旧版 v3 桥接
├── nini-tui/     # 按键 + 状态 + 渲染 + 运行时 + 斜杠命令 + 自动补全
├── nini-cli/     # `nini` 可执行入口
├── nini-conformance/  # Pi 对等 oracle——没有产品代码，只有
│                 #   关于 Pi 的、会变红的陈述
└── nini-ext/     # 空。留在 workspace 里，并带一个说明为什么的头注释；
                  #   它的路线已被证伪（见计划 §26）
```

依赖方向是 `nini-cli` → `nini-tui` → `nini-core` → 不再有别的。
`nini-ai` 和 `nini-tools` 通过 trait 接到 `nini-core` 上，所以新增一个
provider 或工具只是改一个文件。

## 开发

### 构建与测试

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

测试套件里有针对 SSE 解析器的基于属性模糊测试（`proptest`）、会拉起编译
出来的二进制的端到端 CLI 测试、用 `ratatui::backend::TestBackend` 做
帧级快照断言的 TUI 集成套件，以及一个 `nini-conformance` crate——它**只**
装 Pi 对等的 oracle：关于 Pi 0.84.2 的、可执行的陈述，改 nini 就能让它们变红。

conformance 测试的规矩：fixture 必须来自 Pi 的真实输出，绝不来自 nini 自己。
一个拿 nini 和自己对照往返的测试抓不住线格式的 bug，因为那类 bug 是对称的
——`stop_reason` / `stopReason` 的大小写差异就是这样在 950 个绿灯里活下来的。

本次发布的每一个修复，都用"重新注入缺陷、确认对应测试变红"的方式验证过。

### 新增一个 provider

1. 新建 `crates/nini-ai/src/<provider>.rs`
2. 实现 `nini_core::Provider` trait（只有 `name()` 和 `stream()`）
3. 把 SSE 事件翻译成 `nini_core::StreamEvent`
4. 在 `crates/nini-cli/src/main.rs::build_provider` 里注册
5. 在 `crates/nini-ai/src/<provider>.rs` 里加测试

### 新增一个工具

1. 新建 `crates/nini-tools/src/<tool>.rs`
2. 实现 `nini_core::tool::Tool`（只有 `name()`、`spec()` 和 `execute()`）
3. 在 `crates/nini-cli/src/main.rs::build_tools` 里注册
4. 测试与实现写在一起

## 致谢

nini 是 clean-room 重写。参照规格提取自：

- **Mario Zechner** 的 **[Pi](https://github.com/earendil-works/pi)**——
  原始的 TypeScript 实现。nini 的 session 格式、斜杠命令面、settings schema
  和压缩算法，都遵循 Pi v0.85.1 有文档记录的行为。

- **Armin Ronacher** 和 **Rust 社区**——正是他们带来的生态（tokio、ratatui、
  reqwest、serde），让这样的项目在几周内可落地。

- Pi 的贡献者与早期用户，他们的反馈塑造了我们构建时对着读的那份规格。

如果 Pi 上游出了你急需的功能，请提
[issue](https://github.com/jinxumi-oss/nini/issues/new)——优先级列表对齐的是
上游被要求最多的功能。

## 许可

MIT——见 [LICENSE](LICENSE)。

## 参与贡献

欢迎贡献。提交信息风格、PR 模板和 `good first issue` 标签见
[CONTRIBUTING.md](CONTRIBUTING.md)。安全问题见 [SECURITY.md](SECURITY.md)。
