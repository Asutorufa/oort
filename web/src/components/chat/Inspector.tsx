import { useEffect, useState } from "react";
import { errorText, sessions } from "../../api";
import type { SessionDetail, TurnUsage } from "../../api/types";
import type { LogEntry } from "../../hooks/useSessionStream";
import { DebugPanel } from "./DebugPanel";
import { useI18n, type Key } from "../../i18n";

/** Right-hand drawer with the internals: prefix cache, latency, compaction, raw history. */
const TRACE_STATUS: Record<string, Key> = { idle: "trace.done", running: "status.running", needs_approval: "status.needsApproval", halted: "status.halted" };

export function Inspector({
  detail,
  log,
  trace,
  onCloseTrace,
  onInspect,
  onCompact,
  onStop,
  onClose,
}: {
  detail: SessionDetail;
  log: LogEntry[];
  /** A subagent session to show instead of this one. */
  trace?: string | null;
  onCloseTrace?: () => void;
  onInspect: (t: TurnUsage) => void;
  onCompact: () => void;
  onStop: () => void;
  onClose: () => void;
}) {
  const { t, fmt } = useI18n();
  if (trace) return <TraceView id={trace} onBack={onCloseTrace} onClose={onClose} onInspect={onInspect} />;
  const turns = detail.usage.per_turn;
  const cached = turns.reduce((s, x) => s + x.cache_read_tokens, 0);
  const prompt = turns.reduce((s, x) => s + x.prompt_tokens, 0);
  const last = turns[turns.length - 1];
  return (
    <aside className="inspector" aria-label={t("insp.title")}>
      <header className="inspector-head">
        <h3>{t("insp.title")}</h3>
        <span className="spacer" />
        <button className="btn ghost icon" onClick={onClose} aria-label={t("common.close")}>
          ✕
        </button>
      </header>
      <div className="inspector-body">
        <section className="cache-card">
          <div className="cache-head">
            <span>{t("insp.prefixCache")}</span>
            <b>{fmt.percent(detail.usage.cache_hit_rate)}</b>
          </div>
          <div className="cache-bar" title={t("insp.cacheBar", { cached: fmt.number(cached), prompt: fmt.number(prompt) })}>
            <div className="hit" style={{ width: `${prompt ? (cached / prompt) * 100 : 0}%` }} />
          </div>
          <div className="cache-legend">
            <span>
              <i className="sw hit" /> {t("insp.cached", { n: fmt.number(cached) })}
            </span>
            <span>
              <i className="sw miss" /> {t("insp.fresh", { n: fmt.number(prompt - cached) })}
            </span>
          </div>
          {last && (
            <div className="stats">
              <div className="stat">
                <span className="muted">{t("insp.ttft")}</span>
                <b>{fmt.ms(last.ttft_ms)}</b>
              </div>
              <div className="stat">
                <span className="muted">{t("insp.total")}</span>
                <b>{fmt.ms(last.latency_ms)}</b>
              </div>
              <div className="stat">
                <span className="muted">{t("insp.lastHit")}</span>
                <b>{fmt.percent(last.cache_hit_rate)}</b>
              </div>
            </div>
          )}
          <div className="row">
            <button className="btn small" onClick={onCompact} disabled={detail.status === "halted"} title={t("insp.compactHint")}>
              {t("insp.compact")}
            </button>
            <button className="btn small ghost" onClick={onStop} disabled={!detail.active}>
              {t("insp.stopLoop")}
            </button>
          </div>
        </section>
        <DebugPanel detail={detail} log={log} onInspect={onInspect} />
      </div>
    </aside>
  );
}

/** Full trace of a subagent session: its messages, thinking, tool calls and cache statistics. */
function TraceView({ id, onBack, onClose, onInspect }: { id: string; onBack?: () => void; onClose: () => void; onInspect: (t: TurnUsage) => void }) {
  const [d, setD] = useState<SessionDetail | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const { t, fmt } = useI18n();
  useEffect(() => {
    let live = true;
    const load = () =>
      sessions
        .get(id)
        .then((x) => live && setD(x))
        .catch((e) => live && setErr(errorText(e)));
    load();
    const t = window.setInterval(load, 1500);
    return () => {
      live = false;
      window.clearInterval(t);
    };
  }, [id]);
  return (
    <aside className="inspector" aria-label={t("trace.title")}>
      <header className="inspector-head">
        {onBack && (
          <button className="btn ghost icon" onClick={onBack} aria-label={t("common.back")}>
            ←
          </button>
        )}
        <h3>{t("trace.title")}</h3>
        <span className="spacer" />
        <button className="btn ghost icon" onClick={onClose} aria-label={t("common.close")}>
          ✕
        </button>
      </header>
      <div className="inspector-body">
        {err && <div className="error-text">{err}</div>}
        {!d ? (
          <div className="muted">{t("common.loading")}</div>
        ) : (
          <>
            <div className="trace-meta">
              <b>{d.title}</b>
              <div className="muted">
                {t("trace.meta", {
                  depth: d.lineage.depth,
                  status: TRACE_STATUS[d.status] ? t(TRACE_STATUS[d.status]) : d.status,
                  turns: t("subagent.turns", { count: d.turn }),
                  cache: fmt.percent(d.usage.cache_hit_rate),
                })}
              </div>
            </div>
            <ol className="trace">
              {d.system_prompt && (
                <li className="trace-item system">
                  <details>
                    <summary>{t("trace.systemPrompt")}</summary>
                    <pre>{d.system_prompt}</pre>
                  </details>
                </li>
              )}
              {d.raw.map((m) => (
                <li key={m.seq} className={`trace-item ${m.role}`}>
                  <span className="trace-role">{m.role}</span>
                  {m.content.map((p, i) => {
                    if (p.kind === "text") return <pre key={i}>{p.text}</pre>;
                    if (p.kind === "thinking")
                      return (
                        <details key={i}>
                          <summary>{t("trace.thinking")}</summary>
                          <pre>{p.thinking}</pre>
                        </details>
                      );
                    if (p.kind === "tool_call")
                      return (
                        <details key={i}>
                          <summary>
                            {t("trace.toolCall")} <span className="tool-name">{p.name}</span>
                          </summary>
                          <pre>{JSON.stringify(p.arguments, null, 2)}</pre>
                        </details>
                      );
                    if (p.kind === "tool_result")
                      return (
                        <details key={i} open={p.is_error}>
                          <summary className={p.is_error ? "error-text" : ""}>
                            {t("trace.toolResult")} <span className="tool-name">{p.name}</span>
                          </summary>
                          <pre>{p.content}</pre>
                        </details>
                      );
                    return null;
                  })}
                </li>
              ))}
            </ol>
            <DebugPanel detail={d} log={[]} onInspect={onInspect} />
          </>
        )}
      </div>
    </aside>
  );
}
