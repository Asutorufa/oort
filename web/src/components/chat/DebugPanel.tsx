import { useState } from "react";
import type { Message, SessionDetail, TurnUsage } from "../../api/types";
import type { LogEntry } from "../../hooks/useSessionStream";
import { SegmentedControl } from "../ui";
import { useI18n } from "../../i18n";

function preview(m: Message): string {
  const p = m.content[0];
  if (!p) return "(empty)";
  const more = m.content.length > 1 ? ` +${m.content.length - 1}` : "";
  switch (p.kind) {
    case "text":
      return p.text.slice(0, 80);
    case "tool_call":
      return `ToolUse ${p.name}${more}`;
    case "tool_result":
      return `ToolResult ${p.name ?? p.tool_call_id}${p.is_error ? " ✗" : ""}${more}`;
    case "thinking":
      return `Thinking…${more}`;
    default:
      return p.kind;
  }
}

export function DebugPanel({
  detail,
  log,
  onInspect,
}: {
  detail: SessionDetail;
  log: LogEntry[];
  onInspect: (t: TurnUsage) => void;
}) {
  const [view, setView] = useState<"raw" | "active">("active");
  const { t, fmt } = useI18n();
  const { usage, context } = detail;
  const ctxPct = context.tokens == null ? 0 : Math.min(1, context.tokens / context.max_tokens);
  const thrPct = context.compaction_threshold / context.max_tokens;

  return (
    <div className="debug">
      <section>
        <h3>{t("debug.state")}</h3>
        <table className="kv">
          <tbody>
            <tr>
              <td>{t("debug.phase")}</td>
              <td>
                <code>{detail.phase.phase}</code>
                {detail.phase.phase === "halted" && ` (${detail.phase.reason})`}
              </td>
            </tr>
            <tr>
              <td>{t("debug.loop")}</td>
              <td>{detail.active ? t("debug.loopRunning") : t("debug.loopStopped")}</td>
            </tr>
            <tr>
              <td>{t("debug.turns")}</td>
              <td>{t("debug.turnsValue", { turns: detail.turn, count: usage.ledger.llm_calls })}</td>
            </tr>
            <tr>
              <td>{t("debug.tools")}</td>
              <td title={detail.tools.map((x) => x.name).join("\n")}>
                {t("debug.toolsFrozen", { count: detail.tools.length })}
                {detail.settings.mcp_servers.length > 0 && ` · MCP: ${detail.settings.mcp_servers.map((s) => s.prefix).join(", ")}`}
              </td>
            </tr>
            <tr>
              <td>{t("debug.queued")}</td>
              <td>{detail.pending_steers}</td>
            </tr>
          </tbody>
        </table>
      </section>

      <section>
        <h3>{t("debug.tokens")}</h3>
        <table className="kv">
          <tbody>
            <tr>
              <td>{t("debug.inOut")}</td>
              <td className="tabular">
                {fmt.number(usage.ledger.total_input_tokens)} / {fmt.number(usage.ledger.total_output_tokens)}
              </td>
            </tr>
            <tr>
              <td>{t("debug.hitRate")}</td>
              <td>
                <b>{fmt.percent(usage.cache_hit_rate)}</b> <span className="muted">(Σ cached / Σ prompt)</span>
              </td>
            </tr>
          </tbody>
        </table>
        <table className="grid">
          <thead>
            <tr>
              <th>{t("debug.col.turn")}</th>
              <th>{t("debug.col.prompt")}</th>
              <th>{t("debug.col.cached")}</th>
              <th>{t("debug.col.hit")}</th>
              <th>{t("debug.col.out")}</th>
              <th>{t("debug.col.ttft")}</th>
              <th>{t("debug.col.total")}</th>
            </tr>
          </thead>
          <tbody>
            {usage.per_turn.map((x) => (
              <tr key={x.seq} className="clickable" onClick={() => onInspect(x)} title={t("debug.inspectTurn")}>
                <td>{x.turn}</td>
                <td>{fmt.number(x.prompt_tokens)}</td>
                <td>{fmt.number(x.cache_read_tokens)}</td>
                <td className={x.cache_hit_rate > 0.5 ? "good" : "bad"}>{fmt.percent(x.cache_hit_rate)}</td>
                <td>{fmt.number(x.output_tokens)}</td>
                <td>{fmt.ms(x.ttft_ms)}</td>
                <td>{fmt.ms(x.latency_ms)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section>
        <h3>{t("debug.context")}</h3>
        <div className="meter" title={t("debug.compactAt", { at: fmt.number(context.compaction_threshold) })}>
          <div className="fill" style={{ width: `${ctxPct * 100}%` }} />
          <div className="threshold" style={{ left: `${thrPct * 100}%` }} />
        </div>
        <div className="muted">
          {t("debug.contextLine", {
            tokens: context.tokens == null ? "—" : fmt.number(context.tokens),
            max: fmt.number(context.max_tokens),
            at: fmt.number(context.compaction_threshold),
          })}
        </div>
        {detail.checkpoints.length === 0 ? (
          <div className="muted">{t("debug.noCompactions")}</div>
        ) : (
          <ul className="compactions">
            {detail.checkpoints.map((c) => (
              <li key={c.epoch}>
                <details>
                  <summary>
                    {t("debug.epoch", { epoch: c.epoch, last: c.up_to_seq - 1 })}
                  </summary>
                  <pre>{c.summary.content.map((p) => (p.kind === "text" ? p.text : "")).join("")}</pre>
                </details>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section>
        <h3>
          {t("debug.history")}{" "}
          <SegmentedControl
            size="sm"
            ariaLabel={t("debug.historyView")}
            value={view}
            onChange={(v) => setView(v)}
            options={[
              { value: "raw", label: t("debug.rawAll", { n: detail.raw.length }) },
              { value: "active", label: t("debug.active", { n: detail.active_view.messages.length }) },
            ]}
          />
        </h3>
        <ol className="history">
          {view === "raw"
            ? detail.raw.map((m) => (
                <li key={m.seq} className={m.seq < detail.active_view.from_seq ? "compacted" : ""}>
                  <code>#{m.seq}</code> <b>{m.role}</b> {preview(m)}
                </li>
              ))
            : detail.active_view.messages.map((m, i) => {
                const hasSummary = detail.checkpoints.length > 0;
                const isSummary = hasSummary && i === 0;
                const seq = detail.active_view.from_seq + i - (hasSummary ? 1 : 0);
                return (
                  <li key={i} className={isSummary ? "summary" : ""}>
                    <code>{isSummary ? t("debug.summary") : `#${seq}`}</code> <b>{m.role}</b> {preview(m)}
                  </li>
                );
              })}
        </ol>
      </section>

      <section>
        <h3>{t("debug.events")}</h3>
        <ol className="log">
          {log
            .slice(-80)
            .reverse()
            .map((e, i) => (
              <li key={`${e.seq}-${i}`}>
                <code>{e.seq ?? "·"}</code> {e.label}
              </li>
            ))}
        </ol>
      </section>
    </div>
  );
}
