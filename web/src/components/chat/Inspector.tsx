import { useEffect, useState } from "react";
import { errorText, sessions } from "../../api";
import type { SessionDetail, TurnUsage } from "../../api/types";
import type { LogEntry } from "../../hooks/useSessionStream";
import { fmtMs, pct } from "../common";
import { DebugPanel } from "./DebugPanel";

/** Right-hand drawer with the internals: prefix cache, latency, compaction, raw history. */
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
  if (trace) return <TraceView id={trace} onBack={onCloseTrace} onClose={onClose} onInspect={onInspect} />;
  const turns = detail.usage.per_turn;
  const cached = turns.reduce((s, t) => s + t.cache_read_tokens, 0);
  const prompt = turns.reduce((s, t) => s + t.prompt_tokens, 0);
  const last = turns[turns.length - 1];
  return (
    <aside className="inspector" aria-label="透視">
      <header className="inspector-head">
        <h3>透視</h3>
        <span className="spacer" />
        <button className="btn ghost icon" onClick={onClose} aria-label="閉じる">
          ✕
        </button>
      </header>
      <div className="inspector-body">
        <section className="cache-card">
          <div className="cache-head">
            <span>プレフィックスキャッシュ</span>
            <b>{pct(detail.usage.cache_hit_rate)}</b>
          </div>
          <div className="cache-bar" title={`cached ${cached} / prompt ${prompt}`}>
            <div className="hit" style={{ width: `${prompt ? (cached / prompt) * 100 : 0}%` }} />
          </div>
          <div className="cache-legend">
            <span>
              <i className="sw hit" /> キャッシュ {cached.toLocaleString()}
            </span>
            <span>
              <i className="sw miss" /> 新規 {(prompt - cached).toLocaleString()}
            </span>
          </div>
          {last && (
            <div className="stats">
              <div className="stat">
                <span className="muted">TTFT</span>
                <b>{fmtMs(last.ttft_ms)}</b>
              </div>
              <div className="stat">
                <span className="muted">総時間</span>
                <b>{fmtMs(last.latency_ms)}</b>
              </div>
              <div className="stat">
                <span className="muted">今回のヒット</span>
                <b>{pct(last.cache_hit_rate)}</b>
              </div>
            </div>
          )}
          <div className="row">
            <button className="btn small" onClick={onCompact} disabled={detail.status === "halted"} title="Epoch Compaction を次の LLM 呼び出しの前に実行">
              いま要約する（Compact）
            </button>
            <button className="btn small ghost" onClick={onStop} disabled={!detail.active}>
              ループを停止
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
    <aside className="inspector" aria-label="サブエージェントのトレース">
      <header className="inspector-head">
        {onBack && (
          <button className="btn ghost icon" onClick={onBack} aria-label="戻る">
            ←
          </button>
        )}
        <h3>サブエージェントのトレース</h3>
        <span className="spacer" />
        <button className="btn ghost icon" onClick={onClose} aria-label="閉じる">
          ✕
        </button>
      </header>
      <div className="inspector-body">
        {err && <div className="error-text">{err}</div>}
        {!d ? (
          <div className="muted">読み込み中…</div>
        ) : (
          <>
            <div className="trace-meta">
              <b>{d.title}</b>
              <div className="muted">
                深さ {d.lineage.depth} · {d.status} · {d.turn} ターン · キャッシュ {pct(d.usage.cache_hit_rate)}
              </div>
            </div>
            <ol className="trace">
              {d.system_prompt && (
                <li className="trace-item system">
                  <details>
                    <summary>System prompt</summary>
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
                          <summary>Thinking</summary>
                          <pre>{p.thinking}</pre>
                        </details>
                      );
                    if (p.kind === "tool_call")
                      return (
                        <details key={i}>
                          <summary>
                            ToolCall <span className="tool-name">{p.name}</span>
                          </summary>
                          <pre>{JSON.stringify(p.arguments, null, 2)}</pre>
                        </details>
                      );
                    if (p.kind === "tool_result")
                      return (
                        <details key={i} open={p.is_error}>
                          <summary className={p.is_error ? "error-text" : ""}>
                            ToolResult <span className="tool-name">{p.name}</span>
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
