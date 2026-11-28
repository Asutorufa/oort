import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { ContentPart, RawMessage, SessionDetail, SessionSummary, ToolCall, ToolResult } from "../../api/types";
import { ChevronDown } from "../../brand/Icons";
import { OortMark } from "../../brand/OortMark";
import type { Live, SubagentLive, ToolTimes } from "../../hooks/useSessionStream";
import { asWidget, parseResponse, responsesById, type WidgetActions } from "../../widgets/protocol";
import { Widget } from "../../widgets/WidgetRegistry";
import { fmtMs, JsonView } from "../common";
import { Markdown } from "./LazyMarkdown";
import { RowCard, SubagentCard } from "./SubagentCard";
import { LoopErrorCard } from "./LoopErrorCard";
import { Collapse } from "../ui";
import { AnimatePresence, motion } from "motion/react";
import { enter } from "../../lib/motion";
import { ApprovalCard, AskUserCard, McpConfiguredCard } from "./ToolCards";

const RENDER_WIDGET = "render_widget";

/** One user message, or one assistant turn (assistant + tool messages until the next user). */
type Group = { kind: "user"; msg: RawMessage } | { kind: "assistant"; msgs: RawMessage[] };

function groupMessages(raw: RawMessage[]): Group[] {
  const out: Group[] = [];
  for (const m of raw) {
    if (m.role === "user") out.push({ kind: "user", msg: m });
    else {
      const last = out[out.length - 1];
      if (last?.kind === "assistant") last.msgs.push(m);
      else out.push({ kind: "assistant", msgs: [m] });
    }
  }
  return out;
}

const textOf = (m: RawMessage) => m.content.map((p) => (p.kind === "text" ? p.text : "")).join("");

// ----- user bubbles --------------------------------------------------------------------------

const SKILL_BLOCK = /<(?:active_skill|skill) name="([^"]+)">[\s\S]*?<\/(?:active_skill|skill)>\s*|<skill_already_loaded name="([^"]+)" \/>\s*/g;
const REPORT_BLOCK = /<subagent_result skill="([^"]+)"([^>]*)>\n?([\s\S]*?)\n?<\/subagent_result>\s*/g;

/** Splits a user message into attached skills, delegated-skill reports and the typed text. */
function parseUserText(text: string) {
  const skills: string[] = [];
  const reports: { skill: string; error: boolean; body: string }[] = [];
  let rest = text.replace(REPORT_BLOCK, (_m, skill: string, attrs: string, body: string) => {
    reports.push({ skill, error: attrs.includes('error="true"'), body });
    return "";
  });
  rest = rest.replace(SKILL_BLOCK, (_m, a?: string, b?: string) => {
    skills.push((a ?? b)!);
    return "";
  });
  return { skills, reports, rest: rest.trim() };
}

function SkillReport({ skill, error, body }: { skill: string; error: boolean; body: string }) {
  return (
    <RowCard state={error ? "error" : "done"} title={`/${skill}`} subject="スキルをサブエージェントで実行しました" meta={error ? "エラー" : undefined}>
      <div className="rowcard-section">
        <div className="rowcard-label">{error ? "エラー" : "結果"}</div>
        <Markdown text={body} />
      </div>
    </RowCard>
  );
}

/**
 * One assistant turn: the brand mark appears once at the top, and every step of the turn
 * (thinking, tools, subagents, approvals, the answer) stacks in a single column next to it.
 */
function Turn({ mark = true, live, children }: { mark?: boolean; live?: boolean; children: ReactNode }) {
  return (
    <div className={`row-assistant ${live ? "live" : ""}`}>
      <div className="avatar">{mark && <OortMark size={18} />}</div>
      <div className="assistant-body">{children}</div>
    </div>
  );
}

function UserBubble({ msg, widgetTitles }: { msg: RawMessage; widgetTitles: Map<string, string> }) {
  const text = textOf(msg);
  const resp = parseResponse(text);
  if (resp) {
    const what = widgetTitles.get(resp.id);
    const label =
      resp.type === "interactive_form" ? "フォームに回答しました" : resp.type === "data_table" ? `「${(resp.payload as { action?: string })?.action ?? "操作"}」を選択しました` : "カードに応答しました";
    return (
      <div className="row-user">
        <div className="bubble user response" title={text}>
          {label}
          {what && <span className="muted"> · {what}</span>}
        </div>
      </div>
    );
  }
  const { skills, reports, rest } = parseUserText(text);
  return (
    <>
      {reports.map((r, i) => (
        <Turn key={i} mark={false}>
          <SkillReport {...r} />
        </Turn>
      ))}
      {(rest || skills.length > 0) && (
        <div className="row-user">
          <div className="bubble user">
            {skills.length > 0 && (
              <div className="skill-chips">
                {skills.map((k, i) => (
                  <span key={i} className="skill-chip">
                    /{k}
                  </span>
                ))}
              </div>
            )}
            {rest && <div className="plain">{rest}</div>}
          </div>
        </div>
      )}
    </>
  );
}

// ----- activity row --------------------------------------------------------------------------

function ToolLine({ call, result, running, ms }: { call: ToolCall; result?: ToolResult; running: boolean; ms?: number }) {
  const state = result ? (result.is_error ? "error" : "ok") : running ? "running" : "pending";
  return (
    <details className={`tool-line ${state}`}>
      <summary>
        <span className={`state-dot ${state}`} />
        <span className="tool-name">{call.name.replace("__", " · ")}</span>
        <span className="muted">{ms != null ? fmtMs(ms) : state === "running" ? "実行中…" : state === "pending" ? "待機中" : ""}</span>
      </summary>
      <div className="tool-detail">
        <div className="label">入力</div>
        <JsonView value={call.arguments} />
        {result && (
          <>
            <div className="label">{result.is_error ? "エラー" : "結果"}</div>
            <pre className={result.is_error ? "error-text" : ""}>{result.content}</pre>
          </>
        )}
      </div>
    </details>
  );
}

function ActivityRow({
  thinking,
  calls,
  results,
  running,
  toolTimes,
  latencyMs,
}: {
  thinking: string[];
  calls: ToolCall[];
  results: Map<string, ToolResult>;
  running: Set<string>;
  toolTimes: ToolTimes;
  latencyMs: number;
}) {
  const [open, setOpen] = useState(false);
  if (thinking.length === 0 && calls.length === 0) return null;
  const toolMs = calls.reduce((s, c) => s + (toolTimes[c.id]?.ms ?? 0), 0);
  const total = latencyMs + toolMs;
  const parts = [thinking.length > 0 ? "思考して" : "", calls.length > 0 ? `${calls.length} 件のツールを実行` : "思考しました"].join("");
  const failed = calls.filter((c) => results.get(c.id)?.is_error).length;
  const active = calls.some((c) => running.has(c.id));
  return (
    <div className={`activity ${open ? "open" : ""}`}>
      <button className="activity-row" onClick={() => setOpen(!open)}>
        {active && <span className="pulse" />}
        <span>{parts}</span>
        {total > 0 && <span className="muted">({fmtMs(total)})</span>}
        {failed > 0 && <span className="pill danger">{failed} 件エラー</span>}
        <span className="chev">
          <ChevronDown size={13} />
        </span>
      </button>
      <Collapse open={open}>
        <div className="activity-body">
          {thinking.map((t, i) => (
            <div key={i} className="thought">
              <div className="label">思考</div>
              <pre>{t || "（非公開）"}</pre>
            </div>
          ))}
          {calls.map((c) => (
            <ToolLine key={c.id} call={c} result={results.get(c.id)} running={running.has(c.id)} ms={toolTimes[c.id]?.ms} />
          ))}
        </div>
      </Collapse>
    </div>
  );
}

// ----- assistant turn ------------------------------------------------------------------------

interface ThreadProps {
  detail: SessionDetail;
  live: Live;
  toolTimes: ToolTimes;
  actions: WidgetActions;
  subagents: Record<string, SubagentLive>;
  children: SessionSummary[];
  onTrace: (subSessionId: string) => void;
  onAnswer: (callId: string, content: string) => Promise<void>;
  onStartWithMcp: (serverId: string) => Promise<void>;
  onRetry: () => Promise<void>;
  onCancelAutoRetry: () => Promise<void>;
}

/** Calls shown as their own cards (not in the activity line). */
const CARD_TOOLS = new Set([RENDER_WIDGET, "spawn_subagent", "ask_user", "update_plan", "configure_mcp_server"]);

export function Thread({ detail, live, toolTimes, actions, subagents, children, onTrace, onAnswer, onStartWithMcp, onRetry, onCancelAutoRetry }: ThreadProps) {
  const box = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const groups = useMemo(() => groupMessages(detail.raw), [detail.raw]);
  const results = useMemo(() => {
    const m = new Map<string, ToolResult>();
    for (const msg of detail.raw) for (const p of msg.content) if (p.kind === "tool_result") m.set(p.tool_call_id, p);
    return m;
  }, [detail.raw]);
  const responses = useMemo(() => responsesById(detail.raw), [detail.raw]);
  const widgetTitles = useMemo(() => {
    const m = new Map<string, string>();
    for (const msg of detail.raw)
      for (const p of msg.content) if (p.kind === "tool_call" && p.name === RENDER_WIDGET) m.set(p.id, asWidget(p.arguments)?.title ?? "");
    return m;
  }, [detail.raw]);
  const userTextsAfter = (seq: number) => detail.raw.filter((m) => m.role === "user" && m.seq > seq).map(textOf);
  const latency = new Map(detail.usage.per_turn.map((t) => [t.seq, t.latency_ms ?? 0]));
  const pending = new Set(detail.pending_approvals.map((a) => a.call.id));
  const pendingInput = new Set(detail.pending_approvals.filter((a) => a.kind === "input").map((a) => a.call.id));
  const childByCall = new Map(children.filter((c) => c.lineage.parent_tool_call_id).map((c) => [c.lineage.parent_tool_call_id!, c.id]));
  const running = new Set(live.runningTools.map((c) => c.id));
  const cutAt = new Set(detail.checkpoints.map((c) => c.up_to_seq));
  const failure = detail.last_error ?? null;
  const busy = detail.status === "running" && !failure;

  useEffect(() => {
    const el = box.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [detail.raw.length, live.text.length, live.thinking.length, detail.pending_approvals.length]);

  const assistantParts = (msgs: RawMessage[]) => {
    const parts: ContentPart[] = msgs.filter((m) => m.role === "assistant").flatMap((m) => m.content);
    const thinking = parts.flatMap((p) => (p.kind === "thinking" ? [p.thinking] : []));
    const calls = parts.flatMap((p) => (p.kind === "tool_call" && !CARD_TOOLS.has(p.name) ? [p as ToolCall] : []));
    const latencyMs = msgs.reduce((s, m) => s + (latency.get(m.seq) ?? 0), 0);
    return (
      <>
          <ActivityRow thinking={thinking} calls={calls} results={results} running={running} toolTimes={toolTimes} latencyMs={latencyMs} />
          {msgs.map((m) =>
            m.content.map((p, i) => {
              if (p.kind === "text" && m.role === "assistant") return p.text.trim() ? <Markdown key={`${m.seq}-${i}`} text={p.text} /> : null;
              if (p.kind === "tool_call" && p.name === "spawn_subagent") {
                const a = (p.arguments ?? {}) as { task?: string; profile?: string };
                return (
                  <SubagentCard
                    key={`${m.seq}-${i}`}
                    task={a.task ?? ""}
                    profile={a.profile ?? "explorer"}
                    live={subagents[p.id]}
                    result={results.get(p.id)}
                    subSessionId={childByCall.get(p.id)}
                    onTrace={onTrace}
                  />
                );
              }
              if (p.kind === "tool_call" && p.name === "ask_user") {
                return (
                  <AskUserCard
                    key={`${m.seq}-${i}`}
                    call={p}
                    pending={pendingInput.has(p.id)}
                    result={results.get(p.id)}
                    onAnswer={(c) => onAnswer(p.id, c)}
                  />
                );
              }
              if (p.kind === "tool_call" && p.name === "configure_mcp_server") {
                const r = results.get(p.id);
                return r && !r.is_error ? <McpConfiguredCard key={`${m.seq}-${i}`} result={r} onStart={onStartWithMcp} /> : null;
              }
              if (p.kind === "tool_call" && p.name === RENDER_WIDGET) {
                const spec = asWidget(p.arguments);
                if (!spec) return null;
                return (
                  <Widget
                    key={`${m.seq}-${i}`}
                    id={p.id}
                    spec={spec}
                    response={responses.get(p.id)}
                    pendingApproval={pending.has(p.id)}
                    result={results.get(p.id)}
                    laterUserTexts={userTextsAfter(m.seq)}
                    actions={actions}
                    disabled={detail.status === "halted"}
                  />
                );
              }
              return null;
            }),
          )}
      </>
    );
  };

  const otherApprovals = detail.pending_approvals.filter((a) => a.call.name !== RENDER_WIDGET && a.kind === "approval");
  // Subagents started by skills (`skill:` ids) have no tool call in this history: show them
  // while they run; their report arrives as a message.
  const reportsIn = new Set(detail.raw.filter((m) => m.role === "user").flatMap((m) => [...textOf(m).matchAll(/session="([^"]+)"/g)].map((x) => x[1])));
  const skillRuns = Object.entries(subagents).filter(([id, s]) => id.startsWith("skill:") && !reportsIn.has(s.subSessionId));

  const showLive = !!(live.text || live.thinking || (busy && !live.runningTools.length));
  const hasTail = otherApprovals.length > 0 || skillRuns.length > 0 || showLive || !!failure;
  const tailInLast = groups[groups.length - 1]?.kind === "assistant";
  // Steps that belong to the turn in progress: they join the last assistant turn (one mark).
  const tail = (
    <>
      {otherApprovals.map((pa) => (
        <motion.div key={pa.call.id} variants={enter} initial="hidden" animate="shown">
          <ApprovalCard pending={pa} onApprove={() => actions.approve(pa.call.id)} onDeny={(reason) => actions.deny(pa.call.id, reason)} />
        </motion.div>
      ))}
      {skillRuns.map(([id, s]) => (
        <SubagentCard
          key={id}
          task={children.find((c) => c.id === s.subSessionId)?.title ?? "スキルを実行中"}
          profile={s.profile}
          live={s}
          subSessionId={s.subSessionId}
          onTrace={onTrace}
        />
      ))}
      {failure && !showLive && (
        <motion.div variants={enter} initial="hidden" animate="shown">
          <LoopErrorCard error={failure} onRetry={onRetry} onCancelAuto={onCancelAutoRetry} />
        </motion.div>
      )}
      {showLive && (
        <>
          {live.thinking && !live.text && <div className="activity-row ghost">思考中…</div>}
          {live.text ? (
            <Markdown text={live.text} />
          ) : (
            !live.thinking && (
              <div className="typing" aria-label="応答を待っています">
                <span />
                <span />
                <span />
              </div>
            )
          )}
        </>
      )}
    </>
  );

  return (
    <div
      className="thread"
      ref={box}
      onScroll={(e) => {
        const el = e.currentTarget;
        stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
      }}
    >
      <div className="thread-inner">
        <AnimatePresence initial={false}>
        {groups.map((g, i) => {
          const isLast = i === groups.length - 1;
          return (
            <motion.div key={i} className="group" variants={enter} initial="hidden" animate="shown">
              {g.kind === "user" && cutAt.has(g.msg.seq) && <div className="epoch">ここまでの会話は要約されています</div>}
              {g.kind === "user" ? (
                <UserBubble msg={g.msg} widgetTitles={widgetTitles} />
              ) : (
                <Turn live={isLast && showLive}>
                  {assistantParts(g.msgs)}
                  {isLast && tail}
                </Turn>
              )}
            </motion.div>
          );
        })}
        </AnimatePresence>
        {!tailInLast && hasTail && <Turn live={showLive}>{tail}</Turn>}
      </div>
    </div>
  );
}
