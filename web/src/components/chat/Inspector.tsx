import { motion } from "motion/react";
import { useEffect, useState } from "react";
import { errorText, sessions } from "../../api";
import type { SessionDetail, TurnUsage } from "../../api/types";
import { ArrowLeftIcon, XIcon } from "../../brand/Icons";
import type { ConnState, LogEntry } from "../../hooks/useSessionStream";
import { useI18n, type Key } from "../../i18n";
import { spring } from "../../lib/motion";
import { HistoryTab } from "./InspectorHistory";
import { LogTab } from "./InspectorLog";
import { Overview } from "./InspectorOverview";

const TRACE_STATUS: Record<string, Key> = { idle: "trace.done", running: "status.running", needs_approval: "status.needsApproval", halted: "status.halted" };

type Tab = "overview" | "history" | "log";
const TABS: { id: Tab; label: Key }[] = [
  { id: "overview", label: "insp.tab.overview" },
  { id: "history", label: "debug.history" },
  { id: "log", label: "insp.tab.log" },
];

/** Right-hand drawer with the internals: a fixed summary, then overview, history and event log tabs. */
export function Inspector({
  detail,
  log,
  conn,
  trace,
  onCloseTrace,
  onInspect,
  onCompact,
  onStop,
  onClose,
}: {
  detail: SessionDetail;
  log: LogEntry[];
  conn: ConnState;
  /** A subagent session to show instead of this one. */
  trace?: string | null;
  onCloseTrace?: () => void;
  onInspect: (t: TurnUsage) => void;
  onCompact: () => void;
  onStop: () => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [tab, setTab] = useState<Tab>("overview");
  if (trace) return <TraceView id={trace} onBack={onCloseTrace} onClose={onClose} onInspect={onInspect} />;
  // A loop that is merely alive but idle has nothing to stop.
  const working = detail.active && (detail.status === "running" || detail.status === "needs_approval");
  return (
    <aside className="inspector" aria-label={t("insp.title")}>
      <header className="inspector-head">
        <h3>{t("insp.title")}</h3>
        {working && detail.status === "running" && !detail.last_error && <span className="insp-pulse" title={t("status.running")} />}
        <span className="spacer" />
        <button className="insp-icon" onClick={onClose} aria-label={t("common.close")}>
          <XIcon size={15} />
        </button>
      </header>
      <Summary detail={detail} working={working} onCompact={onCompact} onStop={onStop} />
      <div className="insp-tabs" role="tablist">
        {TABS.map((x) => (
          <button key={x.id} type="button" role="tab" id={`insp-tab-${x.id}`} aria-selected={tab === x.id} className={tab === x.id ? "on" : ""} onClick={() => setTab(x.id)}>
            {t(x.label)}
            {x.id === "history" && <span className="insp-count">{detail.raw.length}</span>}
            {tab === x.id && <motion.span layoutId="inspector-tab-indicator" className="insp-tab-indicator" transition={spring} />}
          </button>
        ))}
      </div>
      <div className="insp-panel" role="tabpanel" aria-labelledby={`insp-tab-${tab}`}>
        {tab === "overview" && (
          <div className="insp-scroll insp-overview">
            <Overview detail={detail} onInspect={onInspect} />
          </div>
        )}
        {tab === "history" && <HistoryTab detail={detail} />}
        {tab === "log" && <LogTab log={log} conn={conn} />}
      </div>
    </aside>
  );
}

/** Always visible: latency and cache of the last call, and how full the context is. */
function Summary({ detail, working, onCompact, onStop }: { detail: SessionDetail; working: boolean; onCompact: () => void; onStop: () => void }) {
  const { t, fmt } = useI18n();
  const { usage, context } = detail;
  const cached = usage.per_turn.reduce((s, x) => s + x.cache_read_tokens, 0);
  const prompt = usage.per_turn.reduce((s, x) => s + x.prompt_tokens, 0);
  const last = usage.per_turn[usage.per_turn.length - 1];
  const used = context.tokens == null ? 0 : Math.min(1, context.tokens / context.max_tokens);
  const threshold = Math.min(1, context.compaction_threshold / context.max_tokens);
  return (
    <div className="insp-summary">
      <div className="insp-metrics">
        <div>
          <span>{t("insp.cache")}</span>
          <b>
            {fmt.percent(usage.cache_hit_rate)}
            {last && <small title={t("insp.lastHit")}>{fmt.percent(last.cache_hit_rate)}</small>}
          </b>
        </div>
        <div>
          <span>{t("insp.ttft")}</span>
          <b>{fmt.ms(last?.ttft_ms)}</b>
        </div>
        <div>
          <span>{t("insp.total")}</span>
          <b>{fmt.ms(last?.latency_ms)}</b>
        </div>
      </div>
      <div>
        <div className="insp-bar" title={t("insp.cacheBar", { cached: fmt.number(cached), prompt: fmt.number(prompt) })}>
          <span style={{ width: `${prompt ? (cached / prompt) * 100 : 0}%` }} />
        </div>
        <div className="insp-legend">
          <span>{t("insp.cached", { n: fmt.number(cached) })}</span>
          <span>{t("insp.fresh", { n: fmt.number(prompt - cached) })}</span>
        </div>
      </div>
      <div>
        <div className="insp-context">
          <span className="insp-context-label">{t("debug.context")}</span>
          <span className="insp-context-value">
            {context.tokens == null ? "—" : fmt.number(context.tokens)} / {fmt.number(context.max_tokens)}
          </span>
          <span className="spacer" />
          <button className="insp-action" onClick={onCompact} disabled={detail.status === "halted"} title={t("insp.compactHint")}>
            {t("insp.compact")}
          </button>
          {working && (
            <button className="insp-action stop" onClick={onStop} title={t("insp.stopLoop")}>
              {t("insp.stop")}
            </button>
          )}
        </div>
        <div className="insp-meter" title={t("debug.compactAt", { at: fmt.number(context.compaction_threshold) })}>
          <div className="insp-bar">
            <span style={{ width: `${used * 100}%` }} />
          </div>
          <i style={{ left: `${threshold * 100}%` }} />
        </div>
      </div>
    </div>
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
          <button className="insp-icon" onClick={onBack} aria-label={t("common.back")}>
            <ArrowLeftIcon size={15} />
          </button>
        )}
        <h3>{t("trace.title")}</h3>
        <span className="spacer" />
        <button className="insp-icon" onClick={onClose} aria-label={t("common.close")}>
          <XIcon size={15} />
        </button>
      </header>
      <div className="insp-scroll insp-trace">
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
            <Overview detail={d} onInspect={onInspect} />
          </>
        )}
      </div>
    </aside>
  );
}
