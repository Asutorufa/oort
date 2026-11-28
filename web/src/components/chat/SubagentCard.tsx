import { useEffect, useState } from "react";
import type { ToolResult } from "../../api/types";
import type { SubagentLive } from "../../hooks/useSessionStream";
import { fmtMs } from "../common";
import { Markdown } from "./LazyMarkdown";

const PROFILE: Record<string, string> = { explorer: "探索", researcher: "リサーチ", reviewer: "レビュー" };

export function profileLabel(p: string) {
  return PROFILE[p] ?? p.replace(/^preset:/, "");
}

/** Strips the trailer `spawn_subagent` appends to its report. */
function splitReport(content: string): { body: string; meta: string | null } {
  const m = content.match(/\n\n\[subagent: ([^\]]+)\]\s*$/);
  return m ? { body: content.slice(0, m.index), meta: m[1] } : { body: content, meta: null };
}

/**
 * A delegated task: who works on it, what it is doing now, and (when done) its report.
 * "実行トレースを見る" opens the child session in the inspector.
 */
export function SubagentCard({
  task,
  profile,
  live,
  result,
  subSessionId,
  onTrace,
}: {
  task: string;
  profile: string;
  live?: SubagentLive;
  result?: ToolResult;
  subSessionId?: string;
  onTrace?: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [, tick] = useState(0);
  const running = !result && !!live && !live.done;
  useEffect(() => {
    if (!running) return;
    const t = window.setInterval(() => tick((n) => n + 1), 1000);
    return () => window.clearInterval(t);
  }, [running]);
  const interrupted = result?.is_error && /interrupt/i.test(result.content);
  const status = result ? (interrupted ? "中断" : result.is_error ? "エラー" : "完了") : running ? "実行中" : "待機中";
  const tone = result ? (result.is_error ? "danger" : "ok") : "accent";
  const report = result ? splitReport(result.content) : null;
  const elapsed = live ? (running ? Date.now() : live.lastAt) - live.startedAt : null;
  const sid = subSessionId ?? live?.subSessionId;

  return (
    <section className={`subagent ${running ? "running" : ""}`}>
      <header className="subagent-head">
        <span className={`sa-icon ${running ? "pulse-ring" : ""}`}>◇</span>
        <span className="sa-profile">{profileLabel(profile)}</span>
        <span className={`pill ${tone}`}>{status}</span>
        <span className="spacer" />
        {live && (
          <span className="muted">
            {live.turns} ターン · {fmtMs(elapsed)}
          </span>
        )}
      </header>
      <div className="sa-task">{task}</div>
      {running && (
        <div className="sa-now">
          <span className="pulse" />
          {live?.currentTool ? (
            <>
              <span className="tool-name">{live.currentTool.replace("__", " · ")}</span> を実行中
            </>
          ) : (
            "考えています…"
          )}
        </div>
      )}
      {report && (
        <div className="sa-report">
          <button className="link" onClick={() => setOpen(!open)}>
            {open ? "要約を閉じる" : "要約を表示"}
          </button>
          {open && (
            <div className="sa-summary">
              <Markdown text={report.body} />
            </div>
          )}
        </div>
      )}
      {sid && onTrace && (
        <button className="btn small ghost sa-trace" onClick={() => onTrace(sid)}>
          実行トレースを見る
        </button>
      )}
    </section>
  );
}
