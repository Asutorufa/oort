import { useMemo, useState } from "react";
import type { WidgetProps } from "./protocol";
import { Done, WidgetCard } from "./WidgetCard";

interface Column {
  key: string;
  label: string;
}
interface RowAction {
  id: string;
  label: string;
}
type Row = Record<string, unknown>;

const show = (v: unknown) => (v == null ? "" : typeof v === "object" ? JSON.stringify(v) : String(v));

/** Filterable, sortable table with per-row action buttons (each sends one widget answer). */
export function DataTable({ id, spec, response, actions, disabled }: WidgetProps) {
  const columns = (spec.data.columns as Column[]) ?? [];
  const rows = (spec.data.rows as Row[]) ?? [];
  const rowActions = (spec.data.row_actions as RowAction[]) ?? [];
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<{ key: string; dir: 1 | -1 } | null>(null);
  const [sent, setSent] = useState<{ action: string; index: number } | null>(null);
  const answered = (response?.payload as { action?: string; row_index?: number } | undefined) ?? (sent ? { action: sent.action, row_index: sent.index } : undefined);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = rows.map((r, index) => ({ r, index })).filter(({ r }) => !q || columns.some((c) => show(r[c.key]).toLowerCase().includes(q)));
    if (sort) {
      list.sort((a, b) => {
        const x = a.r[sort.key];
        const y = b.r[sort.key];
        const n = typeof x === "number" && typeof y === "number" ? x - y : show(x).localeCompare(show(y), undefined, { numeric: true });
        return n * sort.dir;
      });
    }
    return list;
  }, [rows, columns, query, sort]);

  const act = async (action: RowAction, row: Row, index: number) => {
    setSent({ action: action.id, index });
    await actions.respond(id, "data_table", { action: action.id, row_index: index, row });
  };

  return (
    <WidgetCard
      title={spec.title}
      kind="table"
      status={answered && <Done>{rowActions.find((a) => a.id === answered.action)?.label ?? answered.action}（{(answered.row_index ?? 0) + 1} 行目）</Done>}
    >
      {rows.length > 5 && <input className="table-filter" placeholder="絞り込み…" value={query} onChange={(e) => setQuery(e.target.value)} />}
      <div className="table-wrap">
        <table className="data-table">
          <thead>
            <tr>
              {columns.map((c) => (
                <th key={c.key} onClick={() => setSort((s) => (s?.key === c.key ? { key: c.key, dir: s.dir === 1 ? -1 : 1 } : { key: c.key, dir: 1 }))}>
                  {c.label}
                  <span className="sort">{sort?.key === c.key ? (sort.dir === 1 ? "▲" : "▼") : ""}</span>
                </th>
              ))}
              {rowActions.length > 0 && <th />}
            </tr>
          </thead>
          <tbody>
            {shown.map(({ r, index }) => (
              <tr key={index} className={answered?.row_index === index ? "chosen" : ""}>
                {columns.map((c) => (
                  <td key={c.key}>{show(r[c.key])}</td>
                ))}
                {rowActions.length > 0 && (
                  <td className="row-actions">
                    {rowActions.map((a) => (
                      <button key={a.id} className="btn small" disabled={disabled || !!answered} onClick={() => act(a, r, index)}>
                        {a.label}
                      </button>
                    ))}
                  </td>
                )}
              </tr>
            ))}
            {shown.length === 0 && (
              <tr>
                <td colSpan={columns.length + 1} className="muted">
                  該当する行がありません
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </WidgetCard>
  );
}
