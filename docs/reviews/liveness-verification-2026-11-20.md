# Verify

- 复现转绿: `bash /tmp/verify/case3.sh a2b '{"liveness":{"no_progress_model_secs":60,"no_progress_tool_secs":30}}' 3 12 15 abort`（tmux + 假 provider 挂死）→ 退出码 0 → 编辑 settings.json 把 `no_progress_model_secs` 改成 3 后，新一轮 **3.12s** 出现 `⚠ no progress 3s · waiting for model` | PASS
- 回归: `cargo test --workspace --no-fail-fast` → 退出码 0（存活阈值改动所在快照：2327 passed / 0 failed）；`cargo fmt -- --check` → 退出码 0；`cargo clippy --workspace --all-targets -- -A clippy::pedantic -A clippy::nursery -A clippy::perf -A clippy::complexity -D warnings` → 退出码 0 | PASS（当前工作树被**另一个 agent 并发改动**后：`cargo test` 退出码 101，1 个失败 = `every_source_file_is_tracked_by_git`（`crates/nini-tui/src/selectors/theme.rs` 未入 git）；`cargo fmt --check` 退出码 1，差异全部落在 `runtime.rs` 主题选择器代码与新增的 `selectors/theme.rs`，与本次存活阈值改动无关）
- 断言有效: 在 `/tmp/nini-mut`（源码副本）把热生效写回短路为 `if false && lv != ...`（runtime.rs:4520）→ 同一复现命令 → 退出码 1，15 秒内**不出现** `⚠ no progress`（未改源码时 3.12s 出现）→ 复现场景对被测代码敏感，不是空跑 | PASS
- 端到端: tmux 真实 TUI 场景（隔离 HOME + `NINI_AGENT_DIR` + 只 accept 不回应的本地 TCP provider）→ 退出码 0 → 启动转录出现校验警告；改配置后新轮次 ~3s 报 `⚠ no progress`；`/status` 列出四个阈值与生效值 | PASS

---

## 验收结论

**A. 阈值热生效 — 通过。** 证据：
- 启动警告（转录通道）：隔离 HOME 下 `settings.json = {"liveness":{"no_progress_model_secs":60,"no_progress_tool_secs":30}}`，启动后转录第 4 行 `⚠ 等工具无进展超时为 30 秒：短于 bash 的默认超时 120 秒，正常的 cargo test / build 会被误报成卡住了`（`/tmp/verify/a2b.startup.txt`）。
- 热生效（关键）：同一会话把 `no_progress_model_secs` 改成 3 → Esc 结束上一轮 → 再发一条 → 轮询 tmux 屏，`[first 'no progress' after 3.122540482s]`，底部 `⚠ no progress 3s · waiting for model`（`/tmp/verify/a2b.msg2.txt`）。
- 启动即生效（对照组）：启动配置直接写 `no_progress_model_secs:3` → 挂死 provider 后 ~3s 起就有 `⚠ no progress`（`/tmp/verify/a1.msg1.txt`）。
- 断言有效性：把热生效写回短路掉的源码副本 → 同一场景 15s 内不出现 `⚠ no progress`（退出码 1）。

**B. 默认阈值不误报 — 通过。** 不写 `liveness` 段（`settings.json` 不存在）→ provider 挂死 12s → `[no 'no progress' within 12s]`（`/tmp/verify/b1.msg1.txt`）；启动配置 `no_progress_model_secs:60` 时同样 12s 内不报（`/tmp/verify/a2b.msg1.txt`）。

**C. `/status` 展示 — 通过。** `/status` 输出：`存活阈值: provider 响应头超时: 45s / provider 流空闲超时: 90s / 等模型无进展: 3s / 等工具无进展: 30s` + `⚠ 等工具无进展超时为 30 秒…`（`/tmp/verify/c1.status.txt`），四个阈值与生效值齐全。

**D. 回归 — 通过（附注见下）。**
- `cargo test --workspace --no-fail-fast` → 退出码 0，`passed: 2327  failed: 0`（`/tmp/verify/cargo-test.log`，对应"存活阈值这一轮"的快照）。
- `cargo fmt -- --check` → 退出码 0。
- `cargo clippy --workspace --all-targets -- -A clippy::pedantic -A clippy::nursery -A clippy::perf -A clippy::complexity -D warnings` → 退出码 0（`/tmp/verify/clippy2.log`，当前树也是 0）。
- **与本次无关的既有失败**：验收期间工作树被另一个 agent 并发改动（新增 `crates/nini-tui/src/selectors/theme.rs` 等，`git diff` 哈希从 `…` 变为 `42e028ef…`）。在改动后的树上重跑：`cargo test --workspace` 退出码 101，唯一失败 `every_source_file_is_tracked_by_git`（`crates/nini-core/tests/workspace_modules.rs:175`，抱怨新文件 `crates/nini-tui/src/selectors/theme.rs` 未入 git）；`cargo fmt --check` 退出码 1，差异全部在 `runtime.rs` 的主题选择器代码与新文件 `selectors/theme.rs`，**没有一条落在存活阈值相关代码**（`runtime.rs:4079/4518-4529`、`dispatch.rs:1570-1595`、`liveness.rs`、`stream_idle.rs`）。这两项不计为本次 FAIL。

**E. 找问题 — 发现 5 个（见下）。** 其中 1 个是真实缺陷（非法值 0 泄漏进渲染阈值 → 永久误报），1 个是断言重复刷屏，1 个是双通道重复，2 个是行为不一致/既有缺陷。

## 发现的问题

1. **[高] 非法阈值 0 泄漏进渲染阈值 → 永久误报「无进展」，且 `/status` 与实际生效值互相矛盾。**
   `crates/nini-tui/src/runtime.rs:4522-4523` 与 `crates/nini-tui/src/commands/dispatch.rs:1577-1578` 把**未清洗的** `lv`（文件原值）写进 `run_state`，而进程全局 `nini_core::liveness::set_liveness_settings` 存的是 `sanitized()` 后的值（0 → 默认）。渲染层（`render.rs:2172/2185`）读的是 `run_state`，于是 `no_progress_model_secs = 0` 让 `quiet >= 0` 恒真。
   复现：`settings.json={"liveness":{"no_progress_model_secs":0}}`，启动 → `/reload` → 发消息，底部**立刻**出现 `⚠ no progress 4s · waiting for model`（`/tmp/verify/e3`）；同时 `/status` 显示 `等模型无进展: 60s`（读的是清洗后的全局），与界面实际用的 0s 矛盾；`/reload` 自己的回报也写 `存活阈值: 模型 60s > 0s`。
   建议：写回 `run_state` 时用 `nini_core::liveness::sanitized(lv)`（或直接写 `nini_core::liveness::liveness_settings()` 清洗后的值），`/status` 与 `/reload` 的回报也统一用生效值。

2. **[中] 每次提交都重推同一条校验警告（转录刷屏）。**
   `runtime.rs:4520` 的比较是「文件原值 vs **清洗后**的全局」，只要文件里有 0 之类会被清洗掉的值，两者永远不相等 → 每轮都走写回分支 → 每轮都 `push_notice(warning)`。
   复现：`{"liveness":{"no_progress_model_secs":0}}`，连发 3 轮，转录里同一句警告出现次数 1 → 2 → 3 → 4（`/tmp/verify/e1b` 屏幕输出）。
   建议：比较时用 `sanitized(lv) != liveness_settings()`；或只在「新增了警告」时推。

3. **[中] 阈值校验警告在两个通道重复出现（stderr + 转录）。**
   `crates/nini-cli/src/main.rs:409-413` 无条件 `eprintln!("nini: {warning}")`，注释写明「TUI 模式由 nini-tui 把同一份警告推进转录」，但代码没区分模式；`crates/nini-tui/src/state.rs:2279-2282` 又把同一份警告推进转录。
   复现：同一份配置，TUI 启动时 stderr 文件里是 `nini: 等工具无进展超时为 30 秒…`，转录第 4 行是同一条（`/tmp/verify/a2b.stderr.log` + `a2b.startup.txt`）→ 用户看到同一条警告两遍。
   建议：TUI 模式下不再 `eprintln!`（由 nini-tui 负责），或把警告只留给 stderr。

4. **[低] 热生效只在「新开一轮」时生效，进行中的轮次里改配置不生效（且 steer 消息不算一轮）。**
   写回逻辑在 spawn 新 agent 的分支里（`runtime.rs:4518`），`submit_user_input_inner` 顶部虽然每轮都重读文件（`runtime.rs:4079-4101`），但轮次被 steer 进正在跑的回合时不会触发写回。
   复现：挂死 provider → 发消息（回合挂住）→ 改 `no_progress_model_secs` 为 3 → 再发一条 → 底部仍是 `⠋ waiting for model · 18s`，**没有** `⚠ no progress`（`/tmp/verify/hot2.msg2.txt`）；把回合 Esc 掉再发一条 → 3.12s 就报（`a2b`）。
   建议：把写回放到「提交时」而不是「新轮 spawn 时」，或至少在 steer 分支也应用。

5. **[低·既有] `/reload` 在生产路径上永远说「no settings file to reload」。**
   `crates/nini-tui/src/state.rs` 里 `model_state.settings_path` 在生产代码中**从未被赋值**（全仓 grep 无写入点，仅默认 `None`），所以 `/reload` 的主题/上下文窗口分支永远走不到（`dispatch.rs` 里 `state.model_state.settings_path.clone()` 恒为 `None`）；本次的 liveness 分支因为改用 `nini_core::settings::load_settings(cwd)` 才生效。证据：`/tmp/verify/e3` 屏上 `/reload` 输出 `reload: no settings file to reload; 存活阈值: 模型 60s > 0s; …`。不属于本轮引入，但会让用户以为 `/reload` 整体没生效。
   另：`/reload` 的输出里**不含**校验警告（只有 `/status` 含），与热生效路径「推警告到转录」不一致。

## 建议

1. 写回 `run_state` 一律用清洗后的值（`sanitized(lv)` 或 `liveness_settings()`），并让 `/status`、`/reload` 回报与渲染层读同一个值；这一条同时修掉问题 1、2 的一半。
2. 热生效的比较改成 `sanitized(lv) != liveness_settings()`，或只在警告集合**新增**时推通知，避免每轮刷同一条警告（问题 2）。
3. TUI 模式下不要再 `eprintln!` 存活阈值警告（`nini-cli/src/main.rs:411-413`），避免与转录重复（问题 3）。
4. 把热生效的写回从「spawn 新轮次」移到「提交时」（或在 steer 分支也应用），使进行中的回合也能吃到新阈值（问题 4）。
5. 给 `model_state.settings_path` 赋上真实路径，让 `/reload` 的其余分支（主题/上下文窗口/校验警告）真正可用（问题 5）。

## 附：关键命令与输出片段

环境：`export PATH="$HOME/.cargo/bin:$PATH"`；二进制 `target/debug/nini`；假 provider = 只 accept 不回应的 TCP 服务（`/tmp/verify/hang_provider.py`）；TUI 用 `tmux -L nini-verify` + 隔离 HOME + `NINI_AGENT_DIR=$HOME/.nini/agent` + `OPENAI_BASE_URL=http://127.0.0.1:<port>/v1`。

```
$ cd /tmp/verify && PORT=19884 bash case3.sh a2b \
    '{"liveness":{"no_progress_model_secs":60,"no_progress_tool_secs":30}}' 3 12 15 abort
### a2b startup:
  ⚠ 等工具无进展超时为 30 秒：短于 bash 的默认超时 120 秒，正常的 cargo test / build 会被误报成卡住了
### a2b stderr:
nini: 等工具无进展超时为 30 秒：短于 bash 的默认超时 120 秒，正常的 cargo test / build 会被误报成卡住了
  [no 'no progress' within 12s]              # 阈值 60：12s 内不误报
### after Esc:
 nini   idle  •  ctx   0% [#########]  (openai-compat) mock-tui-model  [dark]
### settings rewritten: no_progress_model_secs=3
  [first 'no progress' after 3.122540482s]     # 热生效：改完 ~3s 就报
⚠ no progress 3s · waiting for model   Esc  abort  Ctrl+C  abort
```

```
$ cd /tmp/verify && PORT=19875 bash case3.sh b1 NONE NONE 12 0
### b1 startup:
  (no threshold/warning line)
  [no 'no progress' within 12s]              # B：无 liveness 段 → 默认 60s，不误报
```

```
$ cd /tmp/verify && PORT=19877 bash case4.sh c1 '{"liveness":{"no_progress_model_secs":3,"no_progress_tool_secs":30,"provider_response_secs":45,"provider_stream_idle_secs":90}}'
存活阈值:  provider 响应头超时: 45s
provider 流空闲超时: 90s  等模型无进展: 3s  等工具无进展: 30s
⚠ 等工具无进展超时为 30 秒：短于 bash 的默认超时 120 秒，正常的 cargo test / build 会被误报成卡住了
```

```
# 断言有效性（源码副本 /tmp/nini-mut，短路热生效写回 runtime.rs:4520）
$ cd /tmp/verify && PORT=19885 bash case3m.sh mut '{"liveness":{"no_progress_model_secs":60,...}}' 3 12 15 abort
  [no 'no progress' within 12s]
### settings rewritten: no_progress_model_secs=3
  [no 'no progress' within 15s]                # 变红：断言确实咬住了被测代码
```

```
$ cd /home/jin/nini && cargo test --workspace --no-fail-fast   # 存活阈值快照
test result: ok … passed: 2327  failed: 0        EXIT=0
$ cargo fmt -- --check                            FMT_EXIT=0
$ cargo clippy --workspace --all-targets -- -A clippy::pedantic -A clippy::nursery -A clippy::perf -A clippy::complexity -D warnings
CLIPPY_EXIT=0
# 当前树（另一 agent 并发 WIP）：
test result: FAILED. 1 passed; 1 failed … every_source_file_is_tracked_by_git
  → crates/nini-tui/src/selectors/theme.rs 未入 git（与本次改动无关）
cargo fmt -- --check → FMT_EXIT=1（差异仅在 runtime.rs 主题选择器代码 + selectors/theme.rs）
```

```
# 问题 1/2 的复现（非法值 0）
$ cd /tmp/verify && PORT=19886 bash case5.sh e1b '{"liveness":{"no_progress_model_secs":0}}' zero
### startup 警告出现次数: 1
### 第 1 轮后：警告行数=2  底部: ⚠ no progress 3s · waiting for model
### 第 2 轮后：警告行数=3  底部: ⚠ no progress 4s · waiting for model
### 第 3 轮后：警告行数=4  底部: ⚠ no progress 3s · waiting for model
# 问题 1 的 /status 矛盾
$ PORT=19888 bash case7.sh e3 '{"liveness":{"no_progress_model_secs":0}}'
存活阈值:  provider 响应头超时: 60s … 等模型无进展: 60s      # /status 说 60s
### 运行中的底部:
⚠ no progress 4s · waiting for model                            # 界面实际用 0s
# 问题 4 的复现（回合进行中改配置）
$ PORT=19879 bash case.sh hot2 '{"liveness":{"no_progress_model_secs":60,...}}' 3
  (no no-progress line)   # 回合挂住时改配置 + steer 消息 → 不热生效
```

```
# 「没有 settings 文件就不抹阈值」的反证（问题清单里的担心点，未复现）
$ PORT=19883 bash case5.sh e2b '{"liveness":{"no_progress_model_secs":3,...}}' delfile
### settings 文件已删除
### 第 1 轮后：底部: ⚠ no progress 3s · waiting for model   # 阈值仍是 3，没被抹成默认
```

```
# 并行污染抽查（10 轮 × 3 个 crate）
$ for i in $(seq 1 10); do cargo test -p nini-tui --lib; done   # 10/10 EXIT=0
$ for i in $(seq 1 10); do cargo test -p nini-core --lib; done   # 10/10 EXIT=0
$ for i in $(seq 1 10); do cargo test -p nini-ai; done            # 10/10 EXIT=0
```
