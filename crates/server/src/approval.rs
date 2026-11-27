//! Approval policy that can be switched while a session runs.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use oort_agent::{AgentState, ApprovalPolicyKind, LoopHook, ToolDecision};
use oort_llm::ToolCall;

/// Shared, live-updatable policy of one session.
#[derive(Debug, Default)]
pub struct ApprovalHandle(RwLock<ApprovalPolicyKind>);

impl ApprovalHandle {
    pub fn new(policy: ApprovalPolicyKind) -> Arc<Self> {
        Arc::new(Self(RwLock::new(policy)))
    }

    pub fn get(&self) -> ApprovalPolicyKind {
        *self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set(&self, policy: ApprovalPolicyKind) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = policy;
    }
}

/// Decides each tool call from the handle's current policy. A change applies to the next tool
/// batch; calls already waiting for approval keep waiting (nothing is approved implicitly).
pub struct DynamicApproval(pub Arc<ApprovalHandle>);

#[async_trait]
impl LoopHook for DynamicApproval {
    async fn before_tool(&self, _s: &AgentState, call: &ToolCall, read_only: bool) -> ToolDecision {
        match self.0.get() {
            ApprovalPolicyKind::AutoAll => ToolDecision::Approve,
            ApprovalPolicyKind::AutoReadonly if read_only => ToolDecision::Approve,
            ApprovalPolicyKind::AutoReadonly => ToolDecision::NeedHumanApproval {
                reason: format!("tool `{}` has side effects", call.name),
            },
            ApprovalPolicyKind::AlwaysAsk => ToolDecision::NeedHumanApproval {
                reason: format!("approval policy is always_ask (`{}`)", call.name),
            },
        }
    }
}
