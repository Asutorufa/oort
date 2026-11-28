// Usage statistics, the monthly budget and custom model prices (`/api/analytics/*`).

import { request } from "./client";

export type UsageRange = "7d" | "30d" | "month" | "20w";
export type BudgetState = "ok" | "warning" | "exceeded" | "unset";

export interface BudgetStatus {
  monthly_budget_usd: number | null;
  warning_threshold_pct: number;
  usage_pct: number;
  status: BudgetState;
}

export interface BudgetView extends BudgetStatus {
  current_month_cost_usd: number;
  scoped_budgets?: unknown;
}

export interface UsageSummary {
  cost_usd: number;
  cache_saved_usd: number;
  total_tokens: number;
  input_tokens: number;
  cached_input_tokens: number;
  cache_write_tokens: number;
  output_tokens: number;
  thinking_tokens: number;
  cache_hit_rate_pct: number;
  thinking_share_pct: number;
  turns: number;
  avg_ttft_ms: number | null;
  active_days: number;
  total_days: number;
  current_month_cost_usd: number;
  projected_month_cost_usd: number;
}

export interface DayUsage {
  /** Local date, `YYYY-MM-DD`. */
  date: string;
  cost_usd: number;
  cache_saved_usd: number;
  total_tokens: number;
  cached_tokens: number;
  input_tokens: number;
  output_tokens: number;
  thinking_tokens: number;
  turns: number;
}

export interface ModelUsage {
  model: string;
  vendor: string;
  calls: number;
  input_tokens: number;
  cached_tokens: number;
  output_tokens: number;
  thinking_tokens: number;
  cache_hit_rate_pct: number;
  avg_ttft_ms: number | null;
  cost_usd: number;
  cache_saved_usd: number;
  cost_share_pct: number;
  has_pricing: boolean;
}

export interface ToolStats {
  tool_name: string;
  tool_kind: "builtin" | "server" | "mcp";
  calls: number;
  avg_duration_ms: number | null;
  error_rate_pct: number;
}

export interface SessionUsage {
  session_id: string | null;
  title: string;
  model: string;
  turns: number;
  cache_hit_rate_pct: number;
  cost_usd: number;
  exists: boolean;
}

export interface UsageReport {
  summary: UsageSummary;
  budget: BudgetStatus;
  daily_heatmap: DayUsage[];
  month_calendar: DayUsage[];
  trend_14d: DayUsage[];
  by_model: ModelUsage[];
  top_tools: ToolStats[];
  top_sessions: SessionUsage[];
  calendar_month: string;
  total_records: number;
}

export interface PricingOverride {
  model_pattern: string;
  input_per_m: number | null;
  cached_input_per_m: number | null;
  cache_write_per_m: number | null;
  output_per_m: number | null;
  updated_at?: string | null;
}

/** Minutes east of UTC, so the server cuts days at local midnight. */
const tz = () => -new Date().getTimezoneOffset();

export const analytics = {
  usage: (range: UsageRange, calendarMonth: string | null) => {
    const q = new URLSearchParams({ range, tz_offset_minutes: String(tz()) });
    if (calendarMonth) q.set("calendar_month", calendarMonth);
    return request<UsageReport>("GET", `/analytics/usage?${q}`);
  },
  budget: () => request<BudgetView>("GET", `/analytics/budget?tz_offset_minutes=${tz()}`),
  setBudget: (monthly_budget_usd: number | null, warning_threshold_pct: number, scoped_budgets?: unknown) =>
    request<BudgetView>("PUT", `/analytics/budget?tz_offset_minutes=${tz()}`, { monthly_budget_usd, warning_threshold_pct, scoped_budgets }),
  pricing: () => request<PricingOverride[]>("GET", "/analytics/pricing"),
  setPricing: (p: PricingOverride, recalculate: boolean) => request<PricingOverride[]>("PUT", "/analytics/pricing", { ...p, recalculate }),
  deletePricing: (pattern: string, recalculate: boolean) =>
    request<PricingOverride[]>("DELETE", `/analytics/pricing?${new URLSearchParams({ model_pattern: pattern, recalculate: String(recalculate) })}`),
};

/** Fired after the budget or prices change, so the navigation dot updates at once. */
export const BUDGET_CHANGED = "oort:budget-changed";
