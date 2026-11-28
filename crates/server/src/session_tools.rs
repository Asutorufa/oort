//! Tools that belong to a session rather than to a server: `render_widget` (Intelligent UI)
//! and `load_skill` (progressive disclosure of skills).
//!
//! Both are read-only and their definitions are deterministic, so they freeze into the
//! session's tool catalog like any other tool. Widget payloads are the tool-call arguments,
//! which already live in the append-only history; skill bodies arrive as tool results.

use std::sync::Arc;

use async_trait::async_trait;
use oort_agent::{LocalTool, LocalToolExecutor, Tool, ToolContext, ToolError, ToolOutput};
use oort_llm::ToolDefinition;
use oort_skills::SkillRegistry;
use serde_json::{Value, json};

pub const RENDER_WIDGET: &str = "render_widget";
pub const LOAD_SKILL: &str = "load_skill";

pub const WIDGET_TYPES: [&str; 5] = [
    "action_chips",
    "approval_diff",
    "data_table",
    "interactive_form",
    "metrics_chart",
];

/// Prefix of the user message a widget interaction sends back.
pub const WIDGET_RESPONSE_PREFIX: &str = "[widget_response";

/// Whether a call is an `approval_diff` widget (always needs a human decision).
pub fn is_approval_widget(name: &str, args: &Value) -> bool {
    name == RENDER_WIDGET
        && args.get("widget_type").and_then(Value::as_str) == Some("approval_diff")
}

// ----- render_widget -------------------------------------------------------------------------

pub struct RenderWidget;

impl Tool for RenderWidget {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            RENDER_WIDGET,
            "Show an interactive UI card in the chat instead of (or next to) plain text. \
             Call once per card; once it returns `Displayed ...`, the card is already visible to the user (do not re-call for the same chart). \
             `data` depends on `widget_type`:\n\
             - metrics_chart: {\"kpis\": [{\"label\", \"value\", \"delta\"?}] (max 3), \"series\": [{\"name\": \"Price\", \"points\": [{\"x\": \"11-01\", \"y\": 3950}]}], \"kind\"?: \"area\"|\"line\"} — each element of `points` MUST be an object with `x` (string|number) and numeric `y`, never a flat array\n\
             - data_table: {columns: [{key, label}], rows: [object], row_actions?: [{id, label}]}\n\
             - interactive_form: {fields: [{name, label, kind: \"text\"|\"textarea\"|\"number\"|\"select\"|\"checkbox\", options?: [string], required?, default?}], submit_label?}\n\
             - approval_diff: {summary, diff (unified diff), files?: [string]} — the user approves or rejects; the result tells you which\n\
             - action_chips: {chips: [{label, prompt?, command?: \"compact\"}]}\n\
             Interactive widgets (data_table actions, interactive_form, action_chips) answer later with a user \
             message starting with `[widget_response id=<call id>`; end your turn and wait for it.",
            json!({
                "type": "object",
                "properties": {
                    "widget_type": { "type": "string", "enum": WIDGET_TYPES },
                    "title": { "type": "string" },
                    "data": {
                        "type": "object",
                        "properties": {
                            "kind": { "type": "string", "enum": ["area", "line"] },
                            "kpis": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "value": { "type": "string" },
                                        "delta": { "type": "string" }
                                    },
                                    "required": ["label", "value"]
                                }
                            },
                            "series": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "name": { "type": "string" },
                                        "points": {
                                            "type": "array",
                                            "items": {
                                                "type": "object",
                                                "properties": {
                                                    "x": { "type": "string" },
                                                    "y": { "type": "number" }
                                                },
                                                "required": ["x", "y"]
                                            }
                                        }
                                    },
                                    "required": ["name", "points"]
                                }
                            },
                            "columns": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "key": { "type": "string" },
                                        "label": { "type": "string" }
                                    },
                                    "required": ["key", "label"]
                                }
                            },
                            "rows": {
                                "type": "array",
                                "items": { "type": "object" }
                            },
                            "row_actions": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "id": { "type": "string" },
                                        "label": { "type": "string" }
                                    },
                                    "required": ["id", "label"]
                                }
                            },
                            "fields": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "name": { "type": "string" },
                                        "label": { "type": "string" },
                                        "kind": { "type": "string", "enum": ["text", "textarea", "number", "select", "checkbox"] },
                                        "options": { "type": "array", "items": { "type": "string" } },
                                        "required": { "type": "boolean" },
                                        "placeholder": { "type": "string" }
                                    },
                                    "required": ["name", "label", "kind"]
                                }
                            },
                            "submit_label": { "type": "string" },
                            "summary": { "type": "string" },
                            "diff": { "type": "string" },
                            "files": { "type": "array", "items": { "type": "string" } },
                            "chips": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "prompt": { "type": "string" },
                                        "command": { "type": "string" }
                                    },
                                    "required": ["label"]
                                }
                            }
                        }
                    }
                },
                "required": ["widget_type", "data"]
            }),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

fn arr<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    v.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("`data.{key}` must be an array"))
}

fn each_has(items: &[Value], what: &str, keys: &[&str]) -> Result<(), String> {
    for (i, it) in items.iter().enumerate() {
        for k in keys {
            if it.get(*k).is_none() {
                return Err(format!("{what}[{i}] is missing `{k}`"));
            }
        }
    }
    Ok(())
}

/// Validates a widget so the model gets a precise error to fix instead of a broken card.
pub fn validate_widget(args: &Value) -> Result<(&str, &Value), String> {
    let kind = args
        .get("widget_type")
        .and_then(Value::as_str)
        .ok_or("`widget_type` is required")?;
    let data = args
        .get("data")
        .filter(|d| d.is_object())
        .ok_or("`data` must be an object")?;
    match kind {
        "metrics_chart" => {
            let kpis = data.get("kpis").and_then(Value::as_array);
            if kpis.is_some_and(|k| k.len() > 3) {
                return Err("at most 3 `kpis`".into());
            }
            if let Some(k) = kpis {
                each_has(k, "kpis", &["label", "value"])?;
            }
            let series = arr(data, "series")?;
            each_has(series, "series", &["name", "points"])?;
            for (si, s) in series.iter().enumerate() {
                let pts = s.get("points").and_then(Value::as_array).ok_or_else(|| {
                    format!("series[{si}].points must be an array of {{\"x\", \"y\"}} objects")
                })?;
                for (pi, p) in pts.iter().enumerate() {
                    let ok = p.is_object()
                        && p.get("x").is_some()
                        && p.get("y").and_then(Value::as_f64).is_some();
                    if !ok {
                        return Err(format!(
                            "series[{si}].points[{pi}] must be an object {{\"x\": <string|number>, \"y\": <number>}}"
                        ));
                    }
                }
            }
        }
        "data_table" => {
            each_has(arr(data, "columns")?, "columns", &["key", "label"])?;
            let rows = arr(data, "rows")?;
            for (i, r) in rows.iter().enumerate() {
                if !r.is_object() {
                    return Err(format!(
                        "rows[{i}] must be an object keyed by column `key` (not an array)"
                    ));
                }
            }
            if let Some(a) = data.get("row_actions").and_then(Value::as_array) {
                each_has(a, "row_actions", &["id", "label"])?;
            }
        }
        "interactive_form" => {
            let fields = arr(data, "fields")?;
            if fields.is_empty() {
                return Err("`data.fields` must not be empty".into());
            }
            each_has(fields, "fields", &["name", "label", "kind"])?;
            for (i, f) in fields.iter().enumerate() {
                let fkind = f.get("kind").and_then(Value::as_str).unwrap_or("");
                if !matches!(
                    fkind,
                    "text" | "textarea" | "number" | "select" | "checkbox"
                ) {
                    return Err(format!(
                        "fields[{i}].kind `{fkind}` must be one of text, textarea, number, select, checkbox"
                    ));
                }
                if fkind == "select" {
                    let opts = f.get("options").and_then(Value::as_array);
                    if opts.is_none_or(|o| o.is_empty() || !o.iter().all(Value::is_string)) {
                        return Err(format!(
                            "fields[{i}] with kind `select` requires a non-empty string array `options`"
                        ));
                    }
                }
            }
        }
        "approval_diff" => {
            for k in ["summary", "diff"] {
                if !data.get(k).is_some_and(Value::is_string) {
                    return Err(format!("`data.{k}` must be a string"));
                }
            }
            if let Some(files) = data.get("files")
                && !files
                    .as_array()
                    .is_some_and(|a| a.iter().all(Value::is_string))
            {
                return Err("`data.files` must be an array of strings".into());
            }
        }
        "action_chips" => {
            let chips = arr(data, "chips")?;
            if chips.is_empty() {
                return Err("`data.chips` must not be empty".into());
            }
            each_has(chips, "chips", &["label"])?;
        }
        other => {
            return Err(format!(
                "unknown widget_type `{other}` (use one of {})",
                WIDGET_TYPES.join(", ")
            ));
        }
    }
    Ok((kind, data))
}

#[async_trait]
impl LocalTool for RenderWidget {
    async fn run(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let (kind, data) = validate_widget(&args).map_err(ToolError::InvalidArguments)?;
        let id = &ctx.call_id;
        let has_row_actions = data
            .get("row_actions")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty());
        Ok(ToolOutput::ok(match kind {
            // Reached only after the human approved (see `is_approval_widget`).
            "approval_diff" => format!("The user approved the change (widget {id}). Proceed."),
            "metrics_chart" => format!("Displayed metrics_chart widget {id}."),
            "data_table" if !has_row_actions => format!("Displayed data_table widget {id}."),
            "action_chips" => format!(
                "Displayed action_chips widget {id}. Clicking a chip sends its prompt as the next user message."
            ),
            _ => format!(
                "Displayed {kind} widget {id}. The user's choice will arrive as a message starting \
                 with `[widget_response id={id}`; end your turn now and wait for it."
            ),
        }))
    }
}

// ----- load_skill ----------------------------------------------------------------------------

/// Loads one of the session's skills. The `name` enum is the session's skill set, sorted, so
/// the definition is as stable as the frozen index.
pub struct LoadSkill {
    names: Vec<String>,
    registry: Arc<SkillRegistry>,
}

impl LoadSkill {
    pub fn new(mut names: Vec<String>, registry: Arc<SkillRegistry>) -> Self {
        names.sort();
        names.dedup();
        Self { names, registry }
    }
}

impl Tool for LoadSkill {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            LOAD_SKILL,
            "Load the full instructions of a skill listed in <available_skills>. Call it when a skill is relevant before acting.",
            json!({
                "type": "object",
                "properties": { "name": { "type": "string", "enum": self.names } },
                "required": ["name"]
            }),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[async_trait]
impl LocalTool for LoadSkill {
    async fn run(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("missing string argument `name`".into()))?;
        if !self.names.iter().any(|n| n == name) {
            return Err(ToolError::InvalidArguments(format!(
                "`{name}` is not one of this session's skills: {}",
                self.names.join(", ")
            )));
        }
        let skill = self
            .registry
            .by_name(name)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
            .ok_or_else(|| ToolError::Failed(format!("skill `{name}` no longer exists")))?;
        Ok(ToolOutput::ok(skill.render_body()))
    }
}

/// The session-scoped tools for a session.
pub fn executor(
    widgets: bool,
    skills: Vec<String>,
    registry: Arc<SkillRegistry>,
) -> LocalToolExecutor {
    let mut ex = LocalToolExecutor::new();
    if widgets {
        ex = ex.with_tool(Arc::new(RenderWidget));
    }
    if !skills.is_empty() {
        ex = ex.with_tool(Arc::new(LoadSkill::new(skills, registry)));
    }
    ex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_validation() {
        let ok = [
            json!({"widget_type":"metrics_chart","data":{"kpis":[{"label":"a","value":1}],"series":[{"name":"s","points":[]}]}}),
            json!({"widget_type":"data_table","data":{"columns":[{"key":"k","label":"K"}],"rows":[{"k":1}],"row_actions":[{"id":"x","label":"X"}]}}),
            json!({"widget_type":"interactive_form","data":{"fields":[{"name":"n","label":"N","kind":"text"}]}}),
            json!({"widget_type":"approval_diff","data":{"summary":"s","diff":"-a\n+b"}}),
            json!({"widget_type":"action_chips","data":{"chips":[{"label":"Go","prompt":"go"}]}}),
        ];
        for w in &ok {
            assert!(validate_widget(w).is_ok(), "{w}");
        }
        let bad = [
            (
                json!({"widget_type":"nope","data":{}}),
                "unknown widget_type",
            ),
            (
                json!({"widget_type":"metrics_chart","data":{"kpis":[{},{},{},{}],"series":[]}}),
                "at most 3",
            ),
            (
                json!({"widget_type":"data_table","data":{"columns":[{"key":"k"}],"rows":[]}}),
                "missing `label`",
            ),
            (
                json!({"widget_type":"interactive_form","data":{"fields":[]}}),
                "must not be empty",
            ),
            (
                json!({"widget_type":"metrics_chart","data":{"series":[{"name":"s","points":[11,3950]}]}}),
                "must be an object",
            ),
            (
                json!({"widget_type":"data_table","data":{"columns":[{"key":"k","label":"K"}],"rows":[["v"]]}}),
                "must be an object keyed by column",
            ),
            (
                json!({"widget_type":"interactive_form","data":{"fields":[{"name":"n","label":"N","kind":"select"}]}}),
                "requires a non-empty string array `options`",
            ),
            (
                json!({"widget_type":"approval_diff","data":{"summary":"s"}}),
                "data.diff",
            ),
            (
                json!({"widget_type":"action_chips"}),
                "`data` must be an object",
            ),
        ];
        for (w, msg) in &bad {
            let e = validate_widget(w).unwrap_err();
            assert!(e.contains(msg), "{w}: {e}");
        }
        assert!(is_approval_widget(RENDER_WIDGET, &ok[3]));
        assert!(!is_approval_widget(RENDER_WIDGET, &ok[0]));
    }
}
