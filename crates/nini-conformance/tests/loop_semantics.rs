//! Oracle: nini's agent loop must have Pi 0.84.2's two-layer structure.
//!
//! # What Pi actually does
//!
//! Verified against the installed binary's source,
//! `pi-agent-core@0.84.2/dist/agent-loop.js` (552 lines):
//!
//! ```text
//!  83   pendingMessages = await config.getSteeringMessages?.()   // before the outer loop
//!  85   while (true) {                                           // L1 outer
//!  88     while (hasMoreToolCalls || pendingMessages.length > 0) { // L2 inner
//!  96-103   ... inject pendingMessages ...                      // steering AND follow-up
//!  106       const message = await streamAssistantResponse(...)
//!  121-122   stopReason === "length" ? failToolCallsFromTruncatedMessage(...) : execute(...)
//!  127       hasMoreToolCalls = !batch.terminate
//!  151       if (await config.shouldStopAfterTurn?.({...})) { emit(agent_end); return }
//!  160       pendingMessages = await config.getSteeringMessages?.()   // EVERY turn, unconditional
//!     }
//!  163     const followUpMessages = await config.getFollowUpMessages?.()   // only after L2 exits
//!  166     pendingMessages = followUpMessages
//!  167     continue
//!  170   }
//! ```
//!
//! The outer loop exists for one reason: without it, a follow-up queued
//! *after* the model finished would sit in the queue forever. It re-seeds
//! `pendingMessages` and guarantees **at least one more LLM call**.
//!
//! # The defect this pins down
//!
//! nini's `run()` has a single `loop {` (`agent.rs:741`). On the no-tool-call
//! path it pushes follow-up messages into `self.messages` and returns
//! (`agent.rs:1179-1199`). The follow-up therefore stops being a *task* and
//! becomes *dead context*: on the user's next message the model sees it as
//! background, not as work to do.
//!
//! Pi treats it as a new turn. The difference is categorical, not cosmetic.
//!
//! The existing test at `agent.rs:2668-2670` asserts only that the hook fired
//! and that `messages().len()` grew — it never asserts that a second LLM call
//! happened, so it locks in the broken behaviour. That is why the tests in
//! this file count provider calls instead of counting messages.

use futures_util::Stream;
use nini_core::agent::Agent;
use nini_core::agent_hooks::AgentLoopHooks;
use nini_core::provider::{
    Capabilities, Provider, ProviderError, Request, StreamEvent, Usage as PUsage,
};
use nini_core::tool::ToolRegistry;
use nini_core::RunConfig;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Counts how many times the model is actually invoked. The number of calls
/// is the only reliable witness of "did another turn happen"; message counts
/// are ambiguous because the broken path also appends messages.
struct CountingProvider {
    calls: Arc<AtomicUsize>,
}

impl Provider for CountingProvider {
    fn name(&self) -> &'static str {
        "counting"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn stream(
        &self,
        _req: Request,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(futures_util::stream::iter(vec![
            Ok(StreamEvent::MessageStart {
                id: format!("msg-{n}"),
                model: "test".into(),
            }),
            Ok(StreamEvent::TextDelta {
                text: format!("reply {n}"),
            }),
            Ok(StreamEvent::MessageStop {
                stop_reason: "stop".into(),
                usage: PUsage::default(),
            }),
        ]))
    }
}

/// Returns a fixed steering list and follow-up list, and counts polls.
struct Queue {
    steering: Vec<nini_core::provider::Message>,
    followup: Vec<nini_core::provider::Message>,
    steering_polls: AtomicUsize,
    followup_polls: AtomicUsize,
}

impl AgentLoopHooks for Queue {
    fn get_steering_messages(
        &self,
        _recent: &[nini_core::provider::Message],
    ) -> Vec<nini_core::provider::Message> {
        self.steering_polls.fetch_add(1, Ordering::SeqCst);
        self.steering.clone()
    }
    fn get_followup_messages(
        &self,
        _messages: &[nini_core::provider::Message],
    ) -> Vec<nini_core::provider::Message> {
        self.followup_polls.fetch_add(1, Ordering::SeqCst);
        self.followup.clone()
    }
}

fn user(text: &str) -> nini_core::provider::Message {
    nini_core::provider::Message::user(text)
}

async fn drain(agent: &mut Agent, input: &str) {
    use futures_util::StreamExt;
    let s = agent.run(nini_core::AgentMessage::user(input));
    tokio::pin!(s);
    while let Some(_ev) = s.next().await {}
}

// =====================================================================
// L1 — the outer loop
// =====================================================================

/// A follow-up queued at the end of a run must produce **another model
/// call**. Pi: `agent-loop.js:163-167` re-seeds `pendingMessages` and
/// `continue`s, and the inner loop's condition
/// (`pendingMessages.length > 0`) guarantees the LLM is called again.
// v0.10 (R1): `#[ignore]` because the fix is not landed yet. The test is
// deliberately left in the suite rather than deleted, and deliberately left
// red: it is the executable statement of the defect, with the Pi source line
// that proves the expectation. Flip the attribute off when R1 lands.
//
// Do NOT "fix" a red run here by weakening the assertion. `assert_eq!(calls,
// 2)` is the whole point: nini calls the model once, Pi calls it twice.
#[tokio::test(flavor = "current_thread")]
#[ignore = "R1 (Pi L1 outer loop) not implemented — expected 2 model calls, nini makes 1"]
async fn followup_triggers_another_model_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let hooks = Arc::new(Queue {
        steering: vec![],
        followup: vec![user("now write the docs")],
        steering_polls: AtomicUsize::new(0),
        followup_polls: AtomicUsize::new(0),
    });
    let mut agent = Agent::with_hooks(
        Arc::new(CountingProvider {
            calls: calls.clone(),
        }),
        ToolRegistry::new(),
        RunConfig::new("test-model"),
        hooks,
    );

    drain(&mut agent, "fix the login bug").await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "Pi's L1 outer loop calls the model again for a queued follow-up. \
         nini pushed it into history and returned, so the follow-up became \
         dead context instead of a task."
    );
}

/// Pi's inner-loop condition is `hasMoreToolCalls || pendingMessages.length > 0`.
/// With no follow-up queued, exactly one model call happens — the loop must
/// terminate, not spin.
#[tokio::test(flavor = "current_thread")]
async fn no_followup_means_exactly_one_model_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let hooks = Arc::new(Queue {
        steering: vec![],
        followup: vec![],
        steering_polls: AtomicUsize::new(0),
        followup_polls: AtomicUsize::new(0),
    });
    let mut agent = Agent::with_hooks(
        Arc::new(CountingProvider {
            calls: calls.clone(),
        }),
        ToolRegistry::new(),
        RunConfig::new("test-model"),
        hooks,
    );

    drain(&mut agent, "hello").await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "loop must terminate cleanly"
    );
}

// =====================================================================
// L2 — steering must be polled every turn, unconditionally
// =====================================================================

/// Pi polls steering at `agent-loop.js:160` after **every** turn, including
/// a turn that produced no tool calls.
///
/// nini polls it only inside the tool-execution branch (`agent.rs:1451-1456`),
/// so a user interjecting while the model streams a long text-only answer
/// waits until the *next* run to be heard. Pi runs another turn immediately.
///
/// The provider here emits text only — deliberately the case nini misses.
#[tokio::test(flavor = "current_thread")]
#[ignore = "R1 (unconditional end-of-turn steering poll) not implemented — nini polls steering only inside the tool branch"]
async fn steering_is_polled_on_a_text_only_turn() {
    let calls = Arc::new(AtomicUsize::new(0));
    let hooks = Arc::new(Queue {
        steering: vec![user("actually only touch the auth module")],
        followup: vec![],
        steering_polls: AtomicUsize::new(0),
        followup_polls: AtomicUsize::new(0),
    });
    let queue = hooks.clone();
    let mut agent = Agent::with_hooks(
        Arc::new(CountingProvider {
            calls: calls.clone(),
        }),
        ToolRegistry::new(),
        RunConfig::new("test-model"),
        hooks,
    );

    drain(&mut agent, "fix the login bug").await;

    assert!(
        queue.steering_polls.load(Ordering::SeqCst) >= 1,
        "steering was never polled on a text-only turn; Pi polls it at the end of \
         every turn (agent-loop.js:160) regardless of tool calls"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "an interjection during a text-only answer must still buy the user another turn"
    );
}

// Pi also exposes `steer()` / `followUp()` / `hasQueuedMessages()` and a
// `PendingMessageQueue` with `one-at-a-time` / `all` drain modes
// (`agent.js:51,63,128,173,177,228`). nini has none of them, so neither queue
// kind is reachable from a TUI and fixing the loop alone would still leave
// both dead. That gap is recorded in `docs/plans/v0.10-pi-parity-refactor.md`
// rather than asserted here: a compile-time assertion would stop this whole
// test target — and therefore the whole suite — from building.
