import { useId } from "react";
import { Area, AreaChart, CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { token, useTheme } from "../theme/ThemeProvider";
import type { WidgetProps } from "./protocol";
import { WidgetCard } from "./WidgetCard";

interface Kpi {
  label: string;
  value: string | number;
  delta?: string | number;
}
interface Point {
  x: string;
  y: number;
}
interface RawSeries {
  name?: string;
  points?: unknown[];
}

function normalizePoints(raw: unknown, fallbackLabels?: string[]): Point[] {
  if (!Array.isArray(raw) || raw.length === 0) return [];

  // Format 1: flat alternating [x, y, x, y, ...] (e.g. ["11-01", 3950, "11-05", 3880])
  if (
    raw.length >= 2 &&
    raw.length % 2 === 0 &&
    typeof raw[0] === "string" &&
    typeof raw[1] === "number"
  ) {
    const out: Point[] = [];
    for (let i = 0; i < raw.length; i += 2) {
      const y = Number(raw[i + 1]);
      if (Number.isFinite(y)) out.push({ x: String(raw[i]), y });
    }
    if (out.length > 0) return out;
  }

  // Format 2: plain array of numbers [3950, 3880, ...]
  if (raw.every((item) => typeof item === "number" || (typeof item === "string" && item.trim() !== "" && Number.isFinite(Number(item))))) {
    return raw
      .map((item, i) => ({
        x: fallbackLabels?.[i] !== undefined ? String(fallbackLabels[i]) : String(i + 1),
        y: Number(item),
      }))
      .filter((p) => Number.isFinite(p.y));
  }

  // Format 3: array of [x, y] tuples or { x, y } / { label, value } objects
  const out: Point[] = [];
  for (let i = 0; i < raw.length; i++) {
    const item = raw[i];
    if (Array.isArray(item) && item.length >= 2) {
      const y = Number(item[1]);
      if (Number.isFinite(y)) out.push({ x: String(item[0] ?? i + 1), y });
    } else if (item && typeof item === "object") {
      const obj = item as Record<string, unknown>;
      const rawX = obj.x ?? obj.label ?? obj.date ?? obj.name ?? fallbackLabels?.[i] ?? i + 1;
      const rawY = obj.y ?? obj.value ?? obj.v;
      const y = Number(rawY);
      if (rawX !== undefined && Number.isFinite(y)) {
        out.push({ x: String(rawX), y });
      }
    }
  }
  return out;
}

/** KPI cards (max 3) and an area/line chart; colors follow the active theme. */
export default function MetricsChart({ spec }: WidgetProps) {
  const { theme, mode } = useTheme();
  const gradId = useId().replace(/[^a-zA-Z0-9]/g, "");
  const kpis = ((spec.data.kpis as Kpi[]) ?? []).slice(0, 3);
  const rawLabels = Array.isArray(spec.data.labels)
    ? (spec.data.labels as unknown[]).map(String)
    : Array.isArray(spec.data.x)
      ? (spec.data.x as unknown[]).map(String)
      : undefined;
  const rawSeries = (spec.data.series as RawSeries[]) ?? [];
  const series = rawSeries
    .map((s, idx) => ({
      name: s.name || `Series ${idx + 1}`,
      points: normalizePoints(s.points, rawLabels),
    }))
    .filter((s) => s.points.length > 0);
  const kind = spec.data.kind === "line" ? "line" : "area";
  // Recharts needs concrete colors; re-read the tokens when the theme changes.
  const c = { theme, mode, c1: token("--chart-1"), c2: token("--chart-2"), grid: token("--grid"), muted: token("--muted") };
  const colors = [c.c1, c.c2, token("--orbit-dot")];
  const xs = Array.from(new Set(series.flatMap((s) => s.points.map((p) => p.x))));
  const rows = xs.map((x) =>
    Object.fromEntries([["x", x], ...series.map((s) => [s.name, s.points.find((p) => p.x === x)?.y ?? null])]),
  );
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
                  <linearGradient key={s.name} id={`${gradId}-${i}`} x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stopColor={colors[i % 3]} stopOpacity={0.28} />
                    <stop offset="100%" stopColor={colors[i % 3]} stopOpacity={0} />
                  </linearGradient>
                ))}
              </defs>
              <CartesianGrid stroke={c.grid} vertical={false} />
              <XAxis dataKey="x" tick={{ fill: c.muted, fontSize: 11 }} axisLine={false} tickLine={false} />
              <YAxis tick={{ fill: c.muted, fontSize: 11 }} axisLine={false} tickLine={false} domain={["auto", "auto"]} />
              <Tooltip
                contentStyle={{
                  background: token("--bg-elevated") || token("--bg-surface"),
                  border: `1px solid ${token("--border-default") || token("--border")}`,
                  borderRadius: 10,
                  fontSize: 12,
                }}
              />
              {series.map((s, i) =>
                kind === "line" ? (
                  <Line key={s.name} type="monotone" dataKey={s.name} stroke={colors[i % 3]} strokeWidth={2} dot={false} />
                ) : (
                  <Area key={s.name} type="monotone" dataKey={s.name} stroke={colors[i % 3]} strokeWidth={2} fill={`url(#${gradId}-${i})`} />
                ),
              )}
            </Chart>
          </ResponsiveContainer>
        </div>
      )}
    </WidgetCard>
  );
}
