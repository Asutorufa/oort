import { useState } from "react";
import type { LogEntry } from "../hooks/useSession";
import type { Message, SessionDetail } from "../types";

const pct = (r: number) => `${(r * 100).toFixed(1)}%`;

function preview(m: Message): string {
  const p = m.content[0];
  if (!p) return "(empty)";
  switch (p.kind) {
    case "text":
      return p.text.slice(0, 80);
    case "tool_call":
      return `ToolUse ${p.name}${m.content.length > 1 ? ` +${m.content.length - 1}` : ""}`;
    case "tool_result":
      return `ToolResult ${p.name ?? p.tool_call_id}${p.is_error ? " ✗" : ""}`;
    case "thinking":
      return `Thinking… ${m.content.length > 1 ? `+${m.content.length - 1}` : ""}`;
    default:
      return p.kind;
  }
}

export function DebugPanel({ detail, log, connected }: { detail: SessionDetail; log: LogEntry[]; connected: boolean }) {
  const [view, setView] = useState<"raw" | "active">("active");
  const { usage, context } = detail;
  const ctxPct = context.tokens == null ? 0 : Math.min(1, context.tokens / context.max_tokens);
  const thrPct = context.compaction_threshold / context.max_tokens;

  return (
    <div className="debug">
      <section>
        <h3>State</h3>
        <table className="kv">
          <tbody>
            <tr><td>status</td><td><span className={`badge ${detail.status}`}>{detail.status}</span></td></tr>
            <tr><td>phase</td><td><code>{detail.phase.phase}</code>{detail.phase.phase === "halted" && ` (${detail.phase.reason})`}</td></tr>
            <tr><td>loop</td><td>{detail.active ? "running" : "stopped"} · SSE {connected ? "connected" : "disconnected"}</td></tr>
            <tr><td>provider</td><td>{detail.provider} / {detail.model}</td></tr>
            <tr><td>turns</td><td>{detail.turn} · {detail.usage.ledger.llm_calls} LLM calls</td></tr>
            <tr><td>queued steers</td><td>{detail.pending_steers}</td></tr>
          </tbody>
        </table>
      </section>

      <section>
        <h3>Tokens & prefix cache</h3>
        <table className="kv">
          <tbody>
            <tr><td>total in / out</td><td>{usage.ledger.total_input_tokens} / {usage.ledger.total_output_tokens}</td></tr>
            <tr><td>cache hit rate</td><td><b>{pct(usage.cache_hit_rate)}</b> <span className="muted">(Σ cached / Σ prompt)</span></td></tr>
          </tbody>
        </table>
        <table className="grid">
          <thead>
            <tr><th>turn</th><th>prompt</th><th>cached</th><th>hit</th><th>out</th></tr>
          </thead>
          <tbody>
            {usage.per_turn.map((t) => (
              <tr key={t.seq} title={`input_tokens=${t.input_tokens} cache_creation=${t.cache_creation_tokens} (msg #${t.seq})`}>
                <td>{t.turn}</td><td>{t.prompt_tokens}</td><td>{t.cache_read_tokens}</td>
                <td className={t.cache_hit_rate > 0.5 ? "good" : "bad"}>{pct(t.cache_hit_rate)}</td><td>{t.output_tokens}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section>
        <h3>Context</h3>
        <div className="meter" title={`compaction at ${context.compaction_threshold}`}>
          <div className="fill" style={{ width: `${ctxPct * 100}%` }} />
          <div className="threshold" style={{ left: `${thrPct * 100}%` }} />
        </div>
        <div className="muted">
          {context.tokens ?? "—"} / {context.max_tokens} tokens · compaction at {context.compaction_threshold}
        </div>
        {detail.checkpoints.length === 0 ? (
          <div className="muted">no compactions</div>
        ) : (
          <ul className="compactions">
            {detail.checkpoints.map((c) => (
              <li key={c.epoch}>
                <details>
                  <summary>epoch {c.epoch}: messages #0–#{c.up_to_seq - 1} summarized</summary>
                  <pre>{c.summary.content.map((p) => (p.kind === "text" ? p.text : "")).join("")}</pre>
                </details>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section>
        <h3>
          History view{" "}
          <button className={view === "raw" ? "toggle on" : "toggle"} onClick={() => setView("raw")}>Raw ({detail.raw.length})</button>
          <button className={view === "active" ? "toggle on" : "toggle"} onClick={() => setView("active")}>
            Active ({detail.active_view.messages.length})
          </button>
        </h3>
        <ol className="history">
          {view === "raw"
            ? detail.raw.map((m) => (
                <li key={m.seq} className={m.seq < detail.active_view.from_seq ? "compacted" : ""}>
                  <code>#{m.seq}</code> <b>{m.role}</b> {preview(m)}
                </li>
              ))
            : detail.active_view.messages.map((m, i) => {
                const isSummary = detail.checkpoints.length > 0 && i === 0;
                const seq = isSummary ? null : detail.active_view.from_seq + i - (detail.checkpoints.length > 0 ? 1 : 0);
                return (
                  <li key={i} className={isSummary ? "summary" : ""}>
                    <code>{isSummary ? "summary" : `#${seq}`}</code> <b>{m.role}</b> {preview(m)}
                  </li>
                );
              })}
        </ol>
      </section>

      <section>
        <h3>Event log</h3>
        <ol className="log">
          {log.slice(-60).reverse().map((e, i) => (
            <li key={`${e.seq}-${i}`}>
              <code>{e.seq}</code> {e.label}
            </li>
          ))}
        </ol>
      </section>
    </div>
  );
}
