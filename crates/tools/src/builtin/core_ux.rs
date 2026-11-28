//! `update_plan` (progress checklist) and `ask_user` (structured question).

use std::sync::Arc;

use async_trait::async_trait;
use oort_agent::{
    LocalTool, LocalToolExecutor, PlanStatus, PlanStep, ToolContext, ToolError, ToolOutput,
};
use serde_json::Value;

use super::{BuiltinContext, PlanSink, tool_def};

pub struct UpdatePlan {
    sink: Option<Arc<dyn PlanSink>>,
}

tool_def!(UpdatePlan, "update_plan", true,
"Show the user your plan for a multi-step task and keep it current: send the full list of steps each time (one `in_progress` at a time).",
{
    "type": "object",
    "properties": {
        "steps": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "status": { "type": "string", "enum": ["pending", "in_progress", "done", "skipped"] }
                },
                "required": ["id", "title", "status"]
            }
        }
    },
    "required": ["steps"]
});

pub fn parse_steps(args: &Value) -> Result<Vec<PlanStep>, ToolError> {
    let steps: Vec<PlanStep> =
        serde_json::from_value(args.get("steps").cloned().unwrap_or(Value::Null))
            .map_err(|e| ToolError::InvalidArguments(format!("steps: {e}")))?;
    if steps.is_empty() {
        return Err(ToolError::InvalidArguments(
            "steps must not be empty".into(),
        ));
    }
    Ok(steps)
}

#[async_trait]
impl LocalTool for UpdatePlan {
    async fn run(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let steps = parse_steps(&args)?;
        let done = steps
            .iter()
            .filter(|s| s.status == PlanStatus::Done)
            .count();
        let current = steps
            .iter()
            .find(|s| s.status == PlanStatus::InProgress)
            .map(|s| s.title.clone());
        if let Some(sink) = &self.sink {
            sink.publish(steps.clone());
        }
        Ok(ToolOutput::ok(format!(
            "Plan updated: {done}/{} done{}.",
            steps.len(),
            current.map(|c| format!(", now: {c}")).unwrap_or_default()
        )))
    }
}

/// Never executed: the application's hook turns every call into a question for the user, whose
/// answer becomes the result (`ToolDecision::NeedUserInput`).
pub struct AskUser;

tool_def!(AskUser, super::ASK_USER, true,
"Ask the user a question and wait for the answer (it becomes this tool's result). Use for decisions only the user can make; prefer options when the answers are known.",
{
    "type": "object",
    "properties": {
        "question": { "type": "string" },
        "kind": { "type": "string", "enum": ["single", "multiple", "text"], "description": "single / multiple choice from `options`, or free text" },
        "options": { "type": "array", "items": { "type": "string" } },
        "placeholder": { "type": "string" }
    },
    "required": ["question"]
});

#[async_trait]
impl LocalTool for AskUser {
    async fn run(&self, _args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        Err(ToolError::Failed(
            "ask_user must be answered by the user".into(),
        ))
    }
}

pub(super) fn add(ex: LocalToolExecutor, ctx: &BuiltinContext) -> LocalToolExecutor {
    ex.with_tool(Arc::new(UpdatePlan {
        sink: ctx.plan.clone(),
    }))
    .with_tool(Arc::new(AskUser))
}
