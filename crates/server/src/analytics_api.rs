//! `/api/analytics/*`: the usage report, the monthly budget and custom model prices.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Json, Query, State};
use axum::routing::get;
use oort_storage::analytics::{
    AnalyticsStore, BudgetSettings, BudgetStatus, ReportQuery, ReportRange, UsageReport,
    budget_status,
};
use oort_storage::pricing::PricingOverride;
use serde::{Deserialize, Serialize};

use crate::api::ApiError;
use crate::manager::SessionManager;

type AppState = Arc<SessionManager>;
type ApiResult<T> = Result<Json<T>, ApiError>;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/analytics/usage", get(usage))
        .route("/analytics/budget", get(get_budget).put(put_budget))
        .route(
            "/analytics/pricing",
            get(list_pricing).put(put_pricing).delete(delete_pricing),
        )
}

fn store(m: &AppState) -> &Arc<dyn AnalyticsStore> {
    &m.registry().analytics
}

fn tz_offset(minutes: i32) -> Result<i32, ApiError> {
    if (-14 * 60..=14 * 60).contains(&minutes) {
        Ok(minutes)
    } else {
        Err(ApiError::BadRequest(
            "`tz_offset_minutes` must be within ±840".into(),
        ))
    }
}

#[derive(Deserialize)]
struct UsageParams {
    #[serde(default)]
    range: Option<String>,
    tz_offset_minutes: i32,
    /// `YYYY-MM`.
    #[serde(default)]
    calendar_month: Option<String>,
}

fn parse_month(s: &str) -> Option<(i32, u32)> {
    let (year, month) = s.split_once('-')?;
    let (year, month) = (year.parse().ok()?, month.parse().ok()?);
    ((1970..=9999).contains(&year) && (1..=12).contains(&month)).then_some((year, month))
}

async fn usage(State(m): State<AppState>, Query(p): Query<UsageParams>) -> ApiResult<UsageReport> {
    let range = p.range.as_deref().unwrap_or("30d");
    let range = ReportRange::parse(range)
        .ok_or_else(|| ApiError::BadRequest("`range` must be 7d, 30d, month or 20w".into()))?;
    let calendar_month = p
        .calendar_month
        .as_deref()
        .map(|s| {
            parse_month(s)
                .ok_or_else(|| ApiError::BadRequest("`calendar_month` must be YYYY-MM".into()))
        })
        .transpose()?;
    let query = ReportQuery {
        range,
        tz_offset_minutes: tz_offset(p.tz_offset_minutes)?,
        calendar_month,
        now_ms: crate::now_ms(),
    };
    Ok(Json(store(&m).usage_report(query).await?))
}

#[derive(Deserialize)]
struct TzParams {
    #[serde(default)]
    tz_offset_minutes: i32,
}

/// The settings and where the current month stands against them.
#[derive(Serialize)]
struct BudgetView {
    #[serde(flatten)]
    status: BudgetStatus,
    current_month_cost_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    scoped_budgets: Option<serde_json::Value>,
}

async fn budget_view(m: &AppState, tz: i32) -> ApiResult<BudgetView> {
    let settings = store(m).budget_settings().await?;
    let cost = store(m).month_cost(tz_offset(tz)?, crate::now_ms()).await?;
    Ok(Json(BudgetView {
        status: budget_status(&settings, cost),
        current_month_cost_usd: cost,
        scoped_budgets: settings.scoped_budgets,
    }))
}

async fn get_budget(State(m): State<AppState>, Query(p): Query<TzParams>) -> ApiResult<BudgetView> {
    budget_view(&m, p.tz_offset_minutes).await
}

async fn put_budget(
    State(m): State<AppState>,
    Query(p): Query<TzParams>,
    Json(settings): Json<BudgetSettings>,
) -> ApiResult<BudgetView> {
    if settings
        .monthly_budget_usd
        .is_some_and(|b| !b.is_finite() || b <= 0.0)
    {
        return Err(ApiError::BadRequest(
            "`monthly_budget_usd` must be positive (or null for no limit)".into(),
        ));
    }
    if !(1..=100).contains(&settings.warning_threshold_pct) {
        return Err(ApiError::BadRequest(
            "`warning_threshold_pct` must be between 1 and 100".into(),
        ));
    }
    store(&m).set_budget_settings(&settings).await?;
    budget_view(&m, p.tz_offset_minutes).await
}

async fn list_pricing(State(m): State<AppState>) -> ApiResult<Vec<PricingOverride>> {
    Ok(Json(store(&m).list_pricing().await?))
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct PricingInput {
    #[serde(flatten)]
    pricing: PricingOverride,
    /// Also re-price the calls already recorded.
    #[serde(default = "yes")]
    recalculate: bool,
}

async fn put_pricing(
    State(m): State<AppState>,
    Json(input): Json<PricingInput>,
) -> ApiResult<Vec<PricingOverride>> {
    let mut p = input.pricing;
    p.model_pattern = p.model_pattern.trim().to_string();
    let bare = p.model_pattern.trim_end_matches('*');
    if bare.is_empty() || p.model_pattern.len() > 200 {
        return Err(ApiError::BadRequest(
            "`model_pattern` must be a model id, optionally ending in `*`".into(),
        ));
    }
    let prices = [
        p.input_per_m,
        p.cached_input_per_m,
        p.cache_write_per_m,
        p.output_per_m,
    ];
    if p.input_per_m.is_none() || p.output_per_m.is_none() {
        return Err(ApiError::BadRequest(
            "`input_per_m` and `output_per_m` are required".into(),
        ));
    }
    if prices.iter().flatten().any(|v| !v.is_finite() || *v < 0.0) {
        return Err(ApiError::BadRequest("prices must not be negative".into()));
    }
    store(&m).put_pricing(&p, input.recalculate).await?;
    Ok(Json(store(&m).list_pricing().await?))
}

#[derive(Deserialize)]
struct DeletePricingParams {
    model_pattern: String,
    #[serde(default = "yes")]
    recalculate: bool,
}

/// The pattern is a query parameter: model ids may contain `/`.
async fn delete_pricing(
    State(m): State<AppState>,
    Query(p): Query<DeletePricingParams>,
) -> ApiResult<Vec<PricingOverride>> {
    store(&m)
        .delete_pricing(&p.model_pattern, p.recalculate)
        .await?;
    Ok(Json(store(&m).list_pricing().await?))
}
