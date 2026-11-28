import { Area, AreaChart, CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { token, useTheme } from "../theme/ThemeProvider";
import type { WidgetProps } from "./protocol";
import { WidgetCard } from "./WidgetCard";

interface Kpi {
  label: string;
  value: string | number;
  delta?: string | number;
}
interface Series {
  name: string;
  points: { x: string | number; y: number }[];
}

/** KPI cards (max 3) and an area/line chart; colors follow the active theme. */
export default function MetricsChart({ spec }: WidgetProps) {
  const { theme, mode } = useTheme();
  const kpis = ((spec.data.kpis as Kpi[]) ?? []).slice(0, 3);
  const series = (spec.data.series as Series[]) ?? [];
  const kind = spec.data.kind === "line" ? "line" : "area";
  // Recharts needs concrete colors; re-read the tokens when the theme changes.
  const c = { theme, mode, c1: token("--chart-1"), c2: token("--chart-2"), grid: token("--grid"), muted: token("--muted") };
  const colors = [c.c1, c.c2, token("--orbit-dot")];
  const xs = Array.from(new Set(series.flatMap((s) => s.points.map((p) => String(p.x)))));
  const rows = xs.map((x) => Object.fromEntries([["x", x], ...series.map((s) => [s.name, s.points.find((p) => String(p.x) === x)?.y ?? null])]));
  const Chart = kind === "line" ? LineChart : AreaChart;

  return (
    <WidgetCard title={spec.title} kind="metrics">
      {kpis.length > 0 && (
        <div className="kpis">
          {kpis.map((k, i) => {
            const d = String(k.delta ?? "");
            return (
              <div key={i} className="kpi">
                <span className="kpi-label">{k.label}</span>
                <span className="kpi-value">{k.value}</span>
                {d && <span className={`kpi-delta ${d.startsWith("-") ? "down" : "up"}`}>{d}</span>}
              </div>
            );
          })}
        </div>
      )}
      {rows.length > 0 && (
        <div className="chart">
          <ResponsiveContainer width="100%" height={200}>
            <Chart data={rows} margin={{ top: 8, right: 8, bottom: 0, left: -16 }}>
              <defs>
                {series.map((s, i) => (
                  <linearGradient key={s.name} id={`g-${i}`} x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stopColor={colors[i % 3]} stopOpacity={0.28} />
                    <stop offset="100%" stopColor={colors[i % 3]} stopOpacity={0} />
                  </linearGradient>
                ))}
              </defs>
              <CartesianGrid stroke={c.grid} vertical={false} />
              <XAxis dataKey="x" tick={{ fill: c.muted, fontSize: 11 }} axisLine={false} tickLine={false} />
              <YAxis tick={{ fill: c.muted, fontSize: 11 }} axisLine={false} tickLine={false} />
              <Tooltip contentStyle={{ background: token("--bg-surface"), border: `1px solid ${token("--border")}`, borderRadius: 10, fontSize: 12 }} />
              {series.map((s, i) =>
                kind === "line" ? (
                  <Line key={s.name} type="monotone" dataKey={s.name} stroke={colors[i % 3]} strokeWidth={2} dot={false} />
                ) : (
                  <Area key={s.name} type="monotone" dataKey={s.name} stroke={colors[i % 3]} strokeWidth={2} fill={`url(#g-${i})`} />
                ),
              )}
            </Chart>
          </ResponsiveContainer>
        </div>
      )}
    </WidgetCard>
  );
}
