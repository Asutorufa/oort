//! Per-session event hub: sequence numbers, a replay buffer for reconnecting clients and the
//! in-progress ("live") part of the current turn.
//!
//! Snapshots (`GET /sessions/:id`) only contain committed steps. Streaming text, thinking and
//! running tools exist only as events, so a client that connects mid-turn needs [`LiveTurn`]
//! to show them, and a client that reconnects needs the events it missed. Both are taken under
//! the same lock as the broadcast subscription, so nothing falls between them.

use std::collections::VecDeque;
use std::sync::Mutex;

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
    pub running_tools: Vec<ToolCall>,
}

impl LiveTurn {
    fn apply(&mut self, env: &Envelope) {
        match env {
            Envelope::Agent { event, .. } => match event {
                AgentEvent::TurnStarted { turn } => {
                    *self = Self {
                        turn: Some(*turn),
                        ..Self::default()
                    }
                }
                AgentEvent::TextDelta { delta } => self.text.push_str(delta),
                AgentEvent::ThinkingDelta { delta } => self.thinking.push_str(delta),
                // Committed: the snapshot has it now.
                AgentEvent::AssistantMessage { .. } | AgentEvent::Interrupted { .. } => {
                    self.text.clear();
                    self.thinking.clear();
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
            live: st.live.clone(),
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
            event: AgentEvent::TextDelta { delta: text.into() },
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
