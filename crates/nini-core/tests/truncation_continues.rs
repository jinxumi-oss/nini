//! 截断 = 这一轮没写完：丢掉半截输出 + 塞一句继续提示 + 继续同一轮；
//! 连续截断超过 `MAX_CONSECUTIVE_TRUNCATIONS` 才结束运行并汇报。
//!
//! 锁住 `crates/nini-core/src/agent.rs` 的「改动 1」（D1：长度截断不是
//! 终点）以及「改动 3」的 72 小时口径护栏（T3）。
//!
//! 场景取自用户截图：模型把同一句复读烧光输出预算，`stop_reason="length"`、
//! 本轮无工具调用 —— 旧路径下这一轮直接结束运行（任务停在半路）。

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::{Stream, StreamExt};

use nini_ai::fixture::ProgrammedProvider;
use nini_core::agent::{MAX_CONSECUTIVE_TRUNCATIONS, RUNAWAY_TURN_BACKSTOP};
use nini_core::provider::{Capabilities, ContentBlock, Provider, Request, Usage};
use nini_core::tool::ToolRegistry;
use nini_core::{Agent, AgentEvent, AgentMessage, RunConfig};

/// 继续提示的固定文案（改动 1 的 `TRUNCATION_CONTINUATION_NOTICE`）。
const TRUNCATION_CONTINUATION_NOTICE: &str =
    "[TRUNCATED RESPONSE — CONTINUE]\n\n\
     Your previous response was cut off by the output token limit before it finished, and it did not make any tool call.\n\
     Continue the task now: take the next concrete step (call a tool), or — if the task is already done — report the result.\n\
     Do not repeat the previous output.";

/// A fixture provider whose every turn is a `length` stop with no tool
/// calls — exactly the "the model burned the output budget" shape from the
/// bug report, built from `nini_ai::fixture::ProgrammedProvider`.
fn always_truncating_provider(turns: Vec<Vec<nini_ai::fixture::FixtureTurn>>) -> RecordingProvider {
    let inner = Arc::new(ProgrammedProvider::from_turns(turns));
    RecordingProvider {
        inner,
        calls: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
    }
}

fn truncation_turn() -> Vec<nini_ai::fixture::FixtureTurn> {
    vec![
        nini_ai::fixture::FixtureTurn::Text("Let me read scrollbar.rs.".to_string()),
        nini_ai::fixture::FixtureTurn::Stop {
            stop_reason: "length".into(),
            usage: Usage::default(),
        },
    ]
}

fn normal_turn() -> Vec<nini_ai::fixture::FixtureTurn> {
    vec![
        nini_ai::fixture::FixtureTurn::Text("here is the remaining answer".to_string()),
        nini_ai::fixture::FixtureTurn::Stop {
            stop_reason: "end_turn".into(),
            usage: Usage::default(),
        },
    ]
}

/// A turn with one tool call and a `tool_use` stop — a non-truncated
/// round that does **not** end the run (Pi's `hasMoreToolCalls = true`),
/// which is what makes it usable as the "ordinary turn" that resets the
/// truncation counter without killing the run.
fn toolcall_turn() -> Vec<nini_ai::fixture::FixtureTurn> {
    vec![
        nini_ai::fixture::FixtureTurn::ToolCall {
            name: "read".into(),
            args: serde_json::Value::Null,
        },
        nini_ai::fixture::FixtureTurn::Stop {
            stop_reason: "tool_use".into(),
            usage: Usage::default(),
        },
    ]
}

/// Records every request the inner provider receives: the flattened text
/// of each request's messages plus the call count. Tests assert on the
/// retry context the model actually receives, not on the Agent's own
/// memory.
struct RecordingProvider {
    inner: Arc<dyn Provider + 'static>,
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Vec<String>>>>,
}

impl Provider for RecordingProvider {
    fn name(&self) -> &'static str {
        "programmed-truncating-fixture"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn stream(
        &self,
        req: Request,
    ) -> Pin<
        Box<
            dyn Stream<
                    Item = Result<
                        nini_core::provider::StreamEvent,
                        nini_core::provider::ProviderError,
                    >,
                > + Send
                + 'static,
        >,
    > {
        let mut texts = Vec::new();
        for msg in &req.messages {
            for block in &msg.content {
                if let ContentBlock::Text { text } = block {
                    texts.push(text.clone());
                }
            }
        }
        self.requests.lock().unwrap().push(texts);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.stream(req)
    }
}

/// Drive `agent.run(...)` to completion, returning (emitted text, first
/// error the stream yielded — `None` if the run ended normally).
async fn run(agent: &mut Agent, msg: AgentMessage) -> (String, Option<String>) {
    let mut text = String::new();
    let mut error = None;
    let s = agent.run(msg);
    tokio::pin!(s);
    while let Some(ev) = s.next().await {
        match ev {
            Ok(AgentEvent::TextDelta { text: t }) => text.push_str(&t),
            Ok(_) => {}
            Err(e) => {
                error = Some(e.to_string());
                break;
            }
        }
    }
    (text, error)
}

fn config() -> RunConfig {
    let mut cfg = RunConfig::new("test-model");
    cfg.compaction.context_window = 200;
    cfg.compaction.reserve_tokens = 50;
    cfg.compaction.keep_recent_tokens = 40;
    cfg
}

/// T1 #1: a truncation must not end the run — the model is called again
/// and the run ends normally.
#[tokio::test(flavor = "current_thread")]
async fn a_truncated_response_does_not_prevent_the_run() {
    let provider = Arc::new(always_truncating_provider(vec![
        truncation_turn(),
        normal_turn(),
    ]));
    let mut agent = Agent::new(provider.clone(), ToolRegistry::new(), config());

    let (_transmitted, error) = run(&mut agent, AgentMessage::user("go")).await;
    let calls = provider.calls.load(Ordering::SeqCst);

    assert!(
        calls >= 2,
        "a truncation must not end the run — the model is only called once ({}): the truncated \
         turn ended the run instead of being dropped and continued",
        calls
    );
    assert!(
        error.is_none(),
        "a truncated run must end normally, not in an error: {error:?}"
    );
}

/// T1 #2: the half-finished response must not enter the model's retry
/// context (it is dropped, exactly like Pi's truncated message).
#[tokio::test(flavor = "current_thread")]
async fn the_truncated_text_is_not_in_the_retry_context() {
    let provider = Arc::new(always_truncating_provider(vec![
        truncation_turn(),
        normal_turn(),
    ]));
    let mut agent = Agent::new(provider.clone(), ToolRegistry::new(), config());

    let (_, error) = run(&mut agent, AgentMessage::user("go")).await;
    assert!(
        error.is_none(),
        "a truncated run must end normally: {error:?}"
    );

    let requests = provider.requests.lock().unwrap();
    let second_context = requests.get(1).expect("the model is called at least twice");
    assert!(
        !second_context.iter().any(|s| s.contains("scrollbar")),
        "the half-finished response must not enter the retry context — the \
         model's second request carries the truncated text: {second_context:?}"
    );
}

/// T1 #3: the retry context must carry the continuation notice.
#[tokio::test(flavor = "current_thread")]
async fn the_continuation_notice_is_in_the_retry_context() {
    let provider = Arc::new(always_truncating_provider(vec![
        truncation_turn(),
        normal_turn(),
    ]));
    let mut agent = Agent::new(provider.clone(), ToolRegistry::new(), config());

    let (_, error) = run(&mut agent, AgentMessage::user("go")).await;
    assert!(
        error.is_none(),
        "a truncated run must end normally: {error:?}"
    );

    let requests = provider.requests.lock().unwrap();
    let second_context = requests.get(1).expect("the model is called at least twice");
    assert!(
        second_context
            .iter()
            .any(|s| s.contains(TRUNCATION_CONTINUATION_NOTICE)),
        "the retry context must carry the continuation notice: {second_context:?}"
    );
}

/// T1 #4: repeated truncations are bounded — the run ends with the
/// repeated-truncation error after the cap, and never later.
#[tokio::test(flavor = "current_thread")]
async fn repeated_truncations_stop_the_run() {
    // Six truncation turns: the first five increment the counter up to the
    // cap and the run must still continue; the sixth exceeds it and ends
    // the run with the repeated-truncation error.
    let provider = Arc::new(always_truncating_provider(vec![truncation_turn(); 6]));
    let mut agent = Agent::new(provider.clone(), ToolRegistry::new(), config());

    let (_, error) = run(&mut agent, AgentMessage::user("go")).await;
    let calls = provider.calls.load(Ordering::SeqCst);

    assert!(
        calls <= MAX_CONSECUTIVE_TRUNCATIONS + 1,
        "the run must end with the repeated-truncation error, not later: \
         {calls} provider calls"
    );
    let err =
        error.expect("the run must end with the repeated-truncation error, not finish normally");
    assert!(
        err.contains("output token limit"),
        "the error must name the output token limit: {err}"
    );
    assert!(
        err.contains("send another message to continue"),
        "the error must say what the user can do next: {err}"
    );
}

/// T1 #5: a turn without truncation resets the counter — five truncations
/// after one ordinary turn must stay inside the cap (otherwise the run
/// would hit the repeated-truncation error far earlier).
#[tokio::test(flavor = "current_thread")]
async fn a_turn_without_truncation_resets_the_counter() {
    // trunc, toolcall, trunc, trunc, trunc, normal: the tool-call turn is
    // a non-truncated round that does not end the run; with a working reset
    // the counter clears there and the run survives all five truncations
    // after it and ends normally. Without reset the counter keeps growing
    // and the run would hit the repeated-truncation error.
    let provider = Arc::new(always_truncating_provider(vec![
        truncation_turn(),
        toolcall_turn(),
        truncation_turn(),
        truncation_turn(),
        truncation_turn(),
        normal_turn(),
    ]));
    let mut agent = Agent::new(provider.clone(), ToolRegistry::new(), config());

    let (_, error) = run(&mut agent, AgentMessage::user("go")).await;
    let calls = provider.calls.load(Ordering::SeqCst);

    assert!(
        calls == 6,
        "a non-truncated turn must reset the counter, so the run survives \
         all five truncations after it: {calls} provider calls"
    );
    assert!(
        error.is_none(),
        "the run must not end with the repeated-truncation error: {error:?}"
    );
}

/// T3 (改动 3 的 72 小时口径护栏)：runaway 兜底必须在 72 小时内不触发 ——
/// 约 13 秒/轮，72 小时 ≈ 20_000 轮，所以 `RUNAWAY_TURN_BACKSTOP` 必须
/// ≥ 20_000；否则一个无上限的 72 小时连续工作会在兜底处死掉。
#[tokio::test]
async fn the_runaway_backstop_covers_a_72_hour_run() {
    assert!(
        RUNAWAY_TURN_BACKSTOP >= 20_000,
        "RUNAWAY_TURN_BACKSTOP = {RUNAWAY_TURN_BACKSTOP}: the 72h backstop \
         must cover ~20k turns (72h ÷ ~13s/turn); if it is below, an uncapped \
         72h run would die at the backstop"
    );
}
