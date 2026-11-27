import { useEffect, useMemo, useRef } from "react";
import type { ContentPart, RawMessage, SessionDetail, ToolResult, TurnUsage } from "../../api/types";
import type { Live, ToolTimes } from "../../hooks/useSessionStream";
import { fmtMs, pct } from "../common";
import { ThinkingCard, ToolCard } from "./Blocks";
import { Markdown } from "./LazyMarkdown";

interface Props {
  detail: SessionDetail;
  live: Live;
  toolTimes: ToolTimes;
  onInspect: (turn: TurnUsage) => void;
}

/** Results by call id, so each call renders with its result in one card. */
function resultsById(raw: RawMessage[]): Map<string, ToolResult> {
  const m = new Map<string, ToolResult>();
  for (const msg of raw)
    for (const p of msg.content) if (p.kind === "tool_result") m.set(p.tool_call_id, p);
  return m;
}

function UserText({ text }: { text: string }) {
  return <div className="plain">{text}</div>;
}

/** Raw history with compaction markers, followed by the live (uncommitted) turn. */
export function Transcript({ detail, live, toolTimes, onInspect }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const results = useMemo(() => resultsById(detail.raw), [detail.raw]);
  const callIds = useMemo(() => {
    const s = new Set<string>();
    for (const m of detail.raw) for (const p of m.content) if (p.kind === "tool_call") s.add(p.id);
    return s;
  }, [detail.raw]);
  const turnBySeq = useMemo(() => new Map(detail.usage.per_turn.map((t) => [t.seq, t])), [detail.usage.per_turn]);
  const cutAt = new Map(detail.checkpoints.map((c) => [c.up_to_seq, c.epoch]));
  const running = new Set(live.runningTools.map((c) => c.id));

  // Follow the output while the user is at the bottom; leave them alone when they scroll up.
  useEffect(() => {
    const el = box.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [detail.raw.length, live.text.length, live.thinking.length]);

  const part = (p: ContentPart, i: number, m: RawMessage) => {
    switch (p.kind) {
      case "text":
        return m.role === "assistant" ? <Markdown key={i} text={p.text} /> : <UserText key={i} text={p.text} />;
      case "thinking":
        return <ThinkingCard key={i} text={p.thinking} signed={!!p.signature} />;
      case "tool_call":
        return (
          <ToolCard key={i} call={p} result={results.get(p.id)} running={running.has(p.id)} durationMs={toolTimes[p.id]?.ms} />
        );
      case "tool_result":
        // Shown with its call; only an orphan result renders here.
        return callIds.has(p.tool_call_id) ? null : (
          <ToolCard key={i} call={{ id: p.tool_call_id, name: p.name ?? "?", arguments: null }} result={p} />
        );
      default:
        return (
          <div key={i} className="muted">
            [{p.kind}]
          </div>
        );
    }
  };

  return (
    <div
      className="transcript"
      ref={box}
      onScroll={(e) => {
        const el = e.currentTarget;
        stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
      }}
    >
      {detail.system_prompt && (
        <details className="card system">
          <summary>
            <span className="card-kind">System prompt</span>
            <span className="muted">{detail.system_prompt.length} chars · frozen</span>
          </summary>
          <pre>{detail.system_prompt}</pre>
        </details>
      )}
      {detail.raw.map((m) => {
        const allPaired = m.role === "tool" && m.content.every((p) => p.kind === "tool_result" && callIds.has(p.tool_call_id));
        const turn = turnBySeq.get(m.seq);
        return (
          <div key={m.seq}>
            {cutAt.has(m.seq) && <div className="epoch-marker">── epoch {cutAt.get(m.seq)} starts here (earlier messages summarized) ──</div>}
            {!allPaired && (
              <div className={`msg ${m.role} ${m.seq < detail.active_view.from_seq ? "compacted" : ""}`}>
                <div className="msg-head">
                  <span className="role">{m.role}</span>
                  <span className="muted">#{m.seq}</span>
                  {turn && (
                    <>
                      <span className="muted" title="time to first token · total">
                        {fmtMs(turn.ttft_ms)} · {fmtMs(turn.latency_ms)}
                      </span>
                      <span className="muted" title="prompt / cached / output tokens">
                        {turn.prompt_tokens}/{turn.cache_read_tokens}/{turn.output_tokens} · {pct(turn.cache_hit_rate)}
                      </span>
                      <span className="spacer" />
                      <button className="small ghost" title="Inspect this turn" onClick={() => onInspect(turn)}>
                        ⓘ debug
                      </button>
                    </>
                  )}
                </div>
                {m.content.map((p, i) => part(p, i, m))}
              </div>
            )}
          </div>
        );
      })}
      {(live.text || live.thinking) && (
        <div className="msg assistant live">
          <div className="msg-head">
            <span className="role">assistant</span>
            <span className="muted">streaming{live.turn ? ` · turn ${live.turn}` : ""}…</span>
          </div>
          {live.thinking && <ThinkingCard text={live.thinking} live />}
          {live.text && <Markdown text={live.text} />}
        </div>
      )}
    </div>
  );
}
