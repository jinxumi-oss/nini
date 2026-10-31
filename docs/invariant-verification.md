# 链③ 标注的变异验证留痕

> 为什么需要这个文件：**代码无法自动验证"这个断言真的守了那条不变量"。**
>
> 2026-10-19 的发布终审实测过这条捷径：在任意测试文件里插一行
> `//! invariant: <sha>`，`invariants.py` 的 NONE 立刻从 1 变 0、
> `scripts/gate.sh` 照样退出 0。随后加的形式校验（必须在测试文件、必须含
> `#[test]`）**同样被绕过**——"这个文件里的断言是否与这条不变量有关"
> 不是能自动判断的问题。
>
> 所以 `//! invariant: <sha>` 是**人工担保**，不是机器判定。本文件是那份担保的
> 证据：每条声明要写下**注入了哪个 bug、哪个断言红了**。
>
> `scripts/gate.sh` 会交叉检查：每个被声明的 sha 必须在下面出现。
> 写一行注释骗不过这一条；而编造的记录会在独立审计里露馅。

---

## 独立复验（审计者逐条注入-观察-还原，非执行者自述）

| sha | 声明于 | 注入的 bug | 变红的断言 |
|---|---|---|---|
| `c3944d13` `7a71f5b3` `9c9783e3` | `pi_wire_oracle` | `input_json` 不再 seed 首块 `arguments` | `the_first_deltas_lone_open_brace_survives_into_the_final_json`（症状 `input_json: Null`；其余 4 条仍绿） |
| `3bf4815d` | `fallback_diagnostics` | retryable 分支不再记录失败原因 | `an_exhausted_chain_names_every_key_it_tried_and_why`（其余 3 条仍绿） |
| `b4d74570` | `refusal_text` | refusal 理由塞回大段多余空格 | `no_refusal_reason_carries_a_run_of_spaces`（其余 2 条仍绿） |
| `3a31fe9e` | `prompt_template_utf8` | 退回按字节切片 `&first[..60]` | 3 条全红（该不变量的语义即"任何多字节输入都不得 panic"） |
| `545fd548` | `tool_disambiguation` | `find` 描述退回与 `bash` 混淆的旧文案 | 2 条红；`find_guideline_states_the_negative_direction` 仍绿（守的是别的性质） |
| `d4070d0b` | `search_bar_layout_row` | 布局不再为搜索栏预留行 | 3 条全红 |
| `ffb3d965` | `scroll_affordance` | 跳到底部却不恢复 autoscroll | `shift_end_snaps_back_to_the_live_tail`；其余 5 条仍绿（underflow / 冷启动显示尾部 / 隐藏量提示 / 指示器消失） |
| `6eb77154` | `info_skills_count` | 计数改用与进程无关的常数 | 3 条全红 |
| `3df653cc` | `export_concurrent_writers` | 导出文件名去掉 pid+tid | `concurrent_exporters_get_distinct_files_holding_their_own_transcript` |
| `9d954ef0` | `image_temp_concurrent_writers` | 临时文件序号恒为 0 | `concurrent_pastes_get_distinct_files_holding_their_own_bytes` |
| `be783e9a` | `footer_home_portability` | tilde 替换写死 `/home/jin` | 2 条红 |
| `632a3617` | `env_lock_scope` | `settings.rs` 第 4 处去掉加锁 | `every_env_mutating_test_in_the_file_takes_the_lock` |
| `71c0cc19` | `installer_source` | `install.sh` 的 `REPO` 退回旧 slug `jinxumi/nini` | `the_installer_points_at_the_repository_the_releases_live_in` + `the_documented_one_liner_fetches_the_same_repository` 变红（第 3 条仍绿：资产命名未变） |

## 终审复验（2026-10-21 · 在干净克隆里重新注入，逐条还原）

发布终审在 `git clone` 出的干净副本里把上表 12 行**全部重新注入了一遍**，
并补上了当时未复验的 `8744ceb5` 三条子不变量。结论：**15 条声明全部有据**，
且每一行的"变红的断言"与上表记载一致（含条数）。

| sha | 注入的 bug | 复验结果 |
|---|---|---|
| `8744ceb5` · `welcome_card` | 欢迎卡不再要求输入框为空 | `the_card_is_gone_once_the_user_types_even_without_sending` 变红（其余 2 条绿） |
| `8744ceb5` · `stop_reason_notice` | 停止理由被小写化 | `stop_reason_reaches_the_transcript_as_a_stop_notice` + `the_stop_notice_is_visible_in_the_frame` 变红 |
| `8744ceb5` · `stop_reason_producer` | `TurnEnd` 臂不再产生 `length` 的 `StopReason` | 3 条全红（`nini_and_pi_agree_on_which_stop_reasons_get_a_notice` 等） |
| `8744ceb5` · `narrow_hints` | 窄屏阈值 70 → 50 | `no_narrow_width_between_50_and_69_truncates_the_hint_set` 变红（其余 2 条绿） |
| `c3944d13` `7a71f5b3` `9c9783e3` | 首块 `arguments` 不再 seed 进 `input_json` | `the_first_deltas_lone_open_brace_survives_into_the_final_json` 变红，症状 `input_json: Null`（其余 4 条仍绿） |
| `3bf4815d` | retryable 分支不再记录失败原因 | `an_exhausted_chain_names_every_key_it_tried_and_why` 变红（其余 3 条仍绿） |
| `b4d74570` | refusal 理由塞回一段多余空格 | `no_refusal_reason_carries_a_run_of_spaces` 变红（其余 2 条仍绿） |
| `3a31fe9e` | 退回按字节切片 `&first[..60]` | 3 条全红 |
| `545fd548` | `find` 描述退回与 `bash` 混淆的旧文案 | 2 条红；`find_guideline_states_the_negative_direction` 仍绿（守的是别的性质） |
| `d4070d0b` | 布局不再为搜索栏预留行 | 3 条全红 |
| `ffb3d965` | 跳到底部却不恢复 autoscroll | `shift_end_snaps_back_to_the_live_tail` 变红（其余 5 条仍绿） |
| `6eb77154` | 技能计数改用与进程无关的常数 | 3 条全红 |
| `3df653cc` | 导出文件名去掉 pid+tid | `concurrent_exporters_get_distinct_files_holding_their_own_transcript` 变红 |
| `9d954ef0` | 临时文件序号恒为 0 | `concurrent_pastes_get_distinct_files_holding_their_own_bytes` 变红 |
| `be783e9a` | tilde 替换写死 `/home/jin` | 2 条红 |
| `632a3617` | `settings.rs` 第 4 处去掉加锁 | `every_env_mutating_test_in_the_file_takes_the_lock` 变红 |

复验方法：每条变异在 `/tmp` 的干净克隆里改一处源码 → 跑该条断言所在的
测试文件 → 记录变红的断言 → `git checkout` 还原。共 16 次注入，全部命中，
无一条"注入了但测试仍全绿"。

---

## 执行者自复验（2026-10-22 · 两条新不变量的注入-观察-还原）

执行者自述，按上表同一纪律做的注入：改一处源码 → 跑该条断言所在的测试文件
→ 记录变红的断言 → `git checkout` 还原。**这不是独立复验**——独立复验那一节
留给审计者；这里只证明这两条声明不是空写的（写一行注释骗不过门禁，编造的
记录会在审计里露馅，所以连"我注入了什么"都写下来）。

| sha | 声明于 | 注入的 bug | 变红的断言 |
|---|---|---|---|
| `ca97731b` | `terminal_guard` | `terminal_guard` 不再把 fd 2 指向管道（去掉 `dup2`） | `a_library_write_to_stderr_becomes_a_transcript_notice` + `the_real_stderr_works_again_after_the_guard_is_dropped` 变红（症状：transcript 里一条通知都没有） |
| `ca97731b` | `no_terminal_writes` | 在 `nini-tui/src/runtime.rs` 里加回一处 `eprintln!` | `no_source_file_writes_to_the_terminal_outside_the_whitelist` 变红（报出文件与行号） |
| `a0d18027` | `project_trust`（src 内单元测试） | 去掉 `ask_level_warning` 的 per-cwd 去重（每次调用都返回 `Some`） | `the_ask_warning_is_stated_once_per_cwd` 变红（症状："the second call for the same cwd must not repeat the warning"） |
| `a87797f1` | `terminal_guard` / `notice_rendering` / `slash_command_e2e` | 本轮四条修复各自被注入回退 | 顺序：`the_lines_at_the_exit_come_out_in_the_order_they_were_written` 变红；宽度：`a_notice_with_ambiguous_characters_fits_a_cjk_terminal` 变红；fallback：`changing_the_model_does_not_hide_the_provider_fallback` 变红；`check()` 恢复公开 → `cargo build` 失败 |
| `851cf4e` | `terminal_guard` | 退出时把队列的 flush 挪到 reader 停止之后 | `the_lines_at_the_exit_come_out_in_the_order_they_were_written` 变红（`late1` 印在 `early1` 之前：诊断的尾巴跑到了头前面） |
| `851cf4e` | `terminal_guard` | reader 在早退之前也写一次「N lines were dropped」 | `the_dropped_warning_appears_exactly_once` 变红（同一句警告出现两次） |
| `655db2d` | `notice_rendering` | 通知的折行改回 western 口径（不预留歧义宽字符的列） | `a_notice_with_ambiguous_characters_fits_a_cjk_terminal` 变红（`→…✓•·` 在 CJK 终端里多占 12 列，尾部被裁） |
| `83a6871` | `slash_command_e2e` | `set_model` 不再理会 provider fallback 的原因 | `changing_the_model_does_not_hide_the_provider_fallback` 变红（`/model` 之后状态栏又变回正常样子） |
| `81fac45` | `project_trust` | `check()` 恢复成公开的「判断并打印」API | `cargo build` 即失败（`check` 只留给测试，生产路径必须用 `check_and_warn`） |
| `851cf4e` | `terminal_guard` | 队列在退出时被丢弃（drop 不再 flush pending） | `a_line_still_in_the_queue_reaches_the_real_stderr_at_exit` 变红（那一行既不在 transcript 也不在滚动回放里） |
| `851cf4e` | `terminal_guard` | 读线程也写一次「N lines were dropped」 | `the_dropped_warning_appears_exactly_once` 变红（同一句警告出现两次） |
| `3702444` | `html_export` | 导出时不再转义工具参数/命令行/stop reason | `html_escapes_every_arm_that_carries_external_text` 变红（导出文件里出现活的 `<script>`） |
| `81fac45` | `trust_gate_lock` | 把警告打印挪回锁内 | `a_blocked_trust_warning_does_not_hold_the_store_lock` 变红（打印被卡住时，信任库的锁仍被占着） |
| `655db2d` | `notice_rendering` | 通知的折行预算改回 4 列 | `a_notice_row_is_never_wider_than_the_screen` 变红（那一行 61 列，屏幕只有 60 列，尾部被裁掉） |
| `83a6871` | `nini-cli` 单元测试 | provider 构造失败时不再保留错误 | `a_provider_that_cannot_be_started_is_reported` 变红（错误被吞掉，状态栏还在说 openai-compat） |
| `91f31b94` | `nini-tools/src/bash.rs`（src 内单元测试） | 把宽限期放回 5000 ms（v0.8.9 之前的行为） | `timeout_does_not_sit_out_the_full_grace_period` 变红（实测 `timeout took 6.22s`） |
| `54758b32` | `frame_fits_cjk_terminal` | 让策略失效（终端的回答被忽略，`ambiguous_width_is_wide()` 恒为 false） | `the_frame_fits_a_terminal_that_doubles_ambiguous_characters` 变红（症状：`row 2 is 85 columns wide in a CJK terminal (the terminal has 60)`） |
| `7ca58577` | `sse_utf8_split` | 解析器回到修复前的样子：每个 chunk 单独 `from_utf8` | `a_character_split_across_two_chunks_is_not_an_error`、`a_character_split_across_three_chunks_is_not_an_error`、`a_long_chinese_answer_survives_any_chunking` 三条同时变红（症状：`invalid utf-8: incomplete utf-8 byte sequence from index 0`） |
| `bdfdc06d` | `terminal_guard` | 读线程把通知丢在地上（不进队列） | 4 条守卫断言同时变红：`a_library_write_to_stderr_becomes_a_transcript_notice`、`a_notice_is_never_lost_when_the_state_is_locked`、`the_guard_comes_back_after_the_external_editor`、`the_real_stderr_works_again_after_the_guard_is_dropped` |
| `cc2e35ff` | `notice_rendering` | `/export` 的 Notice 臂去掉 `html_escape` | `a_notice_in_the_export_is_escaped` 变红（`<script>` 原样进导出 HTML） |
| `cc2e35ff` | `notice_rendering` | Notice 臂不折行（`wrap_text_to_width` 换成 `None`） | `a_long_notice_is_wrapped_not_clipped` 变红（长诊断的尾部不在屏上） |
| `cc2e35ff` | `dispatch`（src 内单元测试） | `/copy` fallback 改回「copied N bytes to the transcript」 | `the_copy_fallback_message_says_only_what_is_true` 变红 |
| `72dafe4a` | `terminal_guard` | 退回修复前的退出方式：**不发停止信号** + **无限期 join** 读者线程 | `dropping_the_guard_does_not_wait_for_a_child_that_inherited_the_pipe` 变红（实测卡满 30 秒才失败）——只去掉其中任一机制都不会红，两者合起来才是那个 bug |
| `fe56543d` | `scroll_indicator_no_overlap` | 去掉 `total_items > full_height` 闸（陈旧 `scroll_offset` 又开始凭空造提示） | `a_stale_scroll_offset_does_not_invent_a_hint` 变红 |
| `fe56543d` | `dispatch`（src 内单元测试） | `truncate_for_notice` 不再截断（整条回复又进 transcript） | `a_copy_fallback_notice_is_truncated` 变红 |
| `a9b490df` | `scroll_indicator_no_overlap` | 渲染层退回修复前的写法：列表画满整个 transcript 区，两个指示器画在正文的第一行/最后一行上 | `the_overflow_indicator_does_not_eat_the_first_row_of_the_transcript` + `the_user_scroll_indicator_does_not_eat_the_last_row_of_the_transcript` 变红（其余断言仍绿） |

二十四条注入全部命中，无一条"注入了但测试仍全绿"。

---

## 异步落盘的等待（A14，2026-10-30）

| sha | 声明于 | 注入的 bug | 变红的断言 |
|---|---|---|---|
| `af77d45a` | `r2f_bash_session` | `wait_for_session_file` 不再等待（直接 return），即恢复成「读盘时文件还没写出来」 | `a_shell_command_the_user_ran_survives_a_reload` + `it_is_recorded_as_a_bash_execution_message` 2 条全红（`must load: Other("io error: No such file or directory (os error 2)")`） |

**注入-观察-还原的实测输出**（2026-10-30 重跑逐字粘贴）：

```
$ cargo test -p nini-tui --test r2f_bash_session
test it_is_recorded_as_a_bash_execution_message ... FAILED
test a_shell_command_the_user_ran_survives_a_reload ... FAILED
thread 'it_is_recorded_as_a_bash_execution_message' panicked at crates/nini-tui/tests/r2f_bash_session.rs:106:47:
thread 'a_shell_command_the_user_ran_survives_a_reload' panicked at crates/nini-tui/tests/r2f_bash_session.rs:70:38:
must load: Other("io error: No such file or directory (os error 2)")
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out

$ # 还原后
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

> 上面的输出是**逐字粘贴**的，所以它带行号。但**判定依据是失败机制，
> 不是行号**。第一次写下的是 `:68:38`，而那个行号在任何版本里都不存在
> （注入时是 `:68`，HEAD 上是 `:70`，下一次代码编辑就会再变一次）——
> 一条写着假行号的「实测记录」比没有记录更坏：它看起来可核对，实际不可核对。
> 两条测试的**失败机制**才是要留痕的东西：
>   * `a_shell_command_the_user_ran_survives_a_reload` —— `session_load` 拿到
>     `io error: No such file or directory (os error 2)`（文件还没落盘就去读）；
>   * `it_is_recorded_as_a_bash_execution_message` —— `read_to_string(&path).unwrap()`
>     读不到那个还不存在的文件（同一个原因，第二个断言）。

**为什么这条当初没被门禁拦住**：`af77d45` 的 subject 以 `fix` 开头，但它的改动是
「把一条**已有**测试的等待方式从固定 sleep 改成轮询」——提交里没有**新增** `#[test]`，
于是 `scripts/invariants.py:312` 的 `self_test` 判定为 false，`tier` 落到 `NONE`，
`invariants_none` 由 1 变 2，`scripts/gate.sh` 的「裸奔不变量 2 > 基线 1」变红。

**而 `scripts/gate.sh` 从来没有进过 CI**（`.github/workflows/ci.yml` 里没有它的调用），
所以这条欠账一路发布到了 v0.14.0 的 tag。这是本轮堵上的流程漏洞：
CI 现在会跑「裸奔不变量」检查，本地门禁红不再能溜过发布。

---

## 已知的验证局限（如实记录）

1. **两次"测试全绿"是变异打偏，不是测试无效**：一处注入了 `app.rs` 的辅助函数
   而 `nini info` 走的是 `info.rs`；一处改了 `tests/theme_wiring.rs` 而该断言的
   对象是 `src/settings.rs`。两次都靠重新定位注入点才拿到有效变异。
2. **同模型的审计有同源风险**：执行者与审计者用的是同一个模型
   （MiniMax-M3.1-Flash-Preview）。独立性来自独立上下文 + 强制自复算，
   不来自不同意见。本项目栽过的坑（`stop_reason` 大小写、footer 3 份失真副本）
   正是同源测试抓不到的。
3. **上表只覆盖"该不变量被重新破坏时能否被抓到"**，不覆盖"该不变量的语义本身
   是否正确"。后者需要对照物（真 Pi 抓包 / spec），不属于本文件的范围。
4. **2026-10-21 补记**：终审复验把 `8744ceb5` 的三条子不变量补齐了
   （`welcome_card` / `stop_reason_notice` / `stop_reason_producer`），所以
   本文件此前唯一的"未复验"缺口已经关闭——15 条声明现在都有可复现的注入记录。
   局限不变：复验者与执行者仍是同一个模型，独立性来自独立上下文与强制自复算。


---

## 工具对标（T1–T4，2026-10-23）

四条新判据，每条都做过**变异验证**：注入一个 bug → 断言变红 → 还原。

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-tools/tests/tool_contract_parity.rs` | 把 `write` 的 description 改成旧措辞（"Write a file. …"） | `工具 write 的 description 与 Pi 0.85.1 不一致` |
| `crates/nini-core/tests/prompt_tool_linkage.rs` | 把 prompt 的条件 guideline 改成 `if false && …` | `the_bash_guideline_appears_only_when_no_better_tool_exists` |
| `crates/nini-tools/tests/truncation_parity.rs` | 把 `DEFAULT_MAX_BYTES` 改回 1 MiB | `字节上限与 Pi 不一致` |
| `crates/nini-tools/tests/truncation_parity.rs` | 把 `truncate_tail` 的「保留尾部」改成保留头部 | `tail_truncation_matches_pi_case_by_case`（样本 `many-lines` / `tail-byte-limit`） |

**期望值的来源不是手算**：`scripts/pi-tool-dump.mjs` 执行 Pi 0.85.1 自己的
`createToolDefinition`，`scripts/truncate-oracle.mjs` 执行 Pi 自己的 `truncate.js`，
结果冻进 `references/pi-0.85.1/{tool-contracts,truncate-oracle}.json`。
这补上了差距 C1 的一部分：**行为层第一次有了「Pi 侧可执行对照物」**。


---

## 会话落盘（A5，2026-10-23）

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-cli/tests/session_at_startup.rs` | 让 `AppState::start_session` **不再建立会话**（`match None::<PathBuf>`） | 三条全红：`starting_up_creates_a_session_path_under_the_project`、`two_sessions_started_in_the_same_second_do_not_collide`、`the_conversation_actually_reaches_the_disk` |

**这一轮抓到的最重要的一条，不是那个 bug，是判据本身**：

第一版的测试**自己重写了一遍** `start_session` 的逻辑（因为 CLI 里那个函数是
`pub(crate)`，集成测试看不到它）。于是把产品代码里的「不建会话」注入进去之后，
**测试照样全绿**——它根本没碰到被测对象。

修法：把 `start_session` 挪进 **nini-tui 库**（`AppState::start_session`），
CLI 与测试共用**同一份实现**。改完再注入 → 三条全红。

**教训**：「测试全绿」不等于「被测行为被覆盖」。判据必须**打到产品代码**；
如果它不得不自己复刻一份逻辑，那它测的是**复刻品**，不是产品。


---

## 排队消息的自动投递（A6，2026-10-23）

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-tui/tests/queued_delivery.rs` | 回合收尾时**不再**置 `auto_turn_pending`（也就是「没人再捞这条消息了」） | `a_message_queued_after_the_turn_ended_is_delivered_without_another_enter` |
| `crates/nini-tui/tests/queued_delivery.rs` | 投递时把已在 transcript 的消息**再显示一遍** | `a_delivered_message_is_not_shown_twice` |


---

## `/export` 认路径（B1b，2026-10-23）

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-tui/tests/export_path.rs` | 让 `/export` **又变成忽略路径**（`let requested = "";`） | 三条全红：`an_html_path_is_honoured`、`a_jsonl_path_is_honoured_and_holds_the_session`、`an_unsupported_extension_is_refused_out_loud` |


---

## 压缩的生命周期（A2，2026-10-23）

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-core/tests/compaction_lifecycle.rs` | agent **不发** Compacting 相位 | `an_overflow_compaction_has_a_phase_and_ends_it` |
| `crates/nini-core/tests/compaction_lifecycle.rs` | 压缩**失败**（钩子返回 None）时**不收尾** | `a_failed_compaction_also_ends_the_lifecycle` |
| `crates/nini-tui/tests/compaction_queue.rs` | sink **不**把相位映射成 `is_compacting` | `a_submit_during_compaction_is_queued_not_spawned` |
| `crates/nini-tui/tests/compaction_queue.rs` | 运行结束（Done）**不复位**压缩标志 | `the_end_of_a_run_resets_the_compaction_flag` |


---

## Esc 的上下文语义（A3，2026-10-23）

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-tui/tests/escape_target.rs` | 相位**不再**映射成 `EscapeTarget` | `the_phase_decides_the_escape_target` |
| `crates/nini-tui/tests/escape_target.rs` | Compacting 上下文的 Esc 改成「清空输入」 | `esc_in_the_compacting_context_does_not_clear_the_input` |


---

## 引用必须能在快照里解析（23c4340b，2026-10-26）

四条引用**解析不到任何东西**：两条把 `queueCompactionMessage` 挂在
`agent-session.ts` 上（它住在 `interactive-mode.ts`），两条引用了 0.85.1 里
根本不存在的 `handleSlashCommand`（slash 命令的分发在
`setupEditorSubmitHandler` 里）。解析不到的引用比漂掉的行号更糟——它**看着
像证据**。

判据是 `crates/nini-conformance/tests/citations_resolve.rs`：用 Rust 把
「符号引用必须在钉住的快照里解析」这条规则**再写一遍**（Python 那边是
`scripts/pi-oracle-lock.py --verify-refs`，它同时还查哈希）。

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-conformance/tests/citations_resolve.rs` | 把 `queueCompactionMessage` 的引用改回 `agent-session.ts` | `every_citation_resolves_inside_the_pinned_snapshot`（`scripts/pi-oracle-lock.py --verify-refs` 同时报 `DRIFT`，resolved 125 / drift 1） |
| `crates/nini-conformance/tests/citations_resolve.rs` | 把 `setupEditorSubmitHandler` 改回 `handleSlashCommand` | 同上（报 `interactive-mode.ts#handleSlashCommand — the symbol is not in that document`） |

两条注入都实测过：断言变红，Python 门禁同时变红，然后逐字还原。

顺带把**对照物**补进了快照：Pi 0.85.1 的 `pi-tui/dist/fuzzy.js`（B6 的
评分真相）与 `dist/modes/interactive/model-search.js`（`/model` 的搜索
文本）此前**不在仓库里**，所以 `fuzzy.js#fuzzyFilter` 这类引用**无法被
机器复核**——不能被复核的引用不是引用。六个文件（dist / map / 还原出的
.ts）已按既有布局进 `references/pi-0.85.1/` 并记入 `SNAPSHOT.lock`
（419 文件），`scripts/pi-line-refs.py` 的文档名列表也扩了这两个，于是
引用总数从 111 变成 **126，drift=0，missing=0**。


---

## 声明的模块必须存在，源码必须被 git 跟踪（b479ef5d，2026-10-26）

2026-09-21 的一次 `git reset` 把几个模块从 git 里弄丢了，而跟踪着的代码
仍然声明着它们（`lib.rs` 写着 `mod entries;`，而 `entries.rs` 既没被跟踪
也不存在）——工作区编译不了，修复是补回一堆最小 stub。

判据是 `crates/nini-core/tests/workspace_modules.rs`：从**外面**看这棵树，
问 git 它认为自己跟踪了什么。

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-core/tests/workspace_modules.rs` | 在 `crates/nini-core/src/` 放一个**没被 git 跟踪**的 `.rs` | `every_source_file_is_tracked_by_git`（报出 `crates/nini-core/src/mutation_probe.rs`）；注入后已删除 |
| `crates/nini-core/tests/workspace_modules.rs` | 把 `lib.rs` 里的 `pub mod agent_hooks;` 改成 `pub mod agent_hooks_typo;` | **不是这条测试变红，而是工作区编译不了**——悬空声明本来就是编译错误。这条断言是编译器的检查用 nini 的话再说一遍：它报出「声明了却没有文件」，而不是一个 rustc 错误码。已还原 |

实测记录：第一条注入让断言变红（文件已删除、lib.rs 已还原）；第二条注入
让 `cargo test` 在编译阶段失败——这正是 2026-09-21 那次故障的形态。


---

## `/hotkeys` 必须真的列出键位表（02333721，2026-10-27）

2026-09-28 的修复把 v0.8 重构时塌成 stub 的 `/hotkeys` 恢复了回来（它本来
只说「按 F1」）。这条不变量此前一直靠**词法巧合**被认成「有保护」——
本轮新增的两个测试文件让那个词元的文档频率越过了阈值，扫描器就不再
认它了。词法猜测是猜测；这里是断言。

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-tui/tests/slash_command_e2e.rs` | `/hotkeys` 退回 stub（只说「按 F1」） | `hotkeys_labels_every_binding_it_lists`（该文件的 `slash_hotkeys_via_submit_user_input` 同组断言） |

注入已实测：断言变红，`dispatch.rs` 已逐字还原。


---

## Esc 真的要中止这一轮，压缩期只取消压缩（4a96da2，2026-10-27）

两个不变量在同一次修复里：**A7**（中止信号有消费者）与 **A2b**
（压缩期的 Esc 只取消压缩）。判据是
`crates/nini-tui/tests/compaction_escape.rs`（9 条）与
`crates/nini-core/src/abort_signal.rs` 的 3 条单元测试。

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-tui/tests/compaction_escape.rs` | 压缩上下文的 Esc 改回「中止整轮」 | `esc_during_compaction_cancels_the_compaction_not_the_turn` + `the_key_handler_triggers_only_the_compaction_signal_while_compacting` |
| `crates/nini-tui/tests/compaction_escape.rs` | 压缩期的 Esc 误触发**整轮**信号 | `the_key_handler_triggers_only_the_compaction_signal_while_compacting` |
| `crates/nini-core/src/abort_signal.rs` | `trigger()` 不落标志（只通知） | `the_key_handler_triggers_the_turn_abort_signal` + `the_key_handler_triggers_only_the_compaction_signal_while_compacting` + `the_two_abort_signals_are_independent` |

三条注入都实测过，断言变红，源码已逐字还原。

---

## 键位文件按文档的格式必须真的生效（d8f6057，2026-10-27）

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-tui/src/keybindings_manager.rs` | 把运行时的读取器换回**旧的**解析器（方向相反的那个） | `the_runtime_reader_understands_the_documented_format` |

注入已实测：断言变红（12 条仍绿），源码已逐字还原。


---

## 自动投递不能吞掉它刚取出的消息（v0.11 · A4 审查，2026-10-27）

A4 的审查发现一条**老路被本轮放大**的缺陷：`submit_queued_turn` 从队列里
`mem::take` 出**一条**消息、把它当 `explicit_text` 交给
`submit_user_input_inner`，而那个函数的**第一件事**是检查 `agent_alive`，
撞上就 `return`——`explicit_text` 被**无视**，消息**既不在队列里、也没被
提交、也没有任何报错**：它消失了。

触发路径真实存在：`Done` 把 `auto_turn_pending` 置真、运行循环随即取消息；
而同一时刻用户按 Enter 会起**新一轮**（`agent_alive = true`），`select!` 在
事件与按键之间随机挑一个。A4 把 followUp 接上真实生产者后，这条路被放大。

判据是 `crates/nini-tui/tests/auto_delivery_no_loss.rs`（3 条）。

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-tui/tests/auto_delivery_no_loss.rs` | `agent_alive` 分支改回「只读输入框、无视 explicit_text」 | `a_message_taken_for_auto_delivery_is_not_lost` |
| `crates/nini-tui/tests/auto_delivery_no_loss.rs` | `submit_queued_turn` 提前写 `mode = Editing` | `a_refused_auto_delivery_does_not_pretend_the_agent_is_idle` |
| `crates/nini-tui/tests/auto_delivery_no_loss.rs` | 自动投递重新解释 `!cmd`（`is_user_submit` 恒为 true） | `a_queued_bang_command_is_not_executed_when_delivered` |

三条注入都实测过，断言变红，源码已逐字还原。

---

## `/reload` 之后按键解析必须真的变（v0.11 · B3 修复，2026-10-27）

A4 的审查还挖出**我自己上一轮引入**的缺陷：v0.11 (B3) 把用户键位覆盖做成
「可重载的缓存」，却写成了**两份**——`resolve_with_user_overrides()` 读
缓存 A，`reload_user_overrides()` 写缓存 B。于是 `/reload` 说的话
（「N user override(s) in effect」）是**关于它并没有做到的事**，而键位从头
到尾没有变过。

判据是 `crates/nini-tui/tests/keybinding_reload_effect.rs`（4 条）。

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `crates/nini-tui/tests/keybinding_reload_effect.rs` | 键位缓存改回**两份**（reload 写 A、resolve 读 B） | 3 条红（`a_reload_changes_what_the_key_resolves_to` / `the_reported_count_matches_the_overrides_in_effect` / `without_a_reload_the_resolution_does_not_change`） |

注入已实测：断言变红，源码已逐字还原。

顺带修掉同一处的一个**假承诺**：`keybindings_manager.rs` 的模块文档当时
写着「用户照抄一份 Pi 的 `keybindings.json` 到 nini 也不该一行都不生效」，
而**Pi 的用户文件是扁平的**（`references/pi-0.85.1/src/ui/keybindings.ts`
的 `toKeybindingsConfig`），nini 却只认 `{"bindings": {...}}` —— 照抄过来
serde 直接解析失败，覆盖数为 0。现在**两种形状都收**（Pi 的扁平形状由
`pis_flat_keybindings_file_works_too` 钉住）。

---

## v0.11.3 那三条「改脚本/改 CI」的修复此前是裸奔的（2026-10-28 · v0.12 补账）

`scripts/invariants.py` 的测试语料只有 Rust（`crates/*/tests/*.rs` + src 里的
`#[cfg(test)]` 模块），所以 v0.11.3 那三条**改 shell 脚本与 CI 配置**的修复
被判成 NONE——裸奔数从 1 涨到 4，门禁因此变红。

它们的形状是确定的，可以被断言。判据是
`crates/nini-core/tests/tooling_invariants.rs`（7 条）。

| sha | 声明于 | 注入的 bug | 变红的断言 |
|---|---|---|---|
| `5195b587` | `tooling_invariants` | 把提示改回「run the bash tool then say TOOL_RESULT_SEEN」（即用户自己输入的那行就满足 `wait_for`） | `the_recording_prompt_never_contains_the_awaited_phrase` 变红（其余 6 条仍绿） |
| `dcd75b80` | `tooling_invariants` | 去掉 `PI_OFFLINE=1`（Pi 于是会在冷 runner 上下载 fd/ripgrep 并吃掉第一条提示） | `the_first_prompt_is_retried_until_the_model_is_actually_asked` 变红（其余 6 条仍绿） |
| `dcd75b80` | `tooling_invariants` | 把第一轮改回「贴一次就完事」（不重试，直到模型真的被问到） | 同上变红（这一条是上一条之外的**第二个**触发面：即使 PI_OFFLINE 在，提交早了照样被吃掉） |
| `dcd75b80` | `tooling_invariants` | 把那个**反着的**等待加回去（等 `Startup is still in progress` 消失） | 同上变红（见下：2026-10-28 实测，那个等待的第一轮就 break——它等的是「提交早了」才会出现的字符串） |
| `bca7e070` | `tooling_invariants` | 让近似版又写回快照路径（`out_path = SNAPSHOT_DIR / "tool-contracts.json"`） | `the_snapshot_tool_contracts_have_exactly_one_producer` 变红（其余 6 条仍绿） |
| `bca7e070` | `tooling_invariants` | CI 改装别的 Pi 版本（`pi-coding-agent@0.84.2`） | `ci_installs_the_frozen_pi_and_passes_its_root` 与 `ci_installs_the_version_the_target_freezes` 两条同时变红（其余 5 条仍绿） |

五条注入都实测过：断言变红，源码已逐字还原（`git diff` 为空）。

**为什么断言是「形状」而不是「行为」**：这三条修的是脚本与工作流，它们没有
Rust 的调用入口；能守住它们的只有「脚本必须长成什么样」。这比词法启发式
（`invariants.py` 的 NONE 判定）**更强**：注入的正是当初那个缺陷本身。

---

## 长任务不许在半路被截断（v0.12 · A8 修复，2026-10-28）

判据是 `crates/nini-conformance/tests/long_task_delivery.rs`（3 条）。

| 判据 | 注入的 bug | 变红的断言 |
|---|---|---|
| `long_task_delivery.rs` | 把 `MAX_TOOL_ITERATIONS` 改回 50（修复前的默认值） | `a_sixty_step_task_is_delivered` 变红（provider 只被叫 50 次，交付文本没到；其余 2 条仍绿） |
| `long_task_delivery.rs` | 把 `DEFAULT_MAX_FOLLOW_UP_ROUNDS` 改回 16，并去掉「0 表示不限」的判定 | `a_zero_follow_up_ceiling_means_unlimited` 变红（其余 2 条仍绿） |

两条注入都实测过，断言变红，源码已逐字还原。

**为什么这条不能只靠「默认不限」来保证**：默认值是可以被改回去的，而且改
回去的时候**没有测试会红**——除非有一条断言直接盯着「默认不限」这件事。
所以这条不变量守的是**默认值的语义**，不是某一次运行的结果。

**为什么必须换掉那个等待（2026-10-28 在 CI 上实测）**：`Startup is still in
progress` 这个字符串**只在提交早了之后才会出现**——Pi 启动期间的提交会被
`handleStartupSubmit` 扣下（文本留在编辑器里，启动完成后也不会自动重发）。
所以「等它消失」的循环第一轮就 break，它看起来像一道等待，实际上什么都没等。
CI 上它表现为：Pi 一直没被问到（mock provider 一个请求都没有），fixture
录不出来，而红灯与 fixture 本身无关。

现在脚本等的是**机器可判定**的正向信号：mock provider 收到请求；没收到就再按
一次 Enter（启动完成后，这一次才会真的把编辑器里的文本发出去）。

---

## v0.14 变异验证（工具并发度）

判据是 `crates/nini-core/tests/agent_integration.rs` 的
`mod parallel_ceiling_tests` 里的两条新断言
（`the_default_ceiling_is_unlimited` 与 `the_default_batch_runs_all_calls_at_once`），
外加差分 oracle `scripts/e2e-parallel-tools.sh`（两侧报各自的 in-flight 峰值，
判据是 **nini peak == Pi peak == K=20**）。

| 注入的 bug | 变红的断言 |
|---|---|
| 把 `crates/nini-core/src/agent.rs` 的 `DEFAULT_MAX_PARALLEL_TOOLS` 改回 `8`（v0.8.9 的旧默认值） | `the_default_ceiling_is_unlimited` 变红（`left: 8, right: 0`）；`the_default_batch_runs_all_calls_at_once` 变红，断言消息写死为 `peak concurrency was 8`。差分 oracle 同时变红：`nini peak=8 ≠ Pi peak=20` |
| 把 `crates/nini-core/src/agent.rs` 里 `max_parallel_tools == 0` 分支的 `specs.len().max(1)` 改成 `1`（永远只跑一个，串行化） | `the_default_batch_runs_all_calls_at_once` 变红，断言消息写死为 `peak concurrency was 1`。差分 oracle 同时变红：`nini peak=1 ≠ Pi peak=20` |

两条注入都实测过：断言变红、oracle 变红，源码已逐字还原（`git diff` 只剩本轮
真正的改动），还原后 `bash scripts/e2e-parallel-tools.sh` 回到退出 0 且
`NINI_PEAK=20`、`PI_PEAK=20`。

**为什么串行化这一条特别重要**：它证明 oracle 量的是**并发**，不是「跑完了」——
一个「全部跑完、但一次只跑一个」的实现同样满足「都跑完了」，而它让峰值从
20 掉到 1。

**为什么 peak 与 sleep 的长短无关**：nini 侧全部 future 在 `FuturesUnordered`
的同一次 poll 里各被首次 poll，每个 future 在第一个 await 之前就把 in-flight
计数加一；Pi 侧 `Promise.all` 在同一个同步 map 里调用每个 `tool.execute`
（`references/pi-0.85.1/src/loop/agent-loop.ts:547-549`）。两侧的工具都是
**进程内**的（没有 fork 开销），所以 peak 恒等于 K。sleep（60ms）只用来保证
in-flight 窗口不塌——若日后把探针换成真 bash（要 fork+exec），需要重新评估。

### 第二轮：把「flag 被丢弃」与「探针测错却报对」这两类坏法钉住

第一轮的两条变异只覆盖了 nini 侧的默认值与 worker pool。审查发现两个盲区：
**flag 的值可以在中途被丢掉**（clap 解析了、`--help` 宣传了，然后被扔掉），
以及 **Pi 侧的探针可以测错却报出正确的数字**（oracle 只信自报值）。两条都补了
断言，注入结果如下（均已还原）：

| 注入的 bug | 变红的断言 |
|---|---|
| 把 `crates/nini-cli/src/main.rs` 里 flag 的值丢掉（`cfg.max_parallel_tools = None`） | `crates/nini-cli/src/main.rs` 的 `cli_wiring_tests`：`the_max_parallel_tools_flag_reaches_the_run_config`（`Args` → `AppConfig` 那一跳）、`the_max_parallel_tools_flag_really_caps_the_concurrency`（峰值 8 ≠ 3）、`the_two_ceilings_do_not_interfere`、`a_settings_file_may_say_zero_explicitly` —— 共 4 条变红 |
| 把 `scripts/pi-parallel-tools-probe.mjs` 的 in-flight 计数改错（`peak = 1`） | 差分 oracle 变红：`PI_PEAK=1 ≠ PI_OVERLAP_PEAK=20` —— 「Pi 侧的两个证据不一致」 |
| 把 `scripts/pi-parallel-tools-probe.mjs` 的时间戳改错（区间塌成一个点） | 差分 oracle 变红：`PI_OVERLAP_PEAK=1 ≠ PI_PEAK=20` —— 同上 |

第一轮的 oracle 只比对「Pi 探针自报的 PI_PEAK」与「nini 测的 NINI_PEAK」，
所以「探针测错却写死 `PI_PEAK=20`」这类坏法 oracle 照样绿。现在 Pi 侧给出
**两个独立证据**（工具体内的计数器 vs 每个调用自己的开始/结束时间戳算出的
重叠区间数），oracle 校验两者一致——**两个来源一致才算数**。

K 也只有一个来源了：脚本的 `K` 同时喂 Pi 侧（`--calls`）与 nini 侧（Rust 测试
读 `NINI_PARALLEL_CALLS`），脚本还会核对 nini 侧报出的 `NINI_CALLS=`。

## v0.15 变异验证（批次级 toolExecution）

判据是差分 oracle `scripts/e2e-tool-execution.sh`（两侧在 `sequential` 与
`parallel` 两种模式下各报一次 in-flight 峰值），外加 nini 侧测量测试
`crates/nini-core/tests/agent_integration.rs` 的
`batch_execution_mode_tests::the_batch_execution_mode_decides_the_peak`。

oracle 的三条断言：① Pi 侧 `PI_TOOL_EXECUTION=sequential` 时
`PI_PEAK == 1` 且 `PI_OVERLAP_PEAK == 1`；② nini 侧 sequential 时
`NINI_SEQ_PEAK == 1`；③ **对照组** parallel 模式下两侧峰值都 `== K=20`。

两条注入都实测过（源码已逐字还原，还原后 oracle 回到退出 0）：

| 注入的 bug | 变红的断言与实测输出 |
|---|---|
| **变异 1**：去掉 `crates/nini-core/src/agent.rs` 里批次降级判定中的 `config.tool_execution == Sequential \|\|`（整个 `batch_is_sequential` 变成 `false`） | `the_batch_execution_mode_decides_the_peak` 变红（`a sequential batch must run one tool call at a time, but the peak was 20`）。差分 oracle 同时变红，退出码 1：<br>`-- sequential：nini peak=20 Pi peak=1 Pi重叠=1`<br>`-- parallel：  nini peak=20 Pi peak=20 Pi重叠=20 K=20`<br>`!! nini 侧 sequential 的峰值是 20，不是 1 —— nini 的批次级串行没有生效` |
| **变异 2**：把同一处的 `parallelism = 1`（`batch_is_sequential` 为真时的串行取值）改成 `specs.len().max(1)`（声明了 sequential 仍然并发） | `the_batch_execution_mode_decides_the_peak` 变红（同上：`peak was 20`）。差分 oracle 同时变红，退出码 1，输出与变异 1 相同：<br>`-- sequential：nini peak=20 Pi peak=1 Pi重叠=1`<br>`!! nini 侧 sequential 的峰值是 20，不是 1 —— nini 的批次级串行没有生效` |

还原后的实测输出（`bash scripts/e2e-tool-execution.sh`，退出码 0）：

```
-- sequential：nini peak=1 Pi peak=1 Pi重叠=1
-- parallel：  nini peak=20 Pi peak=20 Pi重叠=20 K=20
OK：sequential 两侧的峰值都是 1，parallel 两侧的峰值都是 K=20（NINI_SEQ_PEAK=1 PI_SEQ_PEAK=1 NINI_PAR_PEAK=20 PI_PAR_PEAK=20）
```

**为什么这两条注入重要**：它们分别覆盖「批次级开关根本没读」与「读了但没
照做」两种坏法。变异 1 让 `--tool-execution sequential` 变成一句空话（值被
读进来，然后没人用）；变异 2 让串行判定生效却仍然并发跑——只有量**并发**
才抓得住它，「都跑完了」抓不住。

**对照组的意义**：没有断言 ③（parallel 模式两侧峰值都等于 K），一个
「nini 永远 peak==1」的实现也能骗过断言 ②。断言 ③ 保证 oracle 量的是
「模式带来的差别」，不是「nini 恰好只跑一个」。
