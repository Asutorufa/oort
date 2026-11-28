import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ToolCall, ToolResult } from "../../api/types";
import { ChevronDown } from "../../brand/Icons";
import type { ToolTimes } from "../../hooks/useSessionStream";
import { useI18n } from "../../i18n";
import { JsonView } from "../common";
import { Collapse } from "../ui";
import type { ThinkingBlock, WorkBlock } from "./blocks";

/** The user's explicit open / closed choices, by block id. Absent means "follow the default". */
export interface Folds {
  open: Record<string, boolean>;
  set: (id: string, open: boolean) => void;
}

export interface ToolState {
  results: Map<string, ToolResult>;
  running: Set<string>;
  times: ToolTimes;
}

function useSeconds() {
  const { fmt } = useI18n();
  return (ms: number) => fmt.number(ms / 1000, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
}

/** Re-renders while something is being timed. */
function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const id = window.setInterval(() => setNow(Date.now()), 100);
    return () => window.clearInterval(id);
  }, [active]);
  return now;
}

function ToolLine({ call, result, running, ms }: { call: ToolCall; result?: ToolResult; running: boolean; ms?: number }) {
  const { t, fmt } = useI18n();
  const state = result ? (result.is_error ? "error" : "ok") : running ? "running" : "pending";
  return (
    <details className={`tool-line ${state}`}>
      <summary>
        <span className={`state-dot ${state}`} />
        <span className="tool-name">{call.name.replace("__", " · ")}</span>
        <span className="muted">{ms != null ? fmt.ms(ms) : state === "running" ? t("thread.toolRunning") : state === "pending" ? t("thread.toolPending") : ""}</span>
      </summary>
      <div className="tool-detail">
        <div className="label">{t("thread.input")}</div>
        <JsonView value={call.arguments} />
        {result && (
          <>
            <div className="label">{result.is_error ? t("common.error") : t("thread.result")}</div>
            <pre className={result.is_error ? "error-text" : ""}>{result.content}</pre>
          </>
        )}
      </div>
    </details>
  );
}

/** The reasoning text; follows new output until the reader scrolls up. */
function Viewport({ text, tall }: { text: string; tall: boolean }) {
  const box = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  const lastTop = useRef(0);
  useLayoutEffect(() => {
    const el = box.current;
    if (el && follow.current) el.scrollTop = el.scrollHeight;
  }, [text]);
  return (
    <div
      ref={box}
      className={`think-viewport ${tall ? "tall" : ""}`}
      onScroll={(e) => {
        const el = e.currentTarget;
        // Only a move upwards is the reader's; appended text must not stop the following.
        if (el.scrollTop < lastTop.current - 1) follow.current = false;
        else if (el.scrollHeight - el.scrollTop - el.clientHeight < 8) follow.current = true;
        lastTop.current = el.scrollTop;
      }}
    >
      {text}
    </div>
  );
}

/**
 * One reasoning block: open while it streams, folding to a one-line pill when the model moves
 * on. Clicking it while it streams pins it open.
 */
function ThinkingItem({ block, folds }: { block: ThinkingBlock; folds: Folds }) {
  const { t } = useI18n();
  const seconds = useSeconds();
  const streaming = block.status === "streaming";
  const now = useNow(streaming);
  const choice = folds.open[block.id];
  const open = choice ?? streaming;
  const pinned = choice === true;
  const label = streaming
    ? t("thread.thinkingElapsed", { sec: seconds(block.startedAt == null ? 0 : Math.max(0, now - block.startedAt)) })
    : block.durationMs != null
      ? t("thread.thoughtFor", { sec: seconds(block.durationMs) })
      : t("thread.thought");
  return (
    <div className={`think ${streaming ? "streaming" : ""} ${open ? "open" : ""}`}>
      <button
        className="activity-row"
        aria-expanded={open}
        title={streaming && !pinned ? t("thread.pinThinking") : undefined}
        onClick={() => folds.set(block.id, streaming && choice === undefined ? true : !open)}
      >
        {streaming && <span className="pulse" />}
        <span>{label}</span>
        {streaming && pinned && <span className="pill">{t("thread.pinned")}</span>}
        <span className="chev">
          <ChevronDown size={13} />
        </span>
      </button>
      <Collapse open={open}>
        <Viewport text={block.text} tall={!streaming && !pinned} />
      </Collapse>
    </div>
  );
}

/**
 * A run of reasoning and tool calls with no answer text in between. While it is in progress its
 * steps are listed in place; afterwards it folds to one summary line. Expanding it shows the
 * same steps in the same order.
 */
export function WorkSegment({ id, items, active, tools, folds }: { id: string; items: WorkBlock[]; active: boolean; tools: ToolState; folds: Folds }) {
  const { t } = useI18n();
  const seconds = useSeconds();
  const calls = items.flatMap((b) => (b.type === "tool_group" ? b.calls : []));
  const thoughts = items.filter((b): b is ThinkingBlock => b.type === "thinking");
  // A lone thought is its own summary line.
  const summarized = calls.length > 0 || items.length > 1;
  const pinnedInside = thoughts.some((b) => folds.open[b.id] === true);
  const open = folds.open[id] ?? pinnedInside;
  const showBody = !summarized || active || open;

  const timed = thoughts.filter((b) => b.durationMs != null);
  const parts = [
    thoughts.length === 0
      ? null
      : timed.length > 0
        ? t("thread.thoughtFor", { sec: seconds(timed.reduce((sum, b) => sum + (b.durationMs ?? 0), 0)) })
        : t("thread.thought"),
    calls.length > 0 ? t("thread.tools", { count: calls.length }) : null,
  ].filter(Boolean);
  const failed = calls.filter((c) => tools.results.get(c.id)?.is_error).length;

  return (
    <div className="work">
      {summarized && !active && (
        <div className={`activity ${open ? "open" : ""}`}>
          <button className="activity-row" aria-expanded={open} onClick={() => folds.set(id, !open)}>
            <span>{parts.join(" · ")}</span>
            {failed > 0 && <span className="pill danger">{t("thread.errors", { count: failed })}</span>}
            <span className="chev">
              <ChevronDown size={13} />
            </span>
          </button>
        </div>
      )}
      <Collapse open={showBody}>
        <div className={summarized ? "work-body" : undefined}>
          {items.map((b) =>
            b.type === "thinking" ? (
              <ThinkingItem key={b.id} block={b} folds={folds} />
            ) : (
              <div key={b.id} className="tool-group">
                {b.calls.map((c) => (
                  <ToolLine key={c.id} call={c} result={tools.results.get(c.id)} running={tools.running.has(c.id)} ms={tools.times[c.id]?.ms} />
                ))}
              </div>
            ),
          )}
        </div>
      </Collapse>
    </div>
  );
}
