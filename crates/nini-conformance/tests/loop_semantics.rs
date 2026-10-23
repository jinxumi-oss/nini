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
use std::sync::{Arc, Mutex};

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

/// Models Pi's `PendingMessageQueue`: a queue is **drained**, not re-read.
///
/// The first poll returns the queued items; every later poll returns nothing.
/// That is what `agent.js:63-71` does — `drain()` empties the queue — and it
/// is the difference between a queue and a generator.
///
/// An earlier draft returned the same message on every poll, i.e. modelled a
/// hook that produces fresh work forever. That is a different thing, and the
/// run loop behaved correctly: it kept running until
/// `max_follow_up_rounds` stopped it, which is exactly the guard a
/// pathological producer needs. `a_pathological_producer_is_bounded` covers
/// that case explicitly.
struct Queue {
    steering: Mutex<Option<Vec<nini_core::provider::Message>>>,
    followup: Mutex<Option<Vec<nini_core::provider::Message>>>,
    /// Release the steering item only once this many polls have happened.
    ///
    /// Pi polls steering twice per run: once at `agent-loop.js:83`, before
    /// the outer loop, and once at `:160` after every turn. An interjection
    /// typed by a human arrives *during* the run, so it must be picked up by
    /// the `:160` poll — the `:83` poll already happened. Arming the item
    /// from poll 0 models "the user typed before nini started", which the
    /// first turn already satisfies, and would make this test assert 2 calls
    /// for a scenario Pi also answers in 1.
    steering_after_poll: usize,
    /// When true the follow-up hook ignores `take()` and yields a fresh item
    /// on every poll — a producer with no natural stopping point.
    followup_forever: bool,
    steering_polls: AtomicUsize,
    followup_polls: AtomicUsize,
}

impl Queue {
    /// `None` arms a never-ending producer (see the note above).
    fn once(
        steering: Vec<nini_core::provider::Message>,
        followup: Vec<nini_core::provider::Message>,
    ) -> Self {
        Self {
            steering: Mutex::new(if steering.is_empty() {
                None
            } else {
                Some(steering)
            }),
            followup: Mutex::new(if followup.is_empty() {
                None
            } else {
                Some(followup)
            }),
            steering_after_poll: 0,
            followup_forever: false,
            steering_polls: AtomicUsize::new(0),
            followup_polls: AtomicUsize::new(0),
        }
    }

    /// A steering item that only becomes visible to the poll at
    /// `agent-loop.ts#getSteeringMessages`, i.e. the user interjects mid-run.
    fn steering_mid_run(item: nini_core::provider::Message) -> Self {
        let mut q = Self::once(vec![], vec![]);
        *q.steering.lock().unwrap() = Some(vec![item]);
        q.steering_after_poll = 1;
        q
    }

    /// A follow-up hook that yields new work on *every* poll — a producer
    /// with no natural stopping point. The run loop must still terminate;
    /// `max_follow_up_rounds` is what guarantees it.
    fn forever_followup() -> Self {
        Self {
            steering: Mutex::new(None),
            followup: Mutex::new(None),
            steering_after_poll: 0,
            followup_forever: true,
            steering_polls: AtomicUsize::new(0),
            followup_polls: AtomicUsize::new(0),
        }
    }
}

impl AgentLoopHooks for Queue {
    fn get_steering_messages(
        &self,
        _recent: &[nini_core::provider::Message],
    ) -> Vec<nini_core::provider::Message> {
        let n = self.steering_polls.fetch_add(1, Ordering::SeqCst);
        if n < self.steering_after_poll {
            return Vec::new();
        }
        let mut g = self.steering.lock().unwrap_or_else(|e| e.into_inner());
        g.take().unwrap_or_default()
    }
    fn get_followup_messages(
        &self,
        _messages: &[nini_core::provider::Message],
    ) -> Vec<nini_core::provider::Message> {
        self.followup_polls.fetch_add(1, Ordering::SeqCst);
        if self.followup_forever {
            return vec![user("more work")];
        }
        let mut g = self.followup.lock().unwrap_or_else(|e| e.into_inner());
        g.take().unwrap_or_default()
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
/// call**. Pi: `agent-loop.js:161-167` re-seeds `pendingMessages` and
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

async fn followup_triggers_another_model_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let hooks = Arc::new(Queue::once(vec![], vec![user("now write the docs")]));
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
    let hooks = Arc::new(Queue::once(vec![], vec![]));
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

/// Pi polls steering at `agent-loop.ts#getSteeringMessages` after **every** turn, including
/// a turn that produced no tool calls.
///
/// nini polls it only inside the tool-execution branch (`agent.rs:1451-1456`),
/// so a user interjecting while the model streams a long text-only answer
/// waits until the *next* run to be heard. Pi runs another turn immediately.
///
/// The provider here emits text only — deliberately the case nini misses.
#[tokio::test(flavor = "current_thread")]

async fn steering_is_polled_on_a_text_only_turn() {
    let calls = Arc::new(AtomicUsize::new(0));
    let hooks = Arc::new(Queue::steering_mid_run(user(
        "actually only touch the auth module",
    )));
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
         every turn (agent-loop.ts#getSteeringMessages) regardless of tool calls"
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

// =====================================================================
// Termination guarantee
// =====================================================================

/// A follow-up hook that yields work on *every* poll has no natural stopping
/// point. Before v0.10 (R1) the loop could not even reach such a hook — it
/// returned instead of reviving. Now that it can, it needs its own bound:
/// `max_iterations` counts turns, not revivals, so a producer like this would
/// run until the turn cap, which is the wrong cap for the wrong thing.
///
/// This is the test that the first draft of the R1 implementation failed: a
/// stub returning the same follow-up every poll produced 17 model calls
/// (1 + 16 revivals) instead of 2. The implementation was right and the stub
/// was unrealistic — but a user can write exactly that hook, so the guard
/// needs to exist and be asserted.
#[tokio::test(flavor = "current_thread")]
async fn a_pathological_producer_is_bounded() {
    let calls = Arc::new(AtomicUsize::new(0));
    let hooks = Arc::new(Queue::forever_followup());
    let mut agent = Agent::with_hooks(
        Arc::new(CountingProvider {
            calls: calls.clone(),
        }),
        ToolRegistry::new(),
        {
            let mut cfg = RunConfig::new("test-model");
            cfg.max_follow_up_rounds = 3;
            cfg
        },
        hooks,
    );

    drain(&mut agent, "start").await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "1 initial turn + 3 allowed revivals, then the loop must stop"
    );
}
