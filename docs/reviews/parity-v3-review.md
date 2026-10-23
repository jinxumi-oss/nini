# 审查报告 — parity v3 / A1

审查范围：`git diff`（工作区未提交改动）+ 5 个未跟踪脚本/快照目录。
本轮为**只读审查**：未执行 `cargo build/test`（故所有"测试是否真的红"的结论均为静态推演，见 Answers 第 5 条），
未修改任何被审文件。

## Files Reviewed

- `crates/nini-tui/src/state.rs`（1302-1335 新增 `agent_alive` / `run_seq` 字段及文档注释；1626-1660 `Clone` / `Default` 实现）
- `crates/nini-tui/src/runtime.rs`（391-403 `Done` 事件臂；1813-1830 `KeyAction::Submit` 键处理；1882 abort_signal 安装；2204-2233 `submit_user_input_inner` 新守卫；2415-2437 队列冲刷；2471-2497 spawn 段；2520-2546 watchdog；2661-2890 `apply_action` 第二分派点；2932-2949 `queue_while_running`）
- `crates/nini-tui/tests/esc_concurrency_probe.rs`（1-269，5 条测试）
- `scripts/mock-provider.py`（161-209 请求计时；228-246 `--chunk-delay`）
- `scripts/e2e-interrupt.sh`（1-241，新增）
- `scripts/parity-gap.py`（1-330，新增）
- `scripts/parity-gaps.json`（1-284，新增，22 条 gap）
- `scripts/pi-line-refs.py`（1-124，新增）
- `scripts/pi-oracle-lock.py`（1-241，新增）
- `scripts/plan-selfcheck.py`（1-140，新增）
- `docs/plans/parity-with-pi.md`（重写：§0/§2/§3/§4 W0-W5/§5/§9/§10）
- `docs/GOALS.md`（§2 状态表、§3 排序规则、§P0-6）
- `fixtures/tui/03-interrupt.json`（新增）
- `README.md` / `README.zh-CN.md`（徽章与 provider 数字）

## Critical (must fix)

- `crates/nini-tui/src/runtime.rs:2520` — **watchdog 的 `seq` 取错了轮次**。`run_seq` 在 `2477` 于**锁内**自增，
  但 watchdog 用的 `seq` 是在 `2497`（driver 已 spawn、锁已释放）之后**重新加锁读**的。若 turn N 的 `Done`
  在 `2497..2520` 之间落地（`agent_alive=false`，`mode=Editing`），用户随即提交 turn N+1（`run_seq` 变 N+1），
  则 turn N 的 watchdog 读到的 `seq` 是 **N+1**；等 turn N 的任务退出时，`2526` 的 `run_seq == seq` 成立，
  watchdog 就会把**正在跑的 turn N+1 的守卫清掉**（`2527`）并把 `mode` 改回 `Editing`（`2536`）——
  这正好重新打开了本轮要关的那个窗口（可并发 spawn 第二个 agent）。
  依据：`2477`（自增）与 `2520`（读取）分属两次加锁，中间的 `2497` 是 driver spawn。
  修法：把 `seq` 与 `text` 一起从第一次持锁的块里带出来（`(text, seq)`），watchdog 只用这个值。
- `crates/nini-tui/src/runtime.rs:2226` + `2932-2949` + `2432-2436` — **被 steer 的消息会被写两遍**
  （session 与 transcript 各一遍，模型上下文也会看到两遍）。
  第一次：`queue_while_running` 在 `2944` 调 `state.push_user(text)` → `state.rs:2245` `session_append(Role::User, s)`，
  同时 `2936` 把文本压进 `pending_next_turn_messages`；
  第二次：下一次 spawn 时 `2432` `mem::take` 冲刷队列，`2434` 对每条 `g.push_user(aside.clone())` → **又一次** `session_append(Role::User, …)`。
  且 `2490-2492` 的 seed 在冲刷之后计算，所以模型上下文里同一条用户消息出现两次。
  这是 A 类"数据写重"，与本轮修的是同一类问题；本轮新增的 steer 分支（`2222-2229`）让这条路径从
  "只在 mode==Running 时可达"变成"Esc 之后随时可达"，触发概率大幅上升。
  （该重复在 `queue_while_running` 里是 v0.10.1 就有的旧账，本轮把它接到了新路径上。）
- `crates/nini-tui/src/runtime.rs:2521` — **没有 tokio runtime 上下文时 watchdog 完全不存在**。
  `if let Ok(rt) = tokio::runtime::Handle::try_current()` 没有 `else` 分支：`Err` 时 `handle` 被直接 drop，
  `agent_alive` 永远停在 `true`，此后**任何提交都被 steer 进队列、一条也发不出去**——正是 `state.rs:1324-1326`
  自己写的"the worse failure of the two: it silences every later submission for the life of the process"。
  仓库内唯一的生产调用点在 `1884`（tokio 运行时循环内）所以当前不触发，但这是**静默失败**：
  任何新的/同步的调用方都会得到一个永久哑掉的 TUI，而代码里既没有 `else` 也没有日志。
  至少要加 `else` 分支：直接清 `agent_alive` 或 `tracing::error!`。
- `scripts/parity-gap.py:193-260`（`check_plan`）— **`--check-plan` 现在是空转**。
  实测（只读运行）：
  `note plan §2 table: 0 gap id(s) checked against the ledger` / `note plan §5 declares no 「未闭」 counts — nothing to compare` / 退出码 0。
  原因：v3 把 §2 的差距表删了（`docs/plans/parity-with-pi.md:56-58` 明写"本节不列内容"），§5 也不再有「未闭」计数行，
  于是 `222` 的 `doc_ids` 恒为空、`244` 的 `declared` 恒为空——两个真正的比对**一次都没执行**，
  脚本却打印 "plan cross-check OK" 并退出 0。
  而 `docs/plans/parity-with-pi.md:123` 把这条命令列为台账的验证命令，`scripts/parity-gaps.json` 的 C2
  条目又把"`--check-plan` 退出 0"写成退出判据——**一个恒真的判据被当成证据引用**。
  修法：`doc_ids` 为空或 `declared` 为空时应判红（或显式声明"文档已改为不列表，改为校验 ledger 自身"）。
- `scripts/plan-selfcheck.py:130` — **C9（禁止手写台账计数）永远匹配不到东西**。
  正则 `\*\*\d+ 条\*\*（A 类|A 类 \d+ / B 类` 在当前计划文档上匹配结果为空（实测），
  而文档恰恰手写了计数：`docs/plans/parity-with-pi.md:122`「19 条差距的唯一数据源」、
  `docs/GOALS.md:80`「**19 条**（A 4 / B 11 / C 3 / D 1），open 16、blocked-owner 3」——
  实际台账是 **22 条（A 6 / B 11 / C 3 / D 2），open 18、blocked-owner 3、fixed 1**
  （实测 `python3 scripts/parity-gap.py --json`）。差值正好是本轮新增的 A5 / A6 / D2。
  也就是说：C9 这个"专门防手写计数"的门禁，对本轮自己引入的过期计数**完全无感**。

## Warnings (should fix)

- `crates/nini-tui/src/runtime.rs:2540` — watchdog 的告警**每轮都会误报**：
  `tracing::warn!("agent task ended without a Done event")` 在 watchdog 的出口无条件打印，
  而 watchdog 是**每个任务退出都会跑**的（正常 turn 也跑）。正常路径下 `Done` 已经把 `agent_alive` 清了，
  watchdog 只是重复清一次并打一条"没有 Done"的警告。日志若恒真就等于没有日志。
  建议：watchdog 里先判断 `agent_alive` 是否仍为 `true`（或让 `Done` 打一个 `done_seen` 标记）再决定是否告警。
- `crates/nini-tui/src/runtime.rs:1882` + `2222-2229` — **steer 提交会把在跑那一轮的 abort_signal 换掉**。
  `handle_key` 的 Submit 臂在调用 `submit_user_input` 之前无条件
  `state.run_state.abort_signal = Some(Arc::new(Notify::new()))`；Esc 通知的是**旧的**那个 signal（`1908`），
  用户随后按 Enter 走 steer 分支时，signal 被换成一个新的、**从未被 notify 过**的对象。
  今天没有 driver 读 `abort_signal`（全仓只有 `state.rs:1351/1635/1656` 与 `nini-core/src/agent_hooks.rs:21` 的注释），
  所以暂时无害；但一旦按注释里说的"future driver-side wiring"接上，**abort 会静默失效**。
  建议：只有真正 spawn 成功的那条路径才安装新的 abort_signal。
- `crates/nini-tui/src/runtime.rs:391-403` — `Done` 臂**不检查 `run_seq`**，而 watchdog 检查（`2526`）。
  两个写者语义不对称：只要有一个**迟到的 `Done`**（driver 在 `Done` 之后又推一次、或 driver 连推两轮 `Done`），
  就能把新一轮的 `agent_alive` 清掉，重新打开并发 spawn 的窗口。
  依据：当前生产 driver（`crates/nini-cli/src/app.rs:595`）每轮只推一次 `Done`，所以今天不会触发；
  但守卫的正确性不该依赖"driver 恰好只推一次"。
  建议：`Done` 也带轮次（或在 `Done` 臂里比对 `run_seq`）。
- `crates/nini-tui/src/runtime.rs:2523` — watchdog **没有超时**：它 `await` 的是 agent 任务本身。
  provider 卡死（TCP 读不返回）→ 任务永不结束 → `agent_alive` 永远为 `true` → TUI 再次变成"哑巴"。
  这与 Critical 第 3 条是同一类失效，但发生在有 runtime 的正常路径上。
  建议：给 watchdog 加一个兜底超时（例如 N 秒后强制清 `agent_alive` 并提示"agent 无响应"）。
- `crates/nini-tui/src/runtime.rs:2497` — **driver 闭包本身 panic 会跳过 watchdog**：
  `(agent_driver)(text, seed, sink, done.clone())` 若在返回 JoinHandle 之前 panic，栈展开会越过 `2521` 的
  `rt.spawn`，`agent_alive` 停在 `true`，永久哑掉。
  （任务**内部** panic 是安全的：`handle.await` 返回 `Err`，被 `let _ =` 吞掉，watchdog 照常复位——这条覆盖到了。）
  建议：把 `agent_alive = true` 的设置与 driver 调用放进同一个 `catch_unwind`，或先 spawn 再置位。
- `crates/nini-tui/tests/esc_concurrency_probe.rs:44-56` — **5 条测试共用同一个临时目录**：
  `fresh_state()` 用 `std::env::temp_dir().join(format!("nini-escwin-{}", std::process::id()))`，
  pid 相同 → 目录相同 → 每次调用都 `remove_dir_all` + `create_dir_all`，且 `session_init(dir.join("s.jsonl"))`
  是**同一个 session 文件路径**。Rust 默认并行跑同文件的测试 → 测试之间会互删目录/互写同一份 session。
  目前没有断言去读 session 文件，所以还没炸，但这是明确的 flake 温床。
  建议：用 `nini-escwin-{pid}-{测试名}` 之类的唯一目录。
- `crates/nini-tui/tests/esc_concurrency_probe.rs:265-297` — `an_erroring_turn_reopens_the_editor`
  **名不副实**：`AgentEventLite::Error("429 rate limited")` 推出去之后**没有任何断言**，
  守卫的释放完全由随后的 `Done` 造成。即使把 `AgentEventLite::Error` 的整个处理分支删掉，这条测试仍然绿。
  它实际上是 `a_done_event_reopens_the_editor_for_the_next_turn` 的重复。
  建议：要么断言 Error 在 transcript/状态栏里可见，要么把它合并进 Done 那条。
- `scripts/e2e-interrupt.sh:171` — **重叠判据在缺 END 时间戳时整段跳过**：
  `if i < len(ends) and starts[j] < ends[i]`。而 END 行是在 `resp.write_eof()` 之后才打印的
  （`scripts/mock-provider.py:203-208`）；客户端一旦断开（正是"中断"场景），`write_eof` 抛异常 → **没有 END 行**
  → `ends` 缺项 → 重叠检测被跳过 → **"第二个 agent 在第一个还在流时起来"这个签名反而检测不到**。
  建议：`ends` 缺失时应判为"第一个请求未结束"，此时任何后续 START 都应判红。
- `scripts/e2e-interrupt.sh:226-232` — **判据 2b 在第二轮根本没跑的情况下也判 PASS**：
  `elif second is None: print("  (第二轮的正文没落盘——它还在队列里)")` 之后直接落到最后的 "✓ session 写入顺序单调"。
  而按本轮修复后的语义，第二次提交被 steer 进队列、**根本不会产生第二次请求**，
  所以 `SECOND_TURN_MARKER` 永远不出现——这条 e2e 在"修复生效"时走的就是这个不失败的分支。
  结果：脚本无法区分"第二轮被正确延后"与"第二次提交被静默吞掉"（正是台账 A6 描述的状态）。
  建议：把"第二轮没有开始"从"提示"改成显式判据（例如断言队列里确有该消息，且在第三次提交后被投递）。
- `docs/GOALS.md:78` / `docs/plans/parity-with-pi.md:124` — **行号引用计数在本轮就被自己写错了**：
  文档写「89 条（`.js` 31 + `.ts` 58，22 个文件）」，实测 `python3 scripts/pi-line-refs.py --json` = **90 条**
  （`.js` 32 + `.ts` 58，22 个文件）。HEAD 上是 89 条——**本轮新增的 `runtime.rs:2218` 那条
  `interactive-mode.js:2491` 引用把它变成了 90，而文档仍写 89**。
  工具能算出来的数字，文档写错了，没有任何检查会红。
- `crates/nini-tui/src/runtime.rs:2218` — **新加的 Pi 行号引用指错了行**：
  注释说 `streamingBehavior: "steer"`（`interactive-mode.js:2491`）。对照快照
  `references/pi-0.84.2/dist/interactive-mode.js`：2491 行是注释
  `// This handles extension commands (execute immediately), prompt template expansion, and queueing`；
  `if (this.session.isStreaming)` 在 **2492**，`await this.session.prompt(text, { streamingBehavior: "steer" })` 在 **2495**。
  台账 A1 的 `pi_basis` 写的是 2492（对），新代码写 2491（差一行，且指向注释行）。
  `--verify-refs` 只能判"行号越界"，抓不到这种语义漂移——而这正是计划 §1 说"只能人工核对"的那一类。
  建议：改成 2495（或 2492）。

## Suggestions (consider)

- `crates/nini-tui/src/runtime.rs:2222-2229` — steer 分支改动了可见状态（transcript + `status = "queued: N pending"`），
  但**没有 `done.notify_waiters()`**（对比 `1169-1174` 的 `drop_and_signal`）。运行时靠 tick 重绘，
  所以状态行会晚一拍才出现"queued"。建议补一次唤醒。
- `crates/nini-tui/src/runtime.rs:1887-1931` — Esc 分支的 `else { state.input.clear(); }`：
  在"agent 活着但 `mode==Editing`"这个新状态下按 Esc 会**直接清空输入框**（用户刚敲的字没了）。
  这个窗口比 v0.10.1 时长得多（现在覆盖整个 abort 期间）。建议：新状态下第一次 Esc 只清 completion、不清输入。
- `crates/nini-tui/src/runtime.rs:2258`（`cx.run_blocking`）——状态锁被**持有跨越一次阻塞的 shell 调用**。
  这是旧账，但本轮把"守卫的原子性"押在了同一把锁上（`2222` 检查与 `2476` 置位同处一次持锁），
  锁被拉长意味着守卫在这段时间里对 agent 事件也不可用。建议把 bash 执行移出临界区。
- `crates/nini-tui/src/runtime.rs:2535-2538` — watchdog 把 `mode` 改回 `Editing` 的条件是 `mode == Running`，
  但它**不知道当前 Running 属于哪一轮**（依赖已损坏的 `seq`）。修好 Critical 第 1 条后这段才安全。
- `scripts/parity-gaps.json`（A1 的 `oracle` 字段）— 列出的测试名 `never_starts_a_second_agent` 与真实测试名
  `submitting_soon_after_an_esc_never_starts_a_second_agent` 不一致（简写）。既然是"机器可枚举"的台账，建议写全名。
- `docs/GOALS.md:74`（1554 通过）与 `README.md:11` / `README.zh-CN.md:11`（1555 passing）不一致；
  本轮禁止跑测试，**未确认**哪个是对的。

## Answers

### 1. `agent_alive` 有没有任何路径能漏置 false，导致"之后再也发不出消息"？

**有，至少 4 条。** 逐条对照：

| 路径 | 结论 | 依据 |
|---|---|---|
| spawn 时置位 | ✅ 正确，只在真正要 spawn 时才置位 | `runtime.rs:2476`（置位在 `text`/`seed` 都算完之后、driver spawn 之前） |
| 提前 return（斜杠命令 / 压缩分支 / bash 分支） | ✅ 安全，都在置位之前 return，标志从未抬起 | `runtime.rs:2330-2406`（斜杠命令分支，其 return 在 2406）、`2415-2420`（压缩）、`2258-2325`（bash） |
| `Done` 事件 | ✅ 正常路径会清 | `runtime.rs:401` |
| 任务 panic（任务**内部**） | ✅ 被 watchdog 覆盖：`handle.await` 返回 `Err`，被 `let _ =` 吞掉，watchdog 照常复位 | `runtime.rs:2524-2527` |
| 任务提前 `return`（没推 Done） | ✅ 被 watchdog 覆盖 | 同上 |
| 锁中毒 | ✅ 基本安全：`AgentSink::push` 在 `if let Ok(mut s) = self.state.lock()`（`240`）下中毒会**丢掉事件**（包括 `Done`），但 watchdog 仍会复位；`lock_state`（`59-61`）对中毒用 `into_inner()` 不会 panic | `runtime.rs:59-61, 240, 2524-2527` |
| **无 tokio runtime 上下文** | ❌ **漏**：`try_current()` 为 `Err` 时没有 `else`，watchdog 根本不存在，`agent_alive` 永远 `true` | `runtime.rs:2521` |
| **driver 闭包在返回 handle 之前 panic** | ❌ **漏**：栈展开越过 `rt.spawn`，watchdog 从未启动 | `runtime.rs:2497` vs `2521` |
| **任务永不结束（provider 卡死）** | ❌ **漏**：watchdog `await` 的是任务本身，没有超时 | `runtime.rs:2524` |
| **迟到的 `Done`** | ❌ **漏**：Done 臂不比对 `run_seq`，迟到的 Done 会清掉新一轮的守卫 | `runtime.rs:391-403`（对比 `2526` 有比对） |

即：`state.rs:1324-1326` 写的"missed reset 是更坏的失效"并没有被完全堵住——
**三条路径（无 runtime / driver 前置 panic / 任务挂死）都会让 `agent_alive` 永久停在 `true`，
此后每次提交都走 `2222-2229` 的 steer 分支，一条也发不出去。**

### 2. `run_seq` 的重入保护是否真的成立？watchdog 与新一轮 spawn 交错有没有竞态窗口？有没有 ABA？

**不成立。** 存在一个真实的竞态窗口：

- `2477` 在**第一次持锁**里自增 `run_seq`；
- `2497` spawn driver（**锁已释放**）；
- `2520` **重新加锁**读 `run_state.run_seq` 当作本轮的 `seq`。

这三步之间没有原子性。若 turn N 的 `Done` 在 `2497..2520` 之间落地 → `agent_alive=false`、`mode=Editing`
→ 用户提交 turn N+1 → `run_seq` 变成 N+1 → turn N 的代码在 `2520` 读到的 `seq` 是 **N+1**。
之后 turn N 的 watchdog（`2523`）判定 `run_seq == seq`（N+1 == N+1）成立 →
**把正在跑的 turn N+1 的 `agent_alive` 清成 false（`2527`），并把它的 `mode` 从 Running 改成 Editing（`2536`）**。
守卫在这一瞬间被拆掉，用户再按一次 Enter 就会并发 spawn 第二个 agent——正是本轮要消灭的缺陷。

**ABA：** `run_seq` 是 `u64` 的 `wrapping_add`（`2477`），实际使用中不可能回绕，**无 ABA 问题**；
但"用 `seq` 表示轮次"这个设计本身没错，错的是**取值的时机**（应在 `2477` 同一次持锁内一并取出）。
另外 `Done` 臂完全不看 `run_seq`（`401`），与 watchdog 的语义不对称（见 Warnings）。

### 3. Esc 设 `mode = Editing`、守卫读 `agent_alive`，会不会出现语义打架的新状态？

**会出现这个新状态（agent 活着 + `mode == Editing`），而且它就是本轮 steer 分支的触发条件。**

- Esc 分支：`runtime.rs:1927` 把 `mode` 置 `Editing`，**不动 `agent_alive`**（按设计）。
- 守卫：`2222-2229`，`agent_alive == true` 且 `mode == Editing` → 走 steer（进队列），**不新建 turn**。

判断：**这不是行为回归，而是修复本身**（旧行为是并发 spawn）。但它带来三个副作用，其中一个是真的问题：

1. **提交不再立即开新 turn**，而是被排队，直到下一次 spawn 才冲刷（`2432`）。
   这正是台账 A6 描述的"nini 只在下一次 spawn 时冲队"，本轮**没有解决 A6**，只是给它加了一条新入口。
2. **Esc 之后按 Esc 会清空输入框**：`1896` 的分支链里 `mode == Running` 已经不成立，
   落到 `else { state.input.clear(); }`（`1929-1930`）——用户刚敲的字被无声吃掉。窗口比 v0.10.1 长得多。
3. **steer 会替换在跑那一轮的 abort_signal**（`1882`），见 Warnings。

另外 `agent_alive` 与 `mode` 的组合还会产生一个此前不存在的状态：
`agent_alive == true` 且 `mode == Running` 时，`submit_user_input_inner` 直接 `return`（`2229`），
即**"有 agent 在跑"但提交被静默吞掉**——目前 `handle_key` 的 Submit 臂（`1823-1830`）在 `mode == Running`
时已经先做了 queue，所以走不到这里；但任何绕过 `handle_key` 的调用方（如事件总线的第二分派点
`2661` 起的 `apply_action`，它在 `2817-2830` 只做 `push_user`、不 spawn）语义并不统一，值得再核一遍。

### 4. steer 分支的消息会不会被写两遍（session 里出现重复 user 消息）？

**会，一定会。** 行号依据：

1. `runtime.rs:2226` → `queue_while_running(&mut g, &steered)`；
2. `queue_while_running`（`2932-2949`）里：
   - `2936` 把文本压进 `pending_next_turn_messages`；
   - **`2944` `state.push_user(text)`** → `state.rs:2237-2246` → `2245` `self.session_append(None, Role::User, s)`
     → **第一次写进 session**；同时 transcript 多一条 `TranscriptLine::User` + 一条 divider；
3. 下一次 spawn 时，`runtime.rs:2432` `let queued = std::mem::take(&mut g.run_state.pending_next_turn_messages);`
   然后 `2434-2436` 对每条 aside 再来一次 `g.push_user(aside.clone())`
   → **第二次写进 session**，transcript 也再出现一遍；
4. `2490-2492` 的 seed 在冲刷**之后**计算 → 模型这一轮收到的上下文里，同一条用户消息出现两次。

即：**session JSONL 里会有两条内容相同的 `role: user` 条目，屏幕上也会看到同一条消息出现两次。**
（该重复是 `queue_while_running` 自 v0.10.1 起就有的旧账，但本轮新增的 `2222-2229` 让它从
"仅 mode==Running 时可达"变成"Esc 之后随手可达"，并且这一轮没有为此写任何测试。）

### 5. 测试是不是"真的能红"？有没有恒真 / 与被测行为无关的断言？

逐条静态推演（**本轮禁止 cargo，故为静态结论**）：

| 测试 | 能不能红 | 判断 |
|---|---|---|
| `submitting_soon_after_an_esc_never_starts_a_second_agent`（`69-175`） | ✅ 能 | 去掉新守卫（改回 `mode` 守卫）后，Esc 把 `mode` 置 Editing → 第二次 `submit_user_input` 会真的 spawn → `spawns==2` → `assert_eq!(spawned, 1)` 红。**这是本轮唯一真正钉住新行为的测试。** |
| `the_done_event_itself_clears_the_guard`（`240-263`） | ✅ 能 | 直接 push `Done`，绕开 driver/watchdog；删掉 `runtime.rs:401` 必红。（计划的变异验证第一条成立） |
| `a_driver_that_never_says_done_still_reopens_the_editor`（`205-238`） | ✅ 能 | driver 返回 `tokio::spawn(async {})`（立即结束、无 `Done`）；拆掉 watchdog 复位 → `agent_alive` 停在 true → `assert!(!agent_alive)` 红。（变异验证第二条成立） |
| `a_done_event_reopens_the_editor_for_the_next_turn`（`178-203`） | ✅ 能（但**测不到 Done 本身**） | 文件自己在 `244-247` 承认：走的是 `submit_user_input`，装了 watchdog，**分不清是 `Done` 清的还是 watchdog 清的**。第 4 条测试才是钉 `Done` 的那条。这条本身只是"守卫最终会落下"的回归网。 |
| `an_erroring_turn_reopens_the_editor`（`266-297`） | ⚠️ **对得起名字的部分恒真** | `AgentEventLite::Error("429 rate limited")` 推出去后**没有任何断言**；守卫的释放完全由随后的 `Done` 造成。把 `AgentEventLite::Error` 的整个处理分支删掉，这条测试仍然绿。它是第 3 条的重复。 |

另外两条与"测试"无关但影响可信度的隐患：
- `fresh_state()`（`44-56`）**5 条测试共用同一个临时目录与同一个 `s.jsonl`**，并行执行会互删/互写 → flake 温床（见 Warnings）。
- 没有任何测试覆盖 **steer 消息不重复写**（Answers 第 4 条），也没有测试覆盖 `run_seq` 交错（Answers 第 2 条）。

### 6. 脚本类新增物的判据有没有形同虚设的可能？

**有，四处，实测确认：**

1. **`scripts/parity-gap.py --check-plan` 空转**（Critical）：`222` 拿到 0 个 gap id、`244` 拿到 0 个声明计数，
   两个比对一次都没跑，却打印 "plan cross-check OK" 并退出 0。根因是 v3 把 §2 的表删了、§5 的计数也删了，
   而脚本没跟上。**"永远走不到的分支"成立。**
2. **`scripts/plan-selfcheck.py:130` 的 C9 正则匹配不到任何东西**（实测 `findall` 返回 `[]`）。
   形式是 `\*\*\d+ 条\*\*（A 类|A 类 \d+ / B 类`，而文档里实际的手写计数是
   `19 条差距的唯一数据源`（`parity-with-pi.md:122`）与 `**19 条**（A 4 / B 11 …）`（`GOALS.md:80`），
   两个形式都匹配不上。**结果：本轮引入的过期计数（实际 22 条）被 C9 放行。**
3. **`scripts/plan-selfcheck.py:120-127` 的 C8 也有空洞**：模式表是 `51 个 / 5 原生 / 51 providers / 5 of ~51`，
   而计划里实际出现过的是「51 / 5」（`parity-with-pi.md:244`）——这个形式不在模式表里，靠 `ctx` 里出现
   "推翻" 二字才没被判红。属于"正则写错导致统计为 0"的同类问题。
4. **`scripts/e2e-interrupt.sh` 的两条判据都能在不发生目标行为的情况下判 PASS**：
   - `171` 的重叠检测在缺 END 时间戳时整段跳过（客户端断开 → mock 的 `write_eof` 抛异常 → 无 END 行，
     `scripts/mock-provider.py:203-208`），恰好是"中断"场景；
   - `226-232`：第二轮没跑时只打印提示、不判红。而按本轮修复后的语义第二次提交被 steer 进队列、
     **根本不会产生第二次请求**，所以 `SECOND_TURN_MARKER` 永远不出现——e2e 在"修复生效"时走的就是这条不失败的分支。
   此外 `session 文件` 用 `find ... | head -1`（`182`）挑选，多文件时取到哪个不确定。

   反过来，**`scripts/pi-line-refs.py` 与 `scripts/pi-oracle-lock.py` 是有效的**：
   实测 `--check` 12 个文件 sha256 全对；`--verify-refs` 解析 90 条引用，报出 1 条 DRIFT
   （`crates/nini-core/src/agent.rs:1579` 的 `agent-loop.js:582` 越界）并**退出 1**——
   这条判据真的会红（也意味着计划 §4 W0 剩余项 1 的退出判据当前**未满足**，而 GOALS 已在 `78` 行记录在案）。

### 7. 有没有引入并发问题（多个线程同时读写 run_state）？

**没有数据竞争（Rust 类型层面），但有两处逻辑竞态。**

- 数据层面：`run_state` 只在 `SharedState = Arc<Mutex<AppState>>` 后面，`lock_state`（`59-61`）统一加锁，
  毒化时用 `into_inner()` 不 panic。`RunState` 的 `Clone`（`state.rs:1626-1645`）已补上两个新字段，
  `Default`（`1647-1666`）也补了默认值；`grep "RunState {"` 全仓只有定义处与两个 impl，
  **没有遗漏的结构体字面量**（不会编译失败）。
- 逻辑竞态 1：**`seq` 取值时机**（Critical 第 1 条，`2477` vs `2520`）——watchdog 可能清掉新一轮的守卫。
- 逻辑竞态 2：**`Done` 不带轮次**（`391-403`）而 watchdog 带（`2526`）——两个写者对"我是哪一轮"的判断不一致。
- 逻辑竞态 3（较轻）：`Done`（`401`）与 watchdog（`2527`）**都**会写 `agent_alive=false`，
  两者之间没有互斥标记；watchdog 若先跑，会把 `mode` 从 `Running` 改成 `Editing`（`2536`），
  而 `Done` 稍后再跑又把 `mode` 置 `Editing`（`393`）——两者结论一致，目前无害，
  但一旦以后给 `Done` 加语义（例如区分正常结束与被中断），这两条路径必须合并成一个。
- 另有一处**既有**的锁持有过长问题（非本轮引入，但与本轮"守卫原子性"的前提相关）：
  `2258` 在持有状态锁的情况下跑阻塞 shell（`cx.run_blocking`）。

---

# 处置（2026-10-23，执行者回执）

审查提出的 5 条 Critical **全部复核为真**，全部已修，且每条修复都配了一条
**变异验证**（注入 → 断言变红 → 还原）。审查的其他 8 条警告也逐条处置。

| # | Critical | 复核 | 处置 | 凭据 |
|---|---|---|---|---|
| 1 | watchdog 的 `seq` 在锁外重读，可能解除**新一轮**的守卫 | **真**。`seq` 在 spawn 之后才读，中间可能已经开始了 turn N+1，于是 turn N 的 watchdog 会把 turn N+1 的守卫清掉——正是 A1 要关的窗口 | `seq` 改为**在抬起守卫的同一个临界区**里读出（`runtime.rs` 的 `let seq = g.run_state.run_seq;` 与 `agent_alive = true` 同行） | 变异：把 `Done` 的 seq 判定改成恒真 → `a_late_done_from_an_older_turn_does_not_clear_the_newer_turns_guard` 变红 |
| 2 | steer 的消息被写进 session **两遍**（排队时 + 冲刷时） | **真**。`queue_while_running` 调 `push_user`（transcript + session），而下一次 spawn 的冲刷又 `push_user` 一次 → 模型看到两遍、`/resume` 显示两遍 | 排队只写 transcript（新增 `push_user_queued`），**投递**才写 session（新增 `deliver_queued_user`） | 变异：把 `push_user_queued` 换回 `push_user` → `a_queued_message_is_written_to_the_session_once` 变红 |
| 3 | `try_current()` 失败时没有 watchdog → `agent_alive` 永久 true → TUI 全程哑掉 | **真**（虽然生产路径总在 runtime 里，但哑掉是静默且彻底的） | `Err` 分支现在直接清标志并告警——没有 runtime 就没有 agent 真的跑起来 | 人工复核：分支已覆盖；生产路径不可达（driver 自己也要 runtime 才能 spawn） |
| 4 | `parity-gap.py --check-plan` 空转（"0 gap id(s) checked" 却退出 0） | **真**。v3 把差距表搬进了台账，旧检查读不到任何 id，于是永远通过 | 重写为**基于正文**的交叉校验：计划提到的每个差距 id 必须存在于台账；台账标 `fixed` 的差距必须在计划里被记录；一个 id 都提不到 → 直接判失败 | 负向：计划里写一个不存在的 id → 退出 1 |
| 5 | `plan-selfcheck.py` 的 S9 正则匹配 0 条，空转 | **真** | 换成一组真能命中的模式；并新增 **S10**：每条正则判据都要能抓到它自己的合成样本（**判据本身不许是死的**） | 负向：计划里塞一句「共 22 条差距」→ S9 变红；塞「51 个 provider」→ S8 变红 |

## 其他警告的处置

| 警告 | 处置 |
|---|---|
| watchdog 每轮都 `warn!`「ended without a Done event」 | 现在只在 watchdog **真的需要**清标志时告警（`Done` 已经清过就直接返回） |
| `Done` 不比对 `run_seq` | 修：sink 带 `seq`（`AgentSink::with_seq`），迟到的 `Done` 被识别为陈旧事件并忽略（对应 Critical 1） |
| steer 会替换在跑那轮的 `abort_signal` | 未复现：steer 分支不碰 `abort_signal`；记入待复核 |
| watchdog 没有超时 | 修：加 10 分钟超时（`WATCHDOG_TIMEOUT_SECS`），超时的 driver 也会清标志 |
| `fresh_state()` 五个测试共用同一个临时目录 | 修：目录名带原子计数器，测试之间不再互相删除对方的 session 文件 |
| `an_erroring_turn_reopens_the_editor` 里的 Error 分支没有断言 | 修：断言 `agent_alive` 与 `mode` 都被复位 |
| e2e 判据 1 在缺 END 时整段跳过 | 修：mock 现在会打印 `CUT`（客户端挂断），缺 END/CUT 的请求直接判失败（时间线有洞不能用来判重叠） |
| e2e 判据 2b 第二轮没跑也不判红 | 修：第一轮的正文缺失 → 判失败；第二轮没落盘 → 说明它还在队列里（差距 A6），不判失败但必须说明 |
| 文档写 89 条行号引用，实际会变 | 修：文档不再写死数字，改为指向 `scripts/pi-line-refs.py --json` |
| 新引用 `interactive-mode.js:2491` 指到注释行 | 记入 W0 剩余项 1（逐条复核语义漂移） |

## 顺带发现并修掉的第三个缺陷

steer 分支原本要求 `mode == Editing` 才排队。而在**回合运行中** `mode` 是 `Running`，
于是走 API 路径的提交（driver、测试、未来的调用方）**静默丢弃了用户的输入**：
文本留在输入框里，什么都没排队。产品路径（按键处理）自己会排队，所以这个缺陷
只在 API 路径上暴露——正是新写的测试把它挖了出来。现在两种状态都排队
（只有 `Quitting` 不排队）。

变异：把条件改回 `mode == Editing` → `a_queued_message_is_written_to_the_session_once` 变红。
