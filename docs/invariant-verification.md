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
| `7ca58577` | `sse_utf8_split` | 解析器回到修复前的样子：每个 chunk 单独 `from_utf8` | `a_character_split_across_two_chunks_is_not_an_error`、`a_character_split_across_three_chunks_is_not_an_error`、`a_long_chinese_answer_survives_any_chunking` 三条同时变红（症状：`invalid utf-8: incomplete utf-8 byte sequence from index 0`） |
| `bdfdc06d` | `terminal_guard` | 读线程把通知丢在地上（不进队列） | 4 条守卫断言同时变红：`a_library_write_to_stderr_becomes_a_transcript_notice`、`a_notice_is_never_lost_when_the_state_is_locked`、`the_guard_comes_back_after_the_external_editor`、`the_real_stderr_works_again_after_the_guard_is_dropped` |
| `cc2e35ff` | `notice_rendering` | `/export` 的 Notice 臂去掉 `html_escape` | `a_notice_in_the_export_is_escaped` 变红（`<script>` 原样进导出 HTML） |
| `cc2e35ff` | `notice_rendering` | Notice 臂不折行（`wrap_text_to_width` 换成 `None`） | `a_long_notice_is_wrapped_not_clipped` 变红（长诊断的尾部不在屏上） |
| `cc2e35ff` | `dispatch`（src 内单元测试） | `/copy` fallback 改回「copied N bytes to the transcript」 | `the_copy_fallback_message_says_only_what_is_true` 变红 |
| `72dafe4a` | `terminal_guard` | 退回修复前的退出方式：**不发停止信号** + **无限期 join** 读者线程 | `dropping_the_guard_does_not_wait_for_a_child_that_inherited_the_pipe` 变红（实测卡满 30 秒才失败）——只去掉其中任一机制都不会红，两者合起来才是那个 bug |
| `fe56543d` | `scroll_indicator_no_overlap` | 去掉 `total_items > full_height` 闸（陈旧 `scroll_offset` 又开始凭空造提示） | `a_stale_scroll_offset_does_not_invent_a_hint` 变红 |
| `fe56543d` | `dispatch`（src 内单元测试） | `truncate_for_notice` 不再截断（整条回复又进 transcript） | `a_copy_fallback_notice_is_truncated` 变红 |
| `a9b490df` | `scroll_indicator_no_overlap` | 渲染层退回修复前的写法：列表画满整个 transcript 区，两个指示器画在正文的第一行/最后一行上 | `the_overflow_indicator_does_not_eat_the_first_row_of_the_transcript` + `the_user_scroll_indicator_does_not_eat_the_last_row_of_the_transcript` 变红（其余断言仍绿） |

二十三条注入全部命中，无一条"注入了但测试仍全绿"。

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
