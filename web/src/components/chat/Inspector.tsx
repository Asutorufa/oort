import type { SessionDetail, TurnUsage } from "../../api/types";
import type { LogEntry } from "../../hooks/useSessionStream";
import { fmtMs, pct } from "../common";
import { DebugPanel } from "./DebugPanel";

/** Right-hand drawer with the internals: prefix cache, latency, compaction, raw history. */
export function Inspector({
  detail,
  log,
  onInspect,
  onCompact,
  onStop,
  onClose,
}: {
  detail: SessionDetail;
  log: LogEntry[];
  onInspect: (t: TurnUsage) => void;
  onCompact: () => void;
  onStop: () => void;
  onClose: () => void;
}) {
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
