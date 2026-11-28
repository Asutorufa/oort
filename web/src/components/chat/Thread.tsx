import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { RawMessage, SessionDetail, SessionSummary, ToolResult } from "../../api/types";
import { OortMark, type MarkState } from "../../brand/OortMark";
import type { Live, SubagentLive, ToolTimes } from "../../hooks/useSessionStream";
import { asWidget, parseResponse, responsesById, type WidgetActions } from "../../widgets/protocol";
import { Widget } from "../../widgets/WidgetRegistry";
import { Markdown } from "./LazyMarkdown";
import { RowCard, SubagentCard } from "./SubagentCard";
import { LoopErrorCard } from "./LoopErrorCard";
import { AnimatePresence, motion } from "motion/react";
import { enter } from "../../lib/motion";
import { ApprovalCard, AskUserCard, McpConfiguredCard } from "./ToolCards";
import { useT } from "../../i18n";
import { buildBlocks, fromLive, fromRaw, segments, type SourceMessage, type TurnBlock } from "./blocks";
import { WorkSegment, type Folds } from "./WorkSegment";

const RENDER_WIDGET = "render_widget";

/** One user message, or one assistant turn (assistant + tool messages until the next user). */
type Group = { kind: "user"; msg: RawMessage } | { kind: "assistant"; msgs: SourceMessage[] };

function groupMessages(raw: RawMessage[], live: Live): Group[] {
  const out: Group[] = [];
  const assistant = () => {
    const last = out[out.length - 1];
    if (last?.kind === "assistant") return last;
    const next: Group = { kind: "assistant", msgs: [] };
    out.push(next);
    return next;
  };
  for (const m of raw) {
    if (m.role === "user") out.push({ kind: "user", msg: m });
    else assistant().msgs.push(fromRaw(m));
  }
  const tail = raw[raw.length - 1];
  const turnEnded = tail?.role === "assistant" && !tail.content.some((p) => p.kind === "tool_call");
  for (const step of live.steps) {
    // Already in the snapshot.
    if (step.seq < raw.length) continue;
    // A new user message is on its way: do not attach its reply to the previous turn.
    if (step.seq > raw.length && turnEnded && out[out.length - 1]?.kind === "assistant") out.push({ kind: "assistant", msgs: [] });
    assistant().msgs.push(fromLive(step));
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
  const t = useT();
  return (
    <RowCard state={error ? "error" : "done"} title={`/${skill}`} subject={t("thread.skillReport")} meta={error ? t("common.error") : undefined}>
      <div className="rowcard-section">
        <div className="rowcard-label">{error ? t("common.error") : t("thread.result")}</div>
        <Markdown text={body} />
      </div>
    </RowCard>
  );
}

/**
 * One assistant turn: the brand mark appears once at the top, and every step of the turn
 * (thinking, tools, subagents, approvals, the answer) stacks in a single column next to it.
 * The mark turns while the agent works.
 */
function Turn({ mark = true, state = "idle", children }: { mark?: boolean; state?: MarkState; children: ReactNode }) {
  const t = useT();
  const working = state !== "idle";
  return (
    <div className={`row-assistant ${working ? "live" : ""}`}>
      <div className="avatar" role={working ? "status" : undefined} aria-label={working ? t("thread.waiting") : undefined}>
        {mark && <OortMark size={18} state={state} />}
      </div>
      <div className="assistant-body">{children}</div>
    </div>
  );
}

function UserBubble({ msg, widgetTitles }: { msg: RawMessage; widgetTitles: Map<string, string> }) {
  const t = useT();
  const text = textOf(msg);
  const resp = parseResponse(text);
  if (resp) {
    const what = widgetTitles.get(resp.id);
    const label =
      resp.type === "interactive_form"
        ? t("thread.formAnswered")
        : resp.type === "data_table"
          ? t("thread.tableChose", { action: (resp.payload as { action?: string })?.action ?? t("thread.tableAction") })
          : t("thread.cardResponded");
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

export function Thread({ detail, live, toolTimes, actions, subagents, children, onTrace, onAnswer, onStartWithMcp, onRetry, onCancelAutoRetry }: ThreadProps) {
  const t = useT();
  const box = useRef<HTMLDivElement>(null);
  const inner = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const lastTop = useRef(0);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const folds: Folds = { open, set: (id, value) => setOpen((all) => ({ ...all, [id]: value })) };
  const groups = useMemo(() => groupMessages(detail.raw, live), [detail.raw, live]);
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
  const pending = new Set(detail.pending_approvals.map((a) => a.call.id));
  const pendingInput = new Set(detail.pending_approvals.filter((a) => a.kind === "input").map((a) => a.call.id));
  const childByCall = new Map(children.filter((c) => c.lineage.parent_tool_call_id).map((c) => [c.lineage.parent_tool_call_id!, c.id]));
  const running = new Set(live.runningTools.map((c) => c.id));
  const cutAt = new Set(detail.checkpoints.map((c) => c.up_to_seq));
  const failure = detail.last_error ?? null;
  const streaming = live.steps.some((step) => !step.parts);
  const busy = !failure && (detail.status === "running" || streaming || running.size > 0);

  // Follow the conversation as it grows (streaming, folding, cards loading).
  useEffect(() => {
    const el = box.current;
    if (!el || !inner.current) return;
    const observer = new ResizeObserver(() => {
      if (stick.current) el.scrollTop = el.scrollHeight;
    });
    observer.observe(inner.current);
    return () => observer.disconnect();
  }, []);

  const renderBlock = (b: Exclude<TurnBlock, { type: "thinking" | "tool_group" }>) => {
    switch (b.type) {
      case "text":
        return <Markdown text={b.text} />;
      case "approval":
        return (
          <motion.div variants={enter} initial="hidden" animate="shown">
            <ApprovalCard pending={b.pending} onApprove={() => actions.approve(b.pending.call.id)} onDeny={(reason) => actions.deny(b.pending.call.id, reason)} />
          </motion.div>
        );
      case "subagent": {
        const a = (b.call.arguments ?? {}) as { task?: string; profile?: string };
        return (
          <SubagentCard
            task={a.task ?? ""}
            profile={a.profile ?? "explorer"}
            live={subagents[b.call.id]}
            result={results.get(b.call.id)}
            subSessionId={childByCall.get(b.call.id)}
            onTrace={onTrace}
          />
        );
      }
      case "ask_user":
        return <AskUserCard call={b.call} pending={pendingInput.has(b.call.id)} result={results.get(b.call.id)} onAnswer={(c) => onAnswer(b.call.id, c)} />;
      case "mcp_configured":
        return <McpConfiguredCard result={results.get(b.call.id)!} onStart={onStartWithMcp} />;
      case "widget": {
        const spec = asWidget(b.call.arguments);
        return spec ? (
          <Widget
            id={b.call.id}
            spec={spec}
            response={responses.get(b.call.id)}
            pendingApproval={pending.has(b.call.id)}
            result={results.get(b.call.id)}
            laterUserTexts={userTextsAfter(b.seq)}
            actions={actions}
            disabled={detail.status === "halted"}
          />
        ) : null;
      }
    }
  };

  const mcpShown = (id: string) => results.get(id)?.is_error === false;
  const turnBlocks = (msgs: SourceMessage[], approvals: TurnBlock[], working: boolean) => {
    const list = segments([...buildBlocks(msgs, mcpShown), ...approvals]);
    return list.map((seg, i) => {
      if (seg.kind === "block") return <Fragment key={seg.id}>{renderBlock(seg.block)}</Fragment>;
      const active =
        (working && i === list.length - 1) ||
        seg.items.some((b) => (b.type === "thinking" ? b.status === "streaming" : b.calls.some((c) => running.has(c.id))));
      return <WorkSegment key={seg.id} id={seg.id} items={seg.items} active={active} tools={{ results, running, times: toolTimes }} folds={folds} />;
    });
  };

  const otherApprovals = detail.pending_approvals.filter((a) => a.call.name !== RENDER_WIDGET && a.kind === "approval");
  // Subagents started by skills (`skill:` ids) have no tool call in this history: show them
  // while they run; their report arrives as a message.
  const reportsIn = new Set(detail.raw.filter((m) => m.role === "user").flatMap((m) => [...textOf(m).matchAll(/session="([^"]+)"/g)].map((x) => x[1])));
  const skillRuns = Object.entries(subagents).filter(([id, s]) => id.startsWith("skill:") && !reportsIn.has(s.subSessionId));

  const approvalBlocks: TurnBlock[] = otherApprovals.map((pa) => ({ type: "approval", id: `approval:${pa.call.id}`, pending: pa }));
  const markState: MarkState = !busy ? "idle" : running.size > 0 ? "running" : "thinking";
  const hasTail = approvalBlocks.length > 0 || skillRuns.length > 0 || busy || !!failure;
  const shown: Group[] = hasTail && groups[groups.length - 1]?.kind !== "assistant" ? [...groups, { kind: "assistant", msgs: [] }] : groups;
  // Steps that belong to the turn in progress but have no place in its history yet.
  const tail = (
    <>
      {skillRuns.map(([id, s]) => (
        <SubagentCard
          key={id}
          task={children.find((c) => c.id === s.subSessionId)?.title ?? t("subagent.skillRunning")}
          profile={s.profile}
          live={s}
          subSessionId={s.subSessionId}
          onTrace={onTrace}
        />
      ))}
      {failure && !streaming && (
        <motion.div variants={enter} initial="hidden" animate="shown">
          <LoopErrorCard error={failure} onRetry={onRetry} onCancelAuto={onCancelAutoRetry} />
        </motion.div>
      )}
    </>
  );

  return (
    <div
      className="thread"
      ref={box}
      onScroll={(e) => {
        const el = e.currentTarget;
        // Only a move upwards is the reader's; a growing page must not stop the following.
        if (el.scrollTop < lastTop.current - 1) stick.current = false;
        if (el.scrollHeight - el.scrollTop - el.clientHeight < 60) stick.current = true;
        lastTop.current = el.scrollTop;
      }}
    >
      <div className="thread-inner" ref={inner}>
        <AnimatePresence initial={false}>
          {shown.map((g, i) => {
            const isLast = i === shown.length - 1;
            return (
              <motion.div key={i} className="group" variants={enter} initial="hidden" animate="shown">
                {g.kind === "user" && cutAt.has(g.msg.seq) && <div className="epoch">{t("thread.compacted")}</div>}
                {g.kind === "user" ? (
                  <UserBubble msg={g.msg} widgetTitles={widgetTitles} />
                ) : (
                  <Turn state={isLast ? markState : "idle"}>
                    {turnBlocks(g.msgs, isLast ? approvalBlocks : [], isLast && busy)}
                    {isLast && tail}
                  </Turn>
                )}
              </motion.div>
            );
          })}
        </AnimatePresence>
      </div>
    </div>
  );
}
