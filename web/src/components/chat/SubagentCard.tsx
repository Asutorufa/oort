import { useEffect, useState, type ReactNode } from "react";
import type { ToolResult } from "../../api/types";
import { AlertIcon, CheckIcon, ChevronDown, Spinner } from "../../brand/Icons";
import type { SubagentLive } from "../../hooks/useSessionStream";
import { fmtMs } from "../common";
import { Collapse } from "../ui";
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

export type RowState = "running" | "waiting" | "done" | "error";

/**
 * A one-line (44px) card that expands in place: state icon, title, one-line subject on the left;
 * tabular meta and quiet actions on the right. Used for subagents and delegated skills.
 */
export function RowCard({
  state,
  title,
  subject,
  meta,
  actions,
  children,
}: {
  state: RowState;
  title: string;
  subject?: string;
  meta?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const expandable = children != null;
  return (
    <section className={`rowcard ${state} ${open ? "open" : ""}`}>
      <div
        className="rowcard-head"
        role={expandable ? "button" : undefined}
        tabIndex={expandable ? 0 : undefined}
        aria-expanded={expandable ? open : undefined}
        onClick={() => expandable && setOpen(!open)}
        onKeyDown={(e) => expandable && (e.key === "Enter" || e.key === " ") && (e.preventDefault(), setOpen(!open))}
      >
        <span className="rowcard-icon">
          {state === "running" ? <Spinner /> : state === "done" ? <CheckIcon size={14} stroke={2} /> : state === "error" ? <AlertIcon size={14} /> : <span className="rowcard-wait" />}
        </span>
        <span className="rowcard-title">{title}</span>
        {subject && <span className="rowcard-subject">{subject}</span>}
        <span className="rowcard-meta">{meta}</span>
        {actions && (
          <span className="rowcard-actions" onClick={(e) => e.stopPropagation()}>
            {actions}
          </span>
        )}
        {expandable && (
          <span className="rowcard-chev">
            <ChevronDown size={14} />
          </span>
        )}
      </div>
      {expandable && (
        <Collapse open={open}>
          <div className="rowcard-body">{children}</div>
        </Collapse>
      )}
    </section>
  );
}

/**
 * A delegated task: who works on it, what it is doing now and, once done, its report (shown when
 * the card is expanded). "トレース" opens the child session in the inspector.
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
  const [, tick] = useState(0);
  const running = !result && !!live && !live.done;
  useEffect(() => {
    if (!running) return;
    const t = window.setInterval(() => tick((n) => n + 1), 1000);
    return () => window.clearInterval(t);
  }, [running]);
  const interrupted = result?.is_error && /interrupt|cancel/i.test(result.content);
  const state: RowState = result ? (result.is_error ? "error" : "done") : running ? "running" : "waiting";
  const report = result ? splitReport(result.content) : null;
  const elapsed = live ? (running ? Date.now() : live.lastAt) - live.startedAt : null;
  const sid = subSessionId ?? live?.subSessionId;
  const firstLine = task.split("\n").find((l) => l.trim()) ?? task;
  const doing = running ? (live?.currentTool ? `${live.currentTool.replace("__", " · ")} を実行中` : "考えています") : null;

  const meta = (
    <>
      {interrupted ? "中断" : result?.is_error ? "エラー" : doing}
      {(interrupted || result?.is_error || doing) && live ? " · " : ""}
      {live ? `${live.turns} ターン · ${fmtMs(elapsed)}` : !result ? "待機中" : ""}
    </>
  );

  return (
    <RowCard
      state={state}
      title={profileLabel(profile)}
      subject={firstLine}
      meta={meta}
      actions={
        sid && onTrace ? (
          <button className="btn ghost tiny-btn" onClick={() => onTrace(sid)} title="実行トレースを開く">
            トレース
          </button>
        ) : undefined
      }
    >
      <div className="rowcard-section">
        <div className="rowcard-label">タスク</div>
        <div className="rowcard-task">{task}</div>
      </div>
      {report && (
        <div className="rowcard-section">
          <div className="rowcard-label">{result?.is_error ? "エラー" : "結果"}</div>
          <Markdown text={report.body} />
        </div>
      )}
    </RowCard>
  );
}
