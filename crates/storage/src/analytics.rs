//! Usage ledgers, budget and price settings, and the aggregated usage report.
//!
//! Recording happens beside the conversation (never inside it): nothing here reads or writes
//! messages, the frozen prefix or request parameters.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Days, FixedOffset, NaiveDate, SecondsFormat, Utc};
use oort_agent::{ProviderKind, StoreError};
use oort_llm::Usage;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection};

use crate::pricing::{self, PricingOverride, TokenCounts};
use crate::{SqliteSessionStore, backend, classify, now_ms};

/// Days covered by the heatmap (20 weeks).
const HEATMAP_DAYS: u64 = 140;
const TREND_DAYS: u64 = 14;
const TOP_TOOLS: usize = 8;
const TOP_SESSIONS: usize = 6;
const BACKFILL_KEY: &str = "usage_backfill";

/// Why a model was called.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CallPurpose {
    #[default]
    Turn,
    Compaction,
    Title,
}

impl CallPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Compaction => "compaction",
            Self::Title => "title",
        }
    }
}

/// One completed LLM call, before pricing.
#[derive(Debug, Clone)]
pub struct LlmUsageRecord {
    /// The calling session (a subagent's rows are booked on its top-level chat).
    pub session_id: Option<String>,
    pub provider_id: Option<String>,
    pub vendor: String,
    pub model: String,
    pub purpose: CallPurpose,
    pub tokens: TokenCounts,
    pub ttft_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    /// Unix milliseconds.
    pub at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Builtin,
    /// Hosted by the model provider.
    Server,
    Mcp,
}

impl ToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Server => "server",
            Self::Mcp => "mcp",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolUsageRecord {
    pub session_id: Option<String>,
    pub tool_name: String,
    pub tool_kind: ToolKind,
    pub duration_ms: Option<u64>,
    pub is_error: bool,
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetSettings {
    /// `None`: no limit.
    pub monthly_budget_usd: Option<f64>,
    pub warning_threshold_pct: u32,
    /// Optional per model / provider budgets (stored as given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scoped_budgets: Option<serde_json::Value>,
}

impl Default for BudgetSettings {
    fn default() -> Self {
        Self {
            monthly_budget_usd: None,
            warning_threshold_pct: 80,
            scoped_budgets: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetState {
    Ok,
    Warning,
    Exceeded,
    Unset,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BudgetStatus {
    pub monthly_budget_usd: Option<f64>,
    pub warning_threshold_pct: u32,
    pub usage_pct: f64,
    pub status: BudgetState,
}

/// `warning` from the threshold on, `exceeded` from 100 % on.
pub fn budget_status(settings: &BudgetSettings, month_cost_usd: f64) -> BudgetStatus {
    let limit = settings.monthly_budget_usd.filter(|b| *b > 0.0);
    let usage_pct = limit.map_or(0.0, |b| month_cost_usd / b * 100.0);
    // Sums of floats: 0.1 + 0.7 must still count as having reached 80 % of 1.
    let reached = |pct: f64| usage_pct >= pct - 1e-9;
    let status = match limit {
        None => BudgetState::Unset,
        Some(_) if reached(100.0) => BudgetState::Exceeded,
        Some(_) if reached(f64::from(settings.warning_threshold_pct)) => BudgetState::Warning,
        Some(_) => BudgetState::Ok,
    };
    BudgetStatus {
        monthly_budget_usd: limit,
        warning_threshold_pct: settings.warning_threshold_pct,
        usage_pct,
        status,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportRange {
    Days7,
    Days30,
    /// The current calendar month so far.
    Month,
    Weeks20,
}

impl ReportRange {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "7d" => Self::Days7,
            "30d" => Self::Days30,
            "month" => Self::Month,
            "20w" => Self::Weeks20,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReportQuery {
    pub range: ReportRange,
    /// Minutes east of UTC; days are cut at the client's local midnight.
    pub tz_offset_minutes: i32,
    /// `(year, month)` of the calendar view; the current month when `None`.
    pub calendar_month: Option<(i32, u32)>,
    pub now_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageSummary {
    pub cost_usd: f64,
    pub cache_saved_usd: f64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    /// Cached share of all input.
    pub cache_hit_rate_pct: f64,
    /// Thinking share of all output.
    pub thinking_share_pct: f64,
    pub turns: u64,
    pub avg_ttft_ms: Option<f64>,
    pub active_days: u64,
    pub total_days: u64,
    pub current_month_cost_usd: f64,
    pub projected_month_cost_usd: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DayUsage {
    /// Local date, `YYYY-MM-DD`.
    pub date: String,
    pub cost_usd: f64,
    pub cache_saved_usd: f64,
    pub total_tokens: u64,
    pub cached_tokens: u64,
    /// Uncached input, including cache writes.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub turns: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelUsage {
    pub model: String,
    pub vendor: String,
    pub calls: u64,
    pub input_tokens: u64,
    pub cached_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub cache_hit_rate_pct: f64,
    pub avg_ttft_ms: Option<f64>,
    pub cost_usd: f64,
    pub cache_saved_usd: f64,
    pub cost_share_pct: f64,
    pub has_pricing: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolStats {
    pub tool_name: String,
    pub tool_kind: String,
    pub calls: u64,
    pub avg_duration_ms: Option<f64>,
    pub error_rate_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionUsage {
    /// `None` once the chat was deleted.
    pub session_id: Option<String>,
    pub title: String,
    pub model: String,
    pub turns: u64,
    pub cache_hit_rate_pct: f64,
    pub cost_usd: f64,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageReport {
    pub summary: UsageSummary,
    pub budget: BudgetStatus,
    pub daily_heatmap: Vec<DayUsage>,
    pub month_calendar: Vec<DayUsage>,
    pub trend_14d: Vec<DayUsage>,
    pub by_model: Vec<ModelUsage>,
    pub top_tools: Vec<ToolStats>,
    pub top_sessions: Vec<SessionUsage>,
    /// `YYYY-MM` shown by `month_calendar`.
    pub calendar_month: String,
    /// Ledger rows ever recorded (0: nothing to show yet).
    pub total_records: u64,
}

#[async_trait]
pub trait AnalyticsStore: Send + Sync {
    async fn record_llm_call(&self, call: &LlmUsageRecord) -> Result<(), StoreError>;
    async fn record_tool_call(&self, call: &ToolUsageRecord) -> Result<(), StoreError>;
    async fn usage_report(&self, query: ReportQuery) -> Result<UsageReport, StoreError>;
    async fn budget_settings(&self) -> Result<BudgetSettings, StoreError>;
    async fn set_budget_settings(&self, settings: &BudgetSettings) -> Result<(), StoreError>;
    /// Cost of the current local month so far.
    async fn month_cost(&self, tz_offset_minutes: i32, now_ms: i64) -> Result<f64, StoreError>;
    async fn list_pricing(&self) -> Result<Vec<PricingOverride>, StoreError>;
    /// Adds or replaces an override; `recalculate` re-prices the recorded calls.
    async fn put_pricing(
        &self,
        pricing: &PricingOverride,
        recalculate: bool,
    ) -> Result<(), StoreError>;
    async fn delete_pricing(&self, pattern: &str, recalculate: bool) -> Result<(), StoreError>;
}

// ----- time ----------------------------------------------------------------------------------

fn rfc3339(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn offset(minutes: i32) -> FixedOffset {
    FixedOffset::east_opt(minutes.clamp(-18 * 60, 18 * 60) * 60).expect("within a day")
}

fn local_date(ms: i64, tz: FixedOffset) -> NaiveDate {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .with_timezone(&tz)
        .date_naive()
}

/// UTC timestamp string of the local midnight that starts `date`.
fn day_start(date: NaiveDate, tz: FixedOffset) -> String {
    let local = date.and_hms_opt(0, 0, 0).expect("midnight exists");
    let utc_ms = local.and_utc().timestamp_millis() - i64::from(tz.local_minus_utc()) * 1000;
    rfc3339(utc_ms)
}

fn month_bounds(year: i32, month: u32) -> Option<(NaiveDate, NaiveDate)> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let next = first.checked_add_months(chrono::Months::new(1))?;
    Some((first, next.pred_opt()?))
}

fn days_before(date: NaiveDate, n: u64) -> NaiveDate {
    date.checked_sub_days(Days::new(n)).unwrap_or(date)
}

// ----- recording -----------------------------------------------------------------------------

/// The top-level chat of a session (itself unless it is a subagent) and that chat's title.
/// `None` when the session no longer exists.
async fn root_session(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<(String, String, bool)>, StoreError> {
    let row = sqlx::query(
        "WITH RECURSIVE up(id, parent, title, n) AS ( \
            SELECT id, parent_session_id, title, 0 FROM sessions WHERE id = ? \
            UNION ALL \
            SELECT s.id, s.parent_session_id, s.title, up.n + 1 FROM sessions s \
            JOIN up ON s.id = up.parent \
         ) SELECT id, title, n FROM up ORDER BY n DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(backend)?;
    Ok(row.map(|r| (r.get("id"), r.get("title"), r.get::<i64, _>("n") > 0)))
}

async fn load_overrides(conn: &mut SqliteConnection) -> Result<Vec<PricingOverride>, StoreError> {
    let rows = sqlx::query("SELECT * FROM model_pricing_overrides ORDER BY model_pattern")
        .fetch_all(conn)
        .await
        .map_err(backend)?;
    Ok(rows
        .iter()
        .map(|r| PricingOverride {
            model_pattern: r.get("model_pattern"),
            input_per_m: r.get("input_per_m"),
            cached_input_per_m: r.get("cached_input_per_m"),
            cache_write_per_m: r.get("cache_write_per_m"),
            output_per_m: r.get("output_per_m"),
            updated_at: r.get("updated_at"),
        })
        .collect())
}

async fn insert_usage(
    conn: &mut SqliteConnection,
    overrides: &[PricingOverride],
    call: &LlmUsageRecord,
) -> Result<(), StoreError> {
    let root = match &call.session_id {
        Some(id) => root_session(conn, id).await?,
        None => None,
    };
    let (session_id, title, is_subagent) = match root {
        Some((id, title, sub)) => (Some(id), Some(title).filter(|t| !t.is_empty()), sub),
        None => (None, None, false),
    };
    let (cost, saved) = pricing::resolve(overrides, &call.vendor, &call.model)
        .map_or((0.0, 0.0), |p| pricing::cost(p, call.tokens));
    let t = call.tokens;
    sqlx::query(
        "INSERT INTO usage_ledger (id, session_id, origin_session_id, session_title_snapshot, \
         provider_id, vendor, model, purpose, is_subagent, input_tokens, cached_input_tokens, \
         cache_write_tokens, output_tokens, thinking_tokens, ttft_ms, duration_ms, \
         estimated_cost_usd, cache_saved_usd, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::now_v7().to_string())
    .bind(&session_id)
    .bind(session_id.as_ref().or(call.session_id.as_ref()))
    .bind(title)
    .bind(&call.provider_id)
    .bind(&call.vendor)
    .bind(&call.model)
    .bind(call.purpose.as_str())
    .bind(is_subagent)
    .bind(t.input as i64)
    .bind(t.cached_input as i64)
    .bind(t.cache_write as i64)
    .bind(t.output as i64)
    .bind(t.thinking as i64)
    .bind(call.ttft_ms.map(|v| v as i64))
    .bind(call.duration_ms.map(|v| v as i64))
    .bind(cost)
    .bind(saved)
    .bind(rfc3339(call.at_ms))
    .execute(conn)
    .await
    .map_err(classify)?;
    Ok(())
}

/// Re-prices every recorded call whose model matches `pattern`.
async fn reprice(conn: &mut SqliteConnection, pattern: &str) -> Result<(), StoreError> {
    let overrides = load_overrides(conn).await?;
    let probe = PricingOverride {
        model_pattern: pattern.to_string(),
        input_per_m: None,
        cached_input_per_m: None,
        cache_write_per_m: None,
        output_per_m: None,
        updated_at: None,
    };
    let rows = sqlx::query(
        "SELECT id, vendor, model, input_tokens, cached_input_tokens, cache_write_tokens, \
         output_tokens, thinking_tokens FROM usage_ledger",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(backend)?;
    for r in rows {
        let model: String = r.get("model");
        if !probe.matches(&model) {
            continue;
        }
        let vendor: String = r.get("vendor");
        let (cost, saved) = pricing::resolve(&overrides, &vendor, &model)
            .map_or((0.0, 0.0), |p| pricing::cost(p, tokens_of(&r)));
        sqlx::query(
            "UPDATE usage_ledger SET estimated_cost_usd = ?, cache_saved_usd = ? WHERE id = ?",
        )
        .bind(cost)
        .bind(saved)
        .bind(r.get::<String, _>("id"))
        .execute(&mut *conn)
        .await
        .map_err(classify)?;
    }
    Ok(())
}

fn tokens_of(r: &sqlx::sqlite::SqliteRow) -> TokenCounts {
    let get = |c: &str| r.get::<i64, _>(c).max(0) as u64;
    TokenCounts {
        input: get("input_tokens"),
        cached_input: get("cached_input_tokens"),
        cache_write: get("cache_write_tokens"),
        output: get("output_tokens"),
        thinking: get("thinking_tokens"),
    }
}

// ----- backfill ------------------------------------------------------------------------------

/// Copies the usage of chats recorded before the ledger existed. Runs once per database.
pub(crate) async fn backfill(store: &SqliteSessionStore) -> Result<(), StoreError> {
    let mut tx = store.pool().begin().await.map_err(backend)?;
    // Claiming the marker first takes the write lock, so two processes cannot both copy.
    let claimed = sqlx::query("INSERT OR IGNORE INTO analytics_meta (key, value) VALUES (?, ?)")
        .bind(BACKFILL_KEY)
        .bind(rfc3339(now_ms()))
        .execute(&mut *tx)
        .await
        .map_err(classify)?;
    if claimed.rows_affected() == 0 {
        return Ok(());
    }
    // `llm_calls` has timings; `messages` covers chats older than that table.
    let rows = sqlx::query(
        "SELECT c.session_id, c.usage_json, c.request_json, c.ttft_ms, c.latency_ms, c.created_at, \
                s.model AS session_model, s.provider AS session_provider, s.config_json \
         FROM llm_calls c JOIN sessions s ON s.id = c.session_id WHERE c.usage_json IS NOT NULL \
         UNION ALL \
         SELECT m.session_id, m.usage_json, NULL, NULL, NULL, m.created_at, \
                s.model, s.provider, s.config_json \
         FROM messages m JOIN sessions s ON s.id = m.session_id \
         WHERE m.usage_json IS NOT NULL AND NOT EXISTS \
            (SELECT 1 FROM llm_calls c WHERE c.session_id = m.session_id AND c.seq = m.seq)",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(backend)?;
    let providers: BTreeMap<String, (String, String)> =
        sqlx::query("SELECT id, vendor, kind FROM providers")
            .fetch_all(&mut *tx)
            .await
            .map_err(backend)?
            .iter()
            .map(|r| (r.get("id"), (r.get("vendor"), r.get("kind"))))
            .collect();
    let overrides = load_overrides(&mut tx).await?;
    for r in rows {
        let Ok(usage) = serde_json::from_str::<Usage>(&r.get::<String, _>("usage_json")) else {
            continue;
        };
        let json = |raw: Option<String>| {
            raw.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .unwrap_or_default()
        };
        let config = json(r.get("config_json"));
        let provider_id = config["provider_id"].as_str().map(str::to_string);
        let legacy: String = r.get("session_provider");
        let (vendor, kind) = provider_id
            .as_ref()
            .and_then(|id| providers.get(id))
            .cloned()
            .unwrap_or_else(|| (legacy_vendor(&legacy).into(), legacy.clone()));
        let kind = ProviderKind::parse(&kind).unwrap_or(ProviderKind::Openai);
        let model = json(r.get("request_json"))["model"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| r.get("session_model"));
        let ms = |c: &str| r.get::<Option<i64>, _>(c).map(|v| v.max(0) as u64);
        let call = LlmUsageRecord {
            session_id: Some(r.get("session_id")),
            provider_id,
            vendor,
            model,
            purpose: CallPurpose::Turn,
            tokens: TokenCounts::from_usage(&usage, kind),
            ttft_ms: ms("ttft_ms"),
            duration_ms: ms("latency_ms"),
            at_ms: r.get("created_at"),
        };
        insert_usage(&mut tx, &overrides, &call).await?;
    }
    tx.commit().await.map_err(backend)
}

/// Vendor of a session without a provider record.
fn legacy_vendor(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "anthropic",
        "openai" | "openai_responses" => "openai",
        "gemini" => "gemini",
        _ => "custom",
    }
}

/// Keeps a deleted chat identifiable in the ledger (its title may have been generated after
/// the rows were written).
pub(crate) async fn snapshot_title(
    conn: &mut SqliteConnection,
    session_id: &str,
) -> Result<(), StoreError> {
    sqlx::query(
        "UPDATE usage_ledger SET session_title_snapshot = \
         COALESCE(NULLIF((SELECT title FROM sessions WHERE id = ?1), ''), session_title_snapshot) \
         WHERE session_id = ?1",
    )
    .bind(session_id)
    .execute(conn)
    .await
    .map_err(classify)?;
    Ok(())
}

// ----- report --------------------------------------------------------------------------------

struct LedgerRow {
    date: NaiveDate,
    session_id: Option<String>,
    origin: Option<String>,
    title: String,
    vendor: String,
    model: String,
    purpose: String,
    tokens: TokenCounts,
    ttft_ms: Option<i64>,
    cost: f64,
    saved: f64,
}

#[derive(Default)]
struct Totals {
    tokens: TokenCounts,
    calls: u64,
    turns: u64,
    cost: f64,
    saved: f64,
    ttft_sum: i64,
    ttft_n: u64,
}

impl Totals {
    fn add(&mut self, r: &LedgerRow) {
        self.tokens.input += r.tokens.input;
        self.tokens.cached_input += r.tokens.cached_input;
        self.tokens.cache_write += r.tokens.cache_write;
        self.tokens.output += r.tokens.output;
        self.tokens.thinking += r.tokens.thinking;
        self.calls += 1;
        self.turns += u64::from(r.purpose == "turn");
        self.cost += r.cost;
        self.saved += r.saved;
        // Auxiliary requests would skew how fast replies feel.
        if r.purpose == "turn"
            && let Some(ms) = r.ttft_ms
        {
            self.ttft_sum += ms;
            self.ttft_n += 1;
        }
    }

    fn avg_ttft(&self) -> Option<f64> {
        (self.ttft_n > 0).then(|| self.ttft_sum as f64 / self.ttft_n as f64)
    }

    fn hit_rate(&self) -> f64 {
        let t = self.tokens;
        percent(t.cached_input, t.input + t.cached_input + t.cache_write)
    }
}

fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64 * 100.0
    }
}

/// One entry per day of `first..=last`, including empty ones.
fn series(
    by_day: &BTreeMap<NaiveDate, Totals>,
    first: NaiveDate,
    last: NaiveDate,
) -> Vec<DayUsage> {
    first
        .iter_days()
        .take_while(|d| *d <= last)
        .map(|date| {
            let mut day = DayUsage {
                date: date.to_string(),
                ..Default::default()
            };
            if let Some(t) = by_day.get(&date) {
                day.cost_usd = t.cost;
                day.cache_saved_usd = t.saved;
                day.total_tokens = t.tokens.total();
                day.cached_tokens = t.tokens.cached_input;
                day.input_tokens = t.tokens.input + t.tokens.cache_write;
                day.output_tokens = t.tokens.output;
                day.thinking_tokens = t.tokens.thinking;
                day.turns = t.turns;
            }
            day
        })
        .collect()
}

impl SqliteSessionStore {
    async fn ledger_since(
        &self,
        since: &str,
        tz: FixedOffset,
    ) -> Result<Vec<LedgerRow>, StoreError> {
        let rows = sqlx::query(
            "SELECT u.session_id, u.origin_session_id, u.vendor, u.model, u.purpose, \
             u.input_tokens, u.cached_input_tokens, u.cache_write_tokens, u.output_tokens, \
             u.thinking_tokens, u.ttft_ms, u.estimated_cost_usd, u.cache_saved_usd, u.created_at, \
             COALESCE(NULLIF(s.title, ''), u.session_title_snapshot, '') AS title \
             FROM usage_ledger u LEFT JOIN sessions s ON s.id = u.session_id \
             WHERE u.created_at >= ? ORDER BY u.created_at, u.id",
        )
        .bind(since)
        .fetch_all(self.pool())
        .await
        .map_err(backend)?;
        rows.iter()
            .map(|r| {
                let at: String = r.get("created_at");
                let at = DateTime::parse_from_rfc3339(&at)
                    .map_err(|e| StoreError::Corrupt(format!("usage_ledger.created_at: {e}")))?;
                Ok(LedgerRow {
                    date: at.with_timezone(&tz).date_naive(),
                    session_id: r.get("session_id"),
                    origin: r.get("origin_session_id"),
                    title: r.get("title"),
                    vendor: r.get("vendor"),
                    model: r.get("model"),
                    purpose: r.get("purpose"),
                    tokens: tokens_of(r),
                    ttft_ms: r.get("ttft_ms"),
                    cost: r.get("estimated_cost_usd"),
                    saved: r.get("cache_saved_usd"),
                })
            })
            .collect()
    }

    async fn top_tools(&self, since: &str) -> Result<Vec<ToolStats>, StoreError> {
        let rows = sqlx::query(
            "SELECT tool_name, tool_kind, COUNT(*) AS calls, AVG(duration_ms) AS avg_ms, \
             SUM(is_error) AS errors FROM tool_usage_ledger WHERE created_at >= ? \
             GROUP BY tool_name, tool_kind ORDER BY calls DESC, tool_name LIMIT ?",
        )
        .bind(since)
        .bind(TOP_TOOLS as i64)
        .fetch_all(self.pool())
        .await
        .map_err(backend)?;
        Ok(rows
            .iter()
            .map(|r| {
                let calls = r.get::<i64, _>("calls") as u64;
                ToolStats {
                    tool_name: r.get("tool_name"),
                    tool_kind: r.get("tool_kind"),
                    calls,
                    avg_duration_ms: r.get("avg_ms"),
                    error_rate_pct: percent(r.get::<i64, _>("errors") as u64, calls),
                }
            })
            .collect())
    }
}

#[async_trait]
impl AnalyticsStore for SqliteSessionStore {
    async fn record_llm_call(&self, call: &LlmUsageRecord) -> Result<(), StoreError> {
        let mut conn = self.pool().acquire().await.map_err(backend)?;
        let overrides = load_overrides(&mut conn).await?;
        insert_usage(&mut conn, &overrides, call).await
    }

    async fn record_tool_call(&self, call: &ToolUsageRecord) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO tool_usage_ledger \
             (id, session_id, tool_name, tool_kind, duration_ms, is_error, created_at) \
             VALUES (?, (SELECT id FROM sessions WHERE id = ?), ?, ?, ?, ?, ?)",
        )
        .bind(uuid::Uuid::now_v7().to_string())
        .bind(&call.session_id)
        .bind(&call.tool_name)
        .bind(call.tool_kind.as_str())
        .bind(call.duration_ms.map(|v| v as i64))
        .bind(call.is_error)
        .bind(rfc3339(call.at_ms))
        .execute(self.pool())
        .await
        .map_err(classify)?;
        Ok(())
    }

    async fn usage_report(&self, q: ReportQuery) -> Result<UsageReport, StoreError> {
        let tz = offset(q.tz_offset_minutes);
        let today = local_date(q.now_ms, tz);
        let (month_first, month_last) =
            month_bounds(today.year(), today.month()).expect("today has a month");
        let range_first = match q.range {
            ReportRange::Days7 => days_before(today, 6),
            ReportRange::Days30 => days_before(today, 29),
            ReportRange::Month => month_first,
            ReportRange::Weeks20 => days_before(today, HEATMAP_DAYS - 1),
        };
        let heatmap_first = days_before(today, HEATMAP_DAYS - 1);
        let (cal_first, cal_last) = q
            .calendar_month
            .and_then(|(y, m)| month_bounds(y, m))
            .unwrap_or((month_first, month_last));
        let since = heatmap_first.min(cal_first).min(month_first);
        let rows = self.ledger_since(&day_start(since, tz), tz).await?;

        let mut by_day: BTreeMap<NaiveDate, Totals> = BTreeMap::new();
        let mut total = Totals::default();
        let mut active = BTreeSet::new();
        let mut models: BTreeMap<(String, String), Totals> = BTreeMap::new();
        // Rows are in time order, so the title and model end up being the latest ones.
        let mut sessions: BTreeMap<String, (Totals, Option<String>, String, String)> =
            BTreeMap::new();
        let mut month_cost = 0.0;
        for r in &rows {
            by_day.entry(r.date).or_default().add(r);
            if r.date >= month_first && r.date <= month_last {
                month_cost += r.cost;
            }
            if r.date < range_first || r.date > today {
                continue;
            }
            total.add(r);
            active.insert(r.date);
            models
                .entry((r.vendor.clone(), r.model.clone()))
                .or_default()
                .add(r);
            if let Some(key) = r.session_id.as_ref().or(r.origin.as_ref()) {
                let entry = sessions.entry(key.clone()).or_default();
                entry.0.add(r);
                entry.1 = r.session_id.clone();
                entry.2 = r.title.clone();
                if r.purpose == "turn" || entry.3.is_empty() {
                    entry.3 = r.model.clone();
                }
            }
        }

        let overrides = self.list_pricing().await?;
        let mut by_model: Vec<ModelUsage> = models
            .into_iter()
            .map(|((vendor, model), t)| ModelUsage {
                calls: t.calls,
                input_tokens: t.tokens.input + t.tokens.cache_write,
                cached_tokens: t.tokens.cached_input,
                output_tokens: t.tokens.output,
                thinking_tokens: t.tokens.thinking,
                cache_hit_rate_pct: t.hit_rate(),
                avg_ttft_ms: t.avg_ttft(),
                cost_usd: t.cost,
                cache_saved_usd: t.saved,
                cost_share_pct: if total.cost > 0.0 {
                    t.cost / total.cost * 100.0
                } else {
                    0.0
                },
                has_pricing: pricing::resolve(&overrides, &vendor, &model).is_some(),
                model,
                vendor,
            })
            .collect();
        by_model.sort_by(|a, b| {
            b.cost_usd
                .total_cmp(&a.cost_usd)
                .then_with(|| b.calls.cmp(&a.calls))
        });

        let mut top_sessions: Vec<SessionUsage> = sessions
            .into_values()
            .map(|(t, session_id, title, model)| SessionUsage {
                exists: session_id.is_some(),
                session_id,
                title,
                model,
                turns: t.turns,
                cache_hit_rate_pct: t.hit_rate(),
                cost_usd: t.cost,
            })
            .collect();
        top_sessions.sort_by(|a, b| {
            b.cost_usd
                .total_cmp(&a.cost_usd)
                .then_with(|| b.turns.cmp(&a.turns))
        });
        top_sessions.truncate(TOP_SESSIONS);

        let t = total.tokens;
        let elapsed = f64::from(today.day());
        let summary = UsageSummary {
            cost_usd: total.cost,
            cache_saved_usd: total.saved,
            total_tokens: t.total(),
            input_tokens: t.input,
            cached_input_tokens: t.cached_input,
            cache_write_tokens: t.cache_write,
            output_tokens: t.output,
            thinking_tokens: t.thinking,
            cache_hit_rate_pct: total.hit_rate(),
            thinking_share_pct: percent(t.thinking, t.output + t.thinking),
            turns: total.turns,
            avg_ttft_ms: total.avg_ttft(),
            active_days: active.len() as u64,
            total_days: (today - range_first).num_days() as u64 + 1,
            current_month_cost_usd: month_cost,
            projected_month_cost_usd: month_cost / elapsed * f64::from(month_last.day()),
        };
        let total_records: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_ledger")
            .fetch_one(self.pool())
            .await
            .map_err(backend)?;
        Ok(UsageReport {
            summary,
            budget: budget_status(&self.budget_settings().await?, month_cost),
            daily_heatmap: series(&by_day, heatmap_first, today),
            month_calendar: series(&by_day, cal_first, cal_last),
            trend_14d: series(&by_day, days_before(today, TREND_DAYS - 1), today),
            by_model,
            top_tools: self.top_tools(&day_start(range_first, tz)).await?,
            top_sessions,
            calendar_month: cal_first.format("%Y-%m").to_string(),
            total_records: total_records.max(0) as u64,
        })
    }

    async fn budget_settings(&self) -> Result<BudgetSettings, StoreError> {
        let row = sqlx::query("SELECT * FROM budget_settings WHERE id = 1")
            .fetch_optional(self.pool())
            .await
            .map_err(backend)?;
        Ok(
            row.map_or_else(BudgetSettings::default, |r| BudgetSettings {
                monthly_budget_usd: r.get("monthly_budget_usd"),
                warning_threshold_pct: r.get::<i64, _>("warning_threshold_pct").clamp(1, 100)
                    as u32,
                scoped_budgets: r
                    .get::<Option<String>, _>("scoped_budgets_json")
                    .and_then(|s| serde_json::from_str(&s).ok()),
            }),
        )
    }

    async fn set_budget_settings(&self, s: &BudgetSettings) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT OR REPLACE INTO budget_settings \
             (id, monthly_budget_usd, warning_threshold_pct, scoped_budgets_json) \
             VALUES (1, ?, ?, ?)",
        )
        .bind(s.monthly_budget_usd)
        .bind(i64::from(s.warning_threshold_pct))
        .bind(s.scoped_budgets.as_ref().map(ToString::to_string))
        .execute(self.pool())
        .await
        .map_err(classify)?;
        Ok(())
    }

    async fn month_cost(&self, tz_offset_minutes: i32, now_ms: i64) -> Result<f64, StoreError> {
        let tz = offset(tz_offset_minutes);
        let today = local_date(now_ms, tz);
        let (first, last) = month_bounds(today.year(), today.month()).expect("today has a month");
        let end = last.succ_opt().unwrap_or(last);
        sqlx::query_scalar(
            "SELECT TOTAL(estimated_cost_usd) FROM usage_ledger \
             WHERE created_at >= ? AND created_at < ?",
        )
        .bind(day_start(first, tz))
        .bind(day_start(end, tz))
        .fetch_one(self.pool())
        .await
        .map_err(backend)
    }

    async fn list_pricing(&self) -> Result<Vec<PricingOverride>, StoreError> {
        let mut conn = self.pool().acquire().await.map_err(backend)?;
        load_overrides(&mut conn).await
    }

    async fn put_pricing(&self, p: &PricingOverride, recalculate: bool) -> Result<(), StoreError> {
        let mut tx = self.pool().begin().await.map_err(backend)?;
        sqlx::query(
            "INSERT OR REPLACE INTO model_pricing_overrides (model_pattern, input_per_m, \
             cached_input_per_m, cache_write_per_m, output_per_m, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&p.model_pattern)
        .bind(p.input_per_m)
        .bind(p.cached_input_per_m)
        .bind(p.cache_write_per_m)
        .bind(p.output_per_m)
        .bind(rfc3339(now_ms()))
        .execute(&mut *tx)
        .await
        .map_err(classify)?;
        if recalculate {
            reprice(&mut tx, &p.model_pattern).await?;
        }
        tx.commit().await.map_err(backend)
    }

    async fn delete_pricing(&self, pattern: &str, recalculate: bool) -> Result<(), StoreError> {
        let mut tx = self.pool().begin().await.map_err(backend)?;
        let done = sqlx::query("DELETE FROM model_pricing_overrides WHERE model_pattern = ?")
            .bind(pattern)
            .execute(&mut *tx)
            .await
            .map_err(classify)?;
        crate::found(done.rows_affected(), pattern)?;
        if recalculate {
            reprice(&mut tx, pattern).await?;
        }
        tx.commit().await.map_err(backend)
    }
}

#[cfg(test)]
mod tests;
