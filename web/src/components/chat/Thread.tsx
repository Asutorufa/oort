import { useEffect, useMemo, useRef, useState } from "react";
import type { ContentPart, RawMessage, SessionDetail, ToolCall, ToolResult } from "../../api/types";
import { OortMark } from "../../brand/OortMark";
import type { Live, ToolTimes } from "../../hooks/useSessionStream";
import { DiffView } from "../../widgets/ApprovalDiff";
import { asWidget, parseResponse, responsesById, type WidgetActions } from "../../widgets/protocol";
import { Widget } from "../../widgets/WidgetRegistry";
import { fmtMs, JsonView } from "../common";
import { Markdown } from "./LazyMarkdown";

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

const SKILL_RE = /^<skill name="([^"]+)">[\s\S]*?<\/skill>\s*/;

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
          <span className="resp-icon">↩</span> {label}
          {what && <span className="muted"> · {what}</span>}
        </div>
      </div>
    );
  }
  const skill = text.match(SKILL_RE);
  return (
    <div className="row-user">
      <div className="bubble user">
        {skill && <span className="skill-chip">✦ スキル {skill[1]}</span>}
        <div className="plain">{skill ? text.slice(skill[0].length) : text}</div>
      </div>
    </div>
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
        <span className="chev">⌄</span>
      </button>
      {open && (
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
      )}
    </div>
  );
}

// ----- assistant turn ------------------------------------------------------------------------

interface ThreadProps {
  detail: SessionDetail;
  live: Live;
  toolTimes: ToolTimes;
  actions: WidgetActions;
}

export function Thread({ detail, live, toolTimes, actions }: ThreadProps) {
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
  const running = new Set(live.runningTools.map((c) => c.id));
  const cutAt = new Set(detail.checkpoints.map((c) => c.up_to_seq));
  const busy = detail.status === "running";

  useEffect(() => {
    const el = box.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [detail.raw.length, live.text.length, live.thinking.length, detail.pending_approvals.length]);

  const assistant = (msgs: RawMessage[], key: number) => {
    const parts: ContentPart[] = msgs.filter((m) => m.role === "assistant").flatMap((m) => m.content);
    const thinking = parts.flatMap((p) => (p.kind === "thinking" ? [p.thinking] : []));
    const calls = parts.flatMap((p) => (p.kind === "tool_call" && p.name !== RENDER_WIDGET ? [p as ToolCall] : []));
    const latencyMs = msgs.reduce((s, m) => s + (latency.get(m.seq) ?? 0), 0);
    return (
      <div className="row-assistant" key={key}>
        <div className="avatar">
          <OortMark size={22} />
        </div>
        <div className="assistant-body">
          <ActivityRow thinking={thinking} calls={calls} results={results} running={running} toolTimes={toolTimes} latencyMs={latencyMs} />
          {msgs.map((m) =>
            m.content.map((p, i) => {
              if (p.kind === "text" && m.role === "assistant") return p.text.trim() ? <Markdown key={`${m.seq}-${i}`} text={p.text} /> : null;
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
        </div>
      </div>
    );
  };

  const otherApprovals = detail.pending_approvals.filter((a) => a.call.name !== RENDER_WIDGET);

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
        {groups.map((g, i) => (
          <div key={i}>
            {g.kind === "user" && cutAt.has(g.msg.seq) && <div className="epoch">ここまでの会話は要約されています</div>}
            {g.kind === "user" ? <UserBubble msg={g.msg} widgetTitles={widgetTitles} /> : assistant(g.msgs, i)}
          </div>
        ))}

        {otherApprovals.map(({ call, reason }) => {
          const args = call.arguments as Record<string, unknown> | null;
          const diff = typeof args?.diff === "string" ? args.diff : typeof args?.patch === "string" ? args.patch : null;
          return (
            <div className="row-assistant" key={call.id}>
              <div className="avatar">
                <OortMark size={22} />
              </div>
              <section className="widget widget-approval">
                <header className="widget-head">
                  <h4>
                    <span className="tool-name">{call.name.replace("__", " · ")}</span> を実行してよいですか？
                  </h4>
                  <span className="spacer" />
                  <span className="pill warn">承認待ち</span>
                </header>
                <div className="widget-body">
                  <p className="muted">{reason}</p>
                  {diff ? <DiffView diff={diff} /> : <JsonView value={call.arguments} />}
                  <div className="wactions">
                    <button className="btn primary" onClick={() => actions.approve(call.id)}>
                      承認して実行
                    </button>
                    <button className="btn" onClick={() => actions.deny(call.id, "ユーザーがキャンセルしました")}>
                      キャンセル
                    </button>
                  </div>
                </div>
              </section>
            </div>
          );
        })}

        {(live.text || live.thinking || (busy && !live.runningTools.length)) && (
          <div className="row-assistant live">
            <div className="avatar">
              <OortMark size={22} className="spin" />
            </div>
            <div className="assistant-body">
              {live.thinking && !live.text && <div className="activity-row ghost">思考中…</div>}
              {live.text ? <Markdown text={live.text} /> : !live.thinking && <div className="typing"><span /><span /><span /></div>}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
