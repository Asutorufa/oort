//! Per-session event hub: sequence numbers, a replay buffer for reconnecting clients and the
//! in-progress ("live") part of the current turn.
//!
//! Snapshots (`GET /sessions/:id`) only contain committed steps. Streaming text, thinking and
//! running tools exist only as events, so a client that connects mid-turn needs [`LiveTurn`]
//! to show them, and a client that reconnects needs the events it missed. Both are taken under
//! the same lock as the broadcast subscription, so nothing falls between them.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

use oort_agent::AgentEvent;
use oort_llm::ToolCall;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::api::Envelope;

/// Envelopes kept for `Last-Event-ID` replay.
pub const REPLAY_CAPACITY: usize = 2048;

/// Uncommitted output of the turn in progress.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LiveTurn {
    pub turn: Option<u32>,
    pub text: String,
    pub thinking: String,
    pub text_block: Option<String>,
    pub thinking_block: Option<String>,
    /// Set once reasoning has ended.
    pub thinking_ms: Option<u64>,
    /// Time spent reasoning so far, filled in when a client subscribes mid-thought.
    pub thinking_elapsed_ms: Option<u64>,
    #[serde(skip)]
    thinking_since: Option<Instant>,
    pub running_tools: Vec<ToolCall>,
}

impl LiveTurn {
    fn clear_step(&mut self) {
        *self = Self {
            turn: self.turn,
            running_tools: std::mem::take(&mut self.running_tools),
            ..Self::default()
        };
    }

    fn snapshot(&self) -> Self {
        let mut live = self.clone();
        live.thinking_elapsed_ms = live
            .thinking_since
            .map(|since| since.elapsed().as_millis() as u64);
        live
    }

    fn apply(&mut self, env: &Envelope) {
        match env {
            Envelope::Agent { event, .. } => match event {
                AgentEvent::TurnStarted { turn } => {
                    *self = Self {
                        turn: Some(*turn),
                        ..Self::default()
                    }
                }
                AgentEvent::TextDelta {
                    block_id, delta, ..
                } => {
                    self.text_block = Some(block_id.clone());
                    self.text.push_str(delta);
                }
                AgentEvent::ThinkingDelta {
                    block_id, delta, ..
                } => {
                    self.thinking_block = Some(block_id.clone());
                    self.thinking_ms = None;
                    self.thinking_since.get_or_insert_with(Instant::now);
                    self.thinking.push_str(delta);
                }
                AgentEvent::ThinkingEnd { duration_ms, .. } => {
                    self.thinking_ms = Some(*duration_ms);
                    self.thinking_since = None;
                }
                // Committed: the snapshot has it now.
                AgentEvent::AssistantMessage { .. } | AgentEvent::Interrupted { .. } => {
                    self.clear_step();
                }
                AgentEvent::ToolStarted { call } => self.running_tools.push(call.clone()),
                AgentEvent::ToolFinished { result } => {
                    self.running_tools.retain(|c| c.id != result.tool_call_id)
                }
                AgentEvent::Completed | AgentEvent::Halted { .. } => *self = Self::default(),
                _ => {}
            },
            Envelope::LoopExited { .. } | Envelope::Deleted { .. } => *self = Self::default(),
            _ => {}
        }
    }
}

struct State {
    seq: u64,
    buffer: VecDeque<Envelope>,
    live: LiveTurn,
}

pub struct Hub {
    tx: broadcast::Sender<Envelope>,
    state: Mutex<State>,
}

/// What a new subscriber gets.
pub struct Subscription {
    /// Last sequence number published so far.
    pub seq: u64,
    /// Missed envelopes (`after` was in the buffer), or `None` when the client must resync.
    pub replay: Option<Vec<Envelope>>,
    pub live: LiveTurn,
    pub rx: broadcast::Receiver<Envelope>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(1024).0,
            state: Mutex::new(State {
                seq: 0,
                buffer: VecDeque::new(),
                live: LiveTurn::default(),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Assigns the next sequence number, records and broadcasts the envelope.
    pub fn publish(&self, make: impl FnOnce(u64) -> Envelope) {
        let mut st = self.lock();
        st.seq += 1;
        let env = make(st.seq);
        st.live.apply(&env);
        if st.buffer.len() == REPLAY_CAPACITY {
            st.buffer.pop_front();
        }
        st.buffer.push_back(env.clone());
        // Sent under the lock so subscribers see the same order as the buffer.
        let _ = self.tx.send(env);
    }

    /// Subscribes, replaying what came after `after` when it is still buffered.
    pub fn subscribe(&self, after: Option<u64>) -> Subscription {
        let st = self.lock();
        let rx = self.tx.subscribe();
        let replay = after.and_then(|after| {
            if after > st.seq {
                return None; // from another hub generation
            }
            let oldest = st
                .buffer
                .front()
                .and_then(Envelope::seq)
                .unwrap_or(st.seq + 1);
            if after + 1 < oldest {
                return None; // fell out of the buffer
            }
            Some(
                st.buffer
                    .iter()
                    .filter(|e| e.seq().is_some_and(|s| s > after))
                    .cloned()
                    .collect(),
            )
        });
        Subscription {
            seq: st.seq,
            replay,
            live: st.live.snapshot(),
            rx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(text: &str) -> impl FnOnce(u64) -> Envelope + '_ {
        move |seq| Envelope::Agent {
            seq,
            event: AgentEvent::TextDelta {
                step_index: 1,
                block_id: "1:text".into(),
                delta: text.into(),
            },
        }
    }

    #[test]
    fn live_turn_and_replay() {
        let hub = Hub::new();
        hub.publish(|seq| Envelope::Agent {
            seq,
            event: AgentEvent::TurnStarted { turn: 1 },
        });
        hub.publish(delta("Hel"));
        hub.publish(delta("lo"));

        let fresh = hub.subscribe(None);
        assert_eq!(fresh.seq, 3);
        assert!(fresh.replay.is_none());
        assert_eq!(fresh.live.text, "Hello");
        assert_eq!(fresh.live.turn, Some(1));

        let resumed = hub.subscribe(Some(1));
        let seqs: Vec<u64> = resumed
            .replay
            .unwrap()
            .iter()
            .filter_map(Envelope::seq)
            .collect();
        assert_eq!(seqs, [2, 3]);
        assert_eq!(hub.subscribe(Some(3)).replay.unwrap().len(), 0);
        // A sequence number from before a restart cannot be replayed.
        assert!(hub.subscribe(Some(99)).replay.is_none());
    }

    #[test]
    fn live_thinking_is_visible_to_a_late_subscriber() {
        let hub = Hub::new();
        let agent = |event| move |seq| Envelope::Agent { seq, event };
        hub.publish(agent(AgentEvent::ThinkingDelta {
            step_index: 2,
            block_id: "3:thinking".into(),
            delta: "hmm".into(),
        }));
        let live = hub.subscribe(None).live;
        assert_eq!(live.thinking_block.as_deref(), Some("3:thinking"));
        assert!(live.thinking_elapsed_ms.is_some());
        assert_eq!(live.thinking_ms, None);

        hub.publish(agent(AgentEvent::ThinkingEnd {
            block_id: "3:thinking".into(),
            duration_ms: 40,
        }));
        let live = hub.subscribe(None).live;
        assert_eq!(live.thinking_ms, Some(40));
        assert_eq!(live.thinking_elapsed_ms, None);
    }

    #[test]
    fn replay_gap_forces_resync() {
        let hub = Hub::new();
        for _ in 0..REPLAY_CAPACITY + 5 {
            hub.publish(delta("x"));
        }
        assert!(hub.subscribe(Some(1)).replay.is_none());
        let ok = hub.subscribe(Some(10)).replay.unwrap();
        assert_eq!(ok.len(), REPLAY_CAPACITY + 5 - 10);
    }
}
