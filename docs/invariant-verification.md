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

## 已声明但**尚未独立复验**（执行者声称，审计者未复现）

| sha | 声明于 | 状态 |
|---|---|---|
| `8744ceb5` | `welcome_card`、`stop_reason_notice`、`stop_reason_producer` | ❌ 未复验。同一个 sha 的第 4 个测试 `narrow_hints` **已独立复验**（阈值 70→50，`no_narrow_width_between_50_and_69_truncates_the_hint_set` 变红） |

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
