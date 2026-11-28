import { useState } from "react";
import type { DayUsage } from "../../api/analytics";
import { SegmentedControl } from "../../components/ui";
import { useI18n } from "../../i18n";
import { parseDay, shiftMonth, tokens, usd } from "./format";

type Metric = "cost" | "tokens" | "turns";
type Mode = "daily" | "month";

const WEEKS = 20;

const value = (d: DayUsage, m: Metric) => (m === "cost" ? d.cost_usd : m === "tokens" ? d.total_tokens : d.turns);

/** 0 (nothing) to 4 (close to the busiest day shown). */
const level = (v: number, max: number) => (v <= 0 || max <= 0 ? 0 : Math.min(4, Math.max(1, Math.ceil((v / max) * 4))));

const short = (v: number, m: Metric) => (m === "cost" ? usd(v) : m === "tokens" ? tokens(v) : String(v));

/** Monday = 0. */
const weekday = (date: string) => (parseDay(date).getDay() + 6) % 7;

export function Heatmap({
  days,
  calendar,
  month,
  currentMonth,
  onMonth,
}: {
  days: DayUsage[];
  calendar: DayUsage[];
  month: string;
  currentMonth: string;
  onMonth: (month: string) => void;
}) {
  const { t, fmt } = useI18n();
  const [metric, setMetric] = useState<Metric>("cost");
  const [mode, setMode] = useState<Mode>("daily");
  const [hover, setHover] = useState<DayUsage | null>(null);

  // 20 columns of Monday-first weeks, the current week last (its future days stay absent).
  const today = days[days.length - 1];
  const shown = today ? days.slice(-((WEEKS - 1) * 7 + weekday(today.date) + 1)) : [];
  const cells = mode === "daily" ? shown : calendar;
  const max = Math.max(0, ...cells.map((d) => value(d, metric)));

  const detail = (d: DayUsage) =>
    `${d.date} · ${usd(d.cost_usd)} · ${tokens(d.total_tokens)} tok · ${t("usage.turnsCount", { count: d.turns })}${
      d.cache_saved_usd > 0 ? ` ${t("usage.heat.saved", { amount: usd(d.cache_saved_usd) })}` : ""
    }`;

  const monday = new Date(2024, 0, 1); // a Monday
  const weekdays = Array.from({ length: 7 }, (_, i) =>
    fmt.date(new Date(monday.getTime() + i * 86_400_000).getTime(), { weekday: "short" }),
  );

  return (
    <section className="u-card" aria-label={t("usage.heat.title")}>
      <header className="u-card-head">
        <h2>{t("usage.heat.title")}</h2>
        <span className="u-detail" aria-live="polite">
          {hover ? detail(hover) : t("usage.heat.hint")}
        </span>
        <SegmentedControl<Metric>
          size="sm"
          ariaLabel={t("usage.metric")}
          value={metric}
          onChange={setMetric}
          options={[
            { value: "cost", label: t("usage.metric.cost") },
            { value: "tokens", label: t("usage.metric.tokens") },
            { value: "turns", label: t("usage.metric.turns") },
          ]}
        />
        <SegmentedControl<Mode>
          size="sm"
          ariaLabel={t("usage.view")}
          value={mode}
          onChange={(m) => {
            setMode(m);
            setHover(null);
          }}
          options={[
            { value: "daily", label: t("usage.view.daily") },
            { value: "month", label: t("usage.view.month") },
          ]}
        />
      </header>

      {mode === "daily" ? (
        <div className="u-heat" onMouseLeave={() => setHover(null)}>
          <div className="u-heat-days" aria-hidden="true">
            {weekdays.map((w, i) => (
              <span key={w}>{i % 2 === 0 ? w : ""}</span>
            ))}
          </div>
          <div className="u-heat-grid">
            {shown.map((d) => (
              <span key={d.date} role="img" aria-label={detail(d)} className={`u-cell l${level(value(d, metric), max)}`} onMouseEnter={() => setHover(d)} />
            ))}
          </div>
        </div>
      ) : (
        <div onMouseLeave={() => setHover(null)}>
          <div className="u-cal-nav">
            <button type="button" className="btn ghost small" onClick={() => onMonth(shiftMonth(month, -1))} aria-label={t("usage.month.prev")}>
              ‹
            </button>
            <span>{fmt.date(parseDay(`${month}-01`).getTime(), { year: "numeric", month: "long" })}</span>
            <button
              type="button"
              className="btn ghost small"
              disabled={month >= currentMonth}
              onClick={() => onMonth(shiftMonth(month, 1))}
              aria-label={t("usage.month.next")}
            >
              ›
            </button>
          </div>
          <div className="u-cal">
            {weekdays.map((w) => (
              <span key={w} className="u-cal-weekday">
                {w}
              </span>
            ))}
            {calendar[0] && Array.from({ length: weekday(calendar[0].date) }, (_, i) => <span key={`pad-${i}`} />)}
            {calendar.map((d) => {
              const v = value(d, metric);
              return (
                <span key={d.date} role="img" aria-label={detail(d)} className={`u-cal-cell l${level(v, max)}`} onMouseEnter={() => setHover(d)}>
                  <span className="u-cal-date">{parseDay(d.date).getDate()}</span>
                  <span className="u-cal-value">{v > 0 ? short(v, metric) : ""}</span>
                </span>
              );
            })}
          </div>
        </div>
      )}

      <div className="u-scale">
        <span>{t("usage.less")}</span>
        {[0, 1, 2, 3, 4].map((l) => (
          <i key={l} className={`u-swatch l${l}`} />
        ))}
        <span>{t("usage.more")}</span>
      </div>
    </section>
  );
}
