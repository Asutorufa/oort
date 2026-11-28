import { useState } from "react";
import type { DayUsage } from "../../api/analytics";
import { useT, type Key } from "../../i18n";
import { parseDay, tokens } from "./format";

/** Stacked from the baseline up. */
const LAYERS: { id: "cached" | "input" | "output"; label: Key; of: (d: DayUsage) => number }[] = [
  { id: "cached", label: "usage.layer.cached", of: (d) => d.cached_tokens },
  { id: "input", label: "usage.layer.input", of: (d) => d.input_tokens },
  { id: "output", label: "usage.layer.output", of: (d) => d.output_tokens + d.thinking_tokens },
];

/** Token composition of the last 14 days. */
export function TrendChart({ days }: { days: DayUsage[] }) {
  const t = useT();
  const [hover, setHover] = useState<DayUsage | null>(null);
  const max = Math.max(0, ...days.map((d) => d.total_tokens));
  const detail = (d: DayUsage) => `${d.date} · ${LAYERS.map((l) => `${t(l.label)} ${tokens(l.of(d))}`).join(" · ")}`;

  return (
    <section className="u-card" aria-label={t("usage.trend.title")}>
      <header className="u-card-head">
        <h2>{t("usage.trend.title")}</h2>
        <span className="u-detail" aria-live="polite">
          {hover ? detail(hover) : ""}
        </span>
        <ul className="u-legend">
          {LAYERS.map((l) => (
            <li key={l.id}>
              <i className={`u-swatch ${l.id}`} />
              {t(l.label)}
            </li>
          ))}
        </ul>
      </header>
      <div className="u-trend-max">{tokens(max)}</div>
      <div className="u-trend" onMouseLeave={() => setHover(null)}>
        {days.map((d, i) => (
          <div key={d.date} className="u-trend-col" role="img" aria-label={detail(d)} onMouseEnter={() => setHover(d)}>
            <div className="u-trend-plot">
              {LAYERS.map(
                (l) => l.of(d) > 0 && <span key={l.id} className={`u-bar ${l.id}`} style={{ height: `${(l.of(d) / max) * 100}%` }} />,
              )}
            </div>
            {/* Every other label, always including today. */}
            <span className="u-trend-label">{(days.length - 1 - i) % 2 === 0 ? `${parseDay(d.date).getMonth() + 1}/${parseDay(d.date).getDate()}` : ""}</span>
          </div>
        ))}
      </div>
    </section>
  );
}
