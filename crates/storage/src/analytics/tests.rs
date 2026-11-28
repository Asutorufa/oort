use oort_agent::{AgentState, NewSession, SessionStore};
use oort_llm::Message;

use super::*;

fn ms(rfc: &str) -> i64 {
    DateTime::parse_from_rfc3339(rfc)
        .unwrap()
        .timestamp_millis()
}

async fn session(store: &SqliteSessionStore, title: &str, parent: Option<&str>) -> String {
    let state = AgentState::with_tools(
        "sys",
        vec![Message::user("hi")],
        &oort_agent::FrozenToolset::freeze(&oort_agent::LocalToolExecutor::new()).unwrap(),
    );
    let mut meta = NewSession {
        title: title.into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-5".into(),
        ..Default::default()
    };
    meta.lineage.parent_session_id = parent.map(str::to_string);
    store.create_session(meta, &state).await.unwrap()
}

fn call(session_id: Option<&str>, at: &str) -> LlmUsageRecord {
    LlmUsageRecord {
        session_id: session_id.map(str::to_string),
        provider_id: None,
        vendor: "anthropic".into(),
        model: "claude-sonnet-4-5".into(),
        purpose: CallPurpose::Turn,
        tokens: TokenCounts {
            input: 100_000,
            cached_input: 900_000,
            cache_write: 0,
            output: 10_000,
            thinking: 0,
        },
        ttft_ms: Some(400),
        duration_ms: Some(2000),
        at_ms: ms(at),
    }
}

fn query(now: &str, tz: i32) -> ReportQuery {
    ReportQuery {
        range: ReportRange::Days7,
        tz_offset_minutes: tz,
        calendar_month: None,
        now_ms: ms(now),
    }
}

/// 0.3 (input) + 0.27 (cached) + 0.15 (output).
const CALL_COST: f64 = 0.72;

#[tokio::test]
async fn ledger_survives_session_deletion() {
    let store = SqliteSessionStore::in_memory().await.unwrap();
    let chat = session(&store, "", None).await;
    let child = session(&store, "explorer", Some(&chat)).await;
    let at = "2026-11-10T03:00:00Z";
    store.record_llm_call(&call(Some(&chat), at)).await.unwrap();
    store
        .record_llm_call(&call(Some(&child), at))
        .await
        .unwrap();
    store
        .record_tool_call(&ToolUsageRecord {
            session_id: Some(chat.clone()),
            tool_name: "read_file".into(),
            tool_kind: ToolKind::Builtin,
            duration_ms: Some(5),
            is_error: false,
            at_ms: ms(at),
        })
        .await
        .unwrap();
    // Titles often arrive after the first rows were written.
    store.set_title(&chat, "Named later").await.unwrap();

    let before = store.usage_report(query(at, 0)).await.unwrap();
    // The subagent's cost is booked on the chat that spawned it.
    assert_eq!(before.top_sessions.len(), 1);
    assert_eq!(
        before.top_sessions[0].session_id.as_deref(),
        Some(chat.as_str())
    );
    assert!((before.summary.cost_usd - 2.0 * CALL_COST).abs() < 1e-9);
    let subagents: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM usage_ledger WHERE is_subagent = 1")
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(subagents, 1);

    store.delete_session(&chat).await.unwrap();

    let after = store.usage_report(query(at, 0)).await.unwrap();
    assert_eq!(after.total_records, 2);
    assert!((after.summary.cost_usd - 2.0 * CALL_COST).abs() < 1e-9);
    assert_eq!(after.top_tools.len(), 1);
    let top = &after.top_sessions[0];
    assert_eq!((top.session_id.as_deref(), top.exists), (None, false));
    assert_eq!(top.title, "Named later");
    // A request finishing after the deletion is still booked.
    store.record_llm_call(&call(Some(&chat), at)).await.unwrap();
    assert_eq!(
        store
            .usage_report(query(at, 0))
            .await
            .unwrap()
            .total_records,
        3
    );
}

#[tokio::test]
async fn days_are_cut_at_the_client_midnight() {
    let store = SqliteSessionStore::in_memory().await.unwrap();
    // 23:30 UTC on the 31st is already November 1st in Japan.
    store
        .record_llm_call(&call(None, "2026-10-31T23:30:00Z"))
        .await
        .unwrap();
    store
        .record_llm_call(&call(None, "2026-10-31T14:59:00Z"))
        .await
        .unwrap();
    let now = "2026-11-02T00:00:00Z";

    let utc = store.usage_report(query(now, 0)).await.unwrap();
    let day = |r: &UsageReport, date: &str| {
        r.daily_heatmap
            .iter()
            .find(|d| d.date == date)
            .unwrap()
            .turns
    };
    assert_eq!(day(&utc, "2026-10-31"), 2);
    assert_eq!(utc.summary.current_month_cost_usd, 0.0);

    let jst = store.usage_report(query(now, 540)).await.unwrap();
    assert_eq!(day(&jst, "2026-10-31"), 1);
    assert_eq!(day(&jst, "2026-11-01"), 1);
    assert!((jst.summary.current_month_cost_usd - CALL_COST).abs() < 1e-9);
    // Day 2 of 30 → 15 times the spend so far.
    assert!((jst.summary.projected_month_cost_usd - CALL_COST * 15.0).abs() < 1e-9);
    assert!((store.month_cost(540, ms(now)).await.unwrap() - CALL_COST).abs() < 1e-9);
    assert_eq!(jst.month_calendar.len(), 30);
    assert_eq!(jst.daily_heatmap.len(), 140);
    assert_eq!(jst.trend_14d.len(), 14);
    assert_eq!(jst.summary.total_days, 7);
    assert_eq!(jst.summary.active_days, 2);
    assert!((jst.summary.cache_hit_rate_pct - 90.0).abs() < 1e-9);

    let october = store
        .usage_report(ReportQuery {
            calendar_month: Some((2026, 10)),
            ..query(now, 540)
        })
        .await
        .unwrap();
    assert_eq!(october.calendar_month, "2026-10");
    assert_eq!(october.month_calendar.len(), 31);
}

#[test]
fn budget_status_boundaries() {
    let settings = BudgetSettings {
        monthly_budget_usd: Some(10.0),
        warning_threshold_pct: 80,
        scoped_budgets: None,
    };
    let state = |cost| budget_status(&settings, cost).status;
    assert_eq!(state(0.0), BudgetState::Ok);
    assert_eq!(state(7.99), BudgetState::Ok);
    assert_eq!(state(8.0), BudgetState::Warning);
    assert_eq!(state(9.99), BudgetState::Warning);
    assert_eq!(state(10.0), BudgetState::Exceeded);
    let unset = budget_status(&BudgetSettings::default(), 100.0);
    assert_eq!((unset.status, unset.usage_pct), (BudgetState::Unset, 0.0));
}

#[tokio::test]
async fn price_changes_reprice_recorded_calls_on_request() {
    let store = SqliteSessionStore::in_memory().await.unwrap();
    let at = "2026-11-10T03:00:00Z";
    let mut unknown = call(None, at);
    unknown.vendor = "custom".into();
    unknown.model = "house-model".into();
    store.record_llm_call(&unknown).await.unwrap();
    let report = store.usage_report(query(at, 0)).await.unwrap();
    assert!(!report.by_model[0].has_pricing);
    assert_eq!(report.summary.cost_usd, 0.0);

    let price = PricingOverride {
        model_pattern: "house-model".into(),
        input_per_m: Some(2.0),
        cached_input_per_m: Some(0.2),
        cache_write_per_m: None,
        output_per_m: Some(10.0),
        updated_at: None,
    };
    store.put_pricing(&price, false).await.unwrap();
    assert_eq!(
        store
            .usage_report(query(at, 0))
            .await
            .unwrap()
            .summary
            .cost_usd,
        0.0
    );
    store.put_pricing(&price, true).await.unwrap();
    let report = store.usage_report(query(at, 0)).await.unwrap();
    assert!(report.by_model[0].has_pricing);
    assert!((report.summary.cost_usd - 0.48).abs() < 1e-9);
    assert!((report.summary.cache_saved_usd - 1.62).abs() < 1e-9);

    store.delete_pricing("house-model", true).await.unwrap();
    assert_eq!(
        store
            .usage_report(query(at, 0))
            .await
            .unwrap()
            .summary
            .cost_usd,
        0.0
    );
    assert!(store.delete_pricing("house-model", true).await.is_err());
}

#[tokio::test]
async fn older_history_is_backfilled_once() {
    let store = SqliteSessionStore::in_memory().await.unwrap();
    let chat = session(&store, "Old chat", None).await;
    let usage = Usage {
        input_tokens: 1000,
        output_tokens: 100,
        cache_read_tokens: Some(400),
        ..Default::default()
    };
    store
        .append_message(&chat, 1, &Message::assistant("yo"), Some(&usage))
        .await
        .unwrap();
    // As if the database predated the ledger.
    sqlx::query("DELETE FROM analytics_meta")
        .execute(store.pool())
        .await
        .unwrap();
    backfill(&store).await.unwrap();
    backfill(&store).await.unwrap();
    let rows = sqlx::query("SELECT * FROM usage_ledger")
        .fetch_all(store.pool())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    // Anthropic reports input without the cached part.
    assert_eq!(tokens_of(&rows[0]).input, 1000);
    assert_eq!(tokens_of(&rows[0]).cached_input, 400);
    assert!(rows[0].get::<f64, _>("estimated_cost_usd") > 0.0);
}
