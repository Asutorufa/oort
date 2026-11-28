import { useCallback, useEffect, useRef, useState } from "react";
import { errorText } from "../../api";
import { analytics, type BudgetStatus, type ModelUsage, type SessionUsage, type ToolStats, type UsageRange, type UsageReport, type UsageSummary } from "../../api/analytics";
import { ChartIcon, CubeIcon, GlobeIcon, ToolIcon } from "../../brand/Icons";
import { ErrorBar } from "../../components/common";
import { HeaderActions } from "../../components/settings/MasterDetail";
import { SegmentedControl } from "../../components/ui";
import { navigate } from "../../hooks/useHashRoute";
import { useI18n, useT } from "../../i18n";
import { BudgetModal, type BudgetTab } from "./BudgetModal";
import { percent, tokens, usd } from "./format";
import { Heatmap } from "./Heatmap";
import { TrendChart } from "./TrendChart";
import "./usage.css";

const REFRESH_MS = 15_000;

/** The report of the chosen period, refetched while the page is open. */
function useReport(range: UsageRange, month: string | null) {
  const [data, setData] = useState<UsageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const latest = useRef(0);
  const reload = useCallback(async () => {
    const call = ++latest.current;
    try {
      const report = await analytics.usage(range, month);
      // A slow answer for a period no longer selected must not win.
      if (call !== latest.current) return;
      setData(report);
      setError(null);
    } catch (e) {
      if (call === latest.current) setError(errorText(e));
    }
  }, [range, month]);
  useEffect(() => {
    reload();
    const timer = window.setInterval(reload, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [reload]);
  return { data, error, reload };
}

function Kpi({ label, value, sub, tone }: { label: string; value: string; sub: string; tone?: "good" }) {
  return (
    <div className="u-kpi">
      <div className="u-kpi-label">{label}</div>
      <div className={`u-kpi-value ${tone ?? ""}`}>{value}</div>
      <div className="u-kpi-sub">{sub}</div>
    </div>
  );
}

function Overview({ s, budget }: { s: UsageSummary; budget: BudgetStatus }) {
  const { t, fmt } = useI18n();
  const limit = budget.monthly_budget_usd;
  const forecast = t("usage.kpi.forecast", { amount: usd(s.projected_month_cost_usd) });
  return (
    <section className="u-overview">
      <div className="u-kpis">
        <Kpi label={t("usage.kpi.cost")} value={usd(s.cost_usd)} sub={limit == null ? forecast : `${forecast} / ${t("usage.kpi.budget", { amount: usd(limit) })}`} />
        <Kpi
          label={t("usage.kpi.saved")}
          value={s.cache_saved_usd > 0 ? `-${usd(s.cache_saved_usd)}` : usd(0)}
          tone="good"
          sub={`${t("usage.kpi.hitRate", { rate: percent(s.cache_hit_rate_pct) })} · ${t("usage.kpi.withoutCache", { amount: usd(s.cost_usd + s.cache_saved_usd) })}`}
        />
        <Kpi
          label={t("usage.kpi.tokens")}
          value={tokens(s.total_tokens)}
          sub={`${t("usage.kpi.thinking", { rate: `${Math.round(s.thinking_share_pct)}%` })} · ${t("usage.turnsCount", { count: s.turns })}`}
        />
        <Kpi
          label={t("usage.kpi.ttft")}
          value={s.avg_ttft_ms == null ? "–" : `${fmt.number(Math.round(s.avg_ttft_ms))} ms`}
          sub={t("usage.kpi.activeDays", { active: s.active_days, total: s.total_days })}
        />
      </div>
      {limit != null && (
        <div className={`u-budget is-${budget.status}`}>
          <div className="u-budget-text">
            <span>{t("usage.budget.month")}</span>
            <span className="u-num">
              {usd(s.current_month_cost_usd)} / {usd(limit)} ({Math.round(budget.usage_pct)}%)
            </span>
          </div>
          <div className="u-meter" role="progressbar" aria-label={t("usage.budget.month")} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.min(100, Math.round(budget.usage_pct))}>
            <span className="u-meter-fill" style={{ width: `${Math.min(100, budget.usage_pct)}%` }} />
            <span className="u-meter-mark" style={{ left: `${budget.warning_threshold_pct}%` }} />
          </div>
        </div>
      )}
    </section>
  );
}

function ModelTable({ models, onPrice }: { models: ModelUsage[]; onPrice: (model: string) => void }) {
  const { t, fmt } = useI18n();
  return (
    <section className="u-card" aria-label={t("usage.models.title")}>
      <header className="u-card-head">
        <h2>{t("usage.models.title")}</h2>
      </header>
      {models.length === 0 ? (
        <p className="u-none">{t("usage.noneInRange")}</p>
      ) : (
        <div className="u-table-wrap">
          <table className="u-table">
            <thead>
              <tr>
                <th>{t("usage.col.model")}</th>
                <th>{t("usage.col.calls")}</th>
                <th>{t("usage.col.input")}</th>
                <th>{t("usage.col.output")}</th>
                <th>{t("usage.col.hitRate")}</th>
                <th>{t("usage.col.ttft")}</th>
                <th>{t("usage.col.cost")}</th>
              </tr>
            </thead>
            <tbody>
              {models.map((m) => (
                <tr key={`${m.vendor}/${m.model}`}>
                  <td>
                    <div className="u-model mono" title={m.model}>
                      {m.model}
                    </div>
                    <div className="u-vendor">{m.vendor}</div>
                  </td>
                  <td>{fmt.number(m.calls)}</td>
                  <td>
                    {tokens(m.input_tokens)} / {tokens(m.cached_tokens)}
                  </td>
                  <td>
                    {tokens(m.output_tokens)} ({tokens(m.thinking_tokens)})
                  </td>
                  <td>{percent(m.cache_hit_rate_pct)}</td>
                  <td>{m.avg_ttft_ms == null ? "–" : `${fmt.number(Math.round(m.avg_ttft_ms))} ms`}</td>
                  <td className="u-cost">
                    {m.has_pricing ? (
                      <>
                        <div>
                          {usd(m.cost_usd)} <span className="u-share">{Math.round(m.cost_share_pct)}%</span>
                        </div>
                        <div className="u-sharebar">
                          <span style={{ width: `${m.cost_share_pct}%` }} />
                        </div>
                      </>
                    ) : (
                      <button type="button" className="text-btn" onClick={() => onPrice(m.model)}>
                        {t("usage.price.set")}
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

const KIND = {
  builtin: { label: "Builtin", icon: <ToolIcon size={14} /> },
  server: { label: "Server", icon: <GlobeIcon size={14} /> },
  mcp: { label: "MCP", icon: <CubeIcon size={14} /> },
};

function Tools({ tools }: { tools: ToolStats[] }) {
  const { t, fmt } = useI18n();
  return (
    <section className="u-card" aria-label={t("usage.tools.title")}>
      <header className="u-card-head">
        <h2>{t("usage.tools.title")}</h2>
      </header>
      {tools.length === 0 ? (
        <p className="u-none">{t("usage.noneInRange")}</p>
      ) : (
        <ul className="u-list">
          {tools.map((tool) => (
            <li key={`${tool.tool_kind}/${tool.tool_name}`} className="u-row">
              <span className="u-row-icon">{KIND[tool.tool_kind].icon}</span>
              <span className="u-row-main">
                <span className="u-row-title mono" title={tool.tool_name}>
                  {tool.tool_name}
                </span>
                <span className="u-row-sub">
                  <span className="badge">{KIND[tool.tool_kind].label}</span>
                  {t("usage.tools.calls", { count: tool.calls })} · {fmt.ms(tool.avg_duration_ms == null ? null : Math.round(tool.avg_duration_ms))}
                </span>
              </span>
              <span className="u-row-value">{t("usage.tools.success", { rate: percent(100 - tool.error_rate_pct) })}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function Sessions({ sessions }: { sessions: SessionUsage[] }) {
  const t = useT();
  return (
    <section className="u-card" aria-label={t("usage.sessions.title")}>
      <header className="u-card-head">
        <h2>{t("usage.sessions.title")}</h2>
      </header>
      {sessions.length === 0 ? (
        <p className="u-none">{t("usage.noneInRange")}</p>
      ) : (
        <ul className="u-list">
          {sessions.map((s, i) => {
            const body = (
              <>
                <span className="u-row-main">
                  <span className="u-row-title">{s.title || t("common.untitled")}</span>
                  <span className="u-row-sub">
                    {!s.exists && <span className="badge">{t("usage.sessions.deleted")}</span>}
                    <span className="mono">{s.model}</span> · {t("usage.turnsCount", { count: s.turns })} ·{" "}
                    {t("usage.kpi.hitRate", { rate: percent(s.cache_hit_rate_pct) })}
                  </span>
                </span>
                <span className="u-row-value">{usd(s.cost_usd)}</span>
              </>
            );
            return (
              <li key={s.session_id ?? `deleted-${i}`}>
                {s.exists && s.session_id ? (
                  // Leaving the settings route closes the workspace over that chat.
                  <button type="button" className="u-row u-link" onClick={() => navigate("chat", s.session_id)}>
                    {body}
                  </button>
                ) : (
                  <div className="u-row">{body}</div>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/** Usage statistics and budget. */
export function UsageView() {
  const t = useT();
  const [range, setRange] = useState<UsageRange>("30d");
  /** `null`: the current month. */
  const [month, setMonth] = useState<string | null>(null);
  const [modal, setModal] = useState<{ tab: BudgetTab; model: string } | null>(null);
  const { data, error, reload } = useReport(range, month);
  const now = new Date();
  const currentMonth = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}`;
  const unpriced = [...new Set(data?.by_model.filter((m) => !m.has_pricing).map((m) => m.model) ?? [])];

  return (
    <>
      <HeaderActions>
        <SegmentedControl<UsageRange>
          ariaLabel={t("usage.range")}
          value={range}
          onChange={setRange}
          options={[
            { value: "7d", label: t("usage.range.7d") },
            { value: "30d", label: t("usage.range.30d") },
            { value: "month", label: t("usage.range.month") },
            { value: "20w", label: t("usage.range.20w") },
          ]}
        />
        <button type="button" className="head-action" onClick={() => setModal({ tab: "budget", model: "" })}>
          {t("usage.configure")}
        </button>
      </HeaderActions>

      <div className="usage">
        <ErrorBar error={error} />
        {!data ? (
          !error && <p className="u-none">{t("common.loading")}</p>
        ) : data.total_records === 0 ? (
          <div className="u-card md-empty">
            <span className="md-empty-icon">
              <ChartIcon size={18} />
            </span>
            <h2>{t("usage.empty.title")}</h2>
            <p>{t("usage.empty.body")}</p>
          </div>
        ) : (
          <>
            <Overview s={data.summary} budget={data.budget} />
            <Heatmap days={data.daily_heatmap} calendar={data.month_calendar} month={month ?? data.calendar_month} currentMonth={currentMonth} onMonth={setMonth} />
            <TrendChart days={data.trend_14d} />
            <ModelTable models={data.by_model} onPrice={(model) => setModal({ tab: "pricing", model })} />
            <div className="u-columns">
              <Tools tools={data.top_tools} />
              <Sessions sessions={data.top_sessions} />
            </div>
          </>
        )}
      </div>

      {modal && <BudgetModal initialTab={modal.tab} initialModel={modal.model} unpriced={unpriced} onClose={() => setModal(null)} onChanged={reload} />}
    </>
  );
}
