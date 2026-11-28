import { useCallback, useEffect, useRef, useState } from "react";
import { errorText, sessions } from "../api";
import type { AgentEvent, Envelope, LiveTurnWire, PlanStep, SessionDetail, ToolCall } from "../api/types";

/** Uncommitted output of the turn in progress (deltas are never part of the snapshot). */
/** Live progress of one subagent (keyed by the parent's tool call id). */
export interface SubagentLive {
  subSessionId: string;
  profile: string;
  turns: number;
  /** Tool the child is running right now. */
  currentTool: string | null;
  toolsRun: number;
  startedAt: number;
  lastAt: number;
  done: boolean;
}

export interface Live {
  turn: number | null;
  text: string;
  thinking: string;
  runningTools: ToolCall[];
}

export const EMPTY_LIVE: Live = { turn: null, text: "", thinking: "", runningTools: [] };

export type ConnState = "connecting" | "open" | "reconnecting" | "closed";

export interface LogEntry {
  seq: number | null;
  label: string;
  at: number;
}

/** Client-measured tool durations (`tool_started` → `tool_finished`), kept per browser tab. */
export type ToolTimes = Record<string, { startedAt: number; ms?: number }>;

const fromWire = (l: LiveTurnWire): Live => ({
  turn: l.turn,
  text: l.text,
  thinking: l.thinking,
  runningTools: l.running_tools,
});

function describe(ev: AgentEvent): string {
  switch (ev.event) {
    case "turn_started":
      return `turn ${ev.turn} started`;
    case "subagent":
      return `subagent ${ev.profile}: ${describe(ev.inner)}`;
    case "plan_updated":
      return `plan: ${ev.steps.filter((s) => s.status === "done").length}/${ev.steps.length} done`;
    case "tool_started":
      return `tool ${ev.call.name} started`;
    case "tool_finished":
      return `tool ${ev.result.name ?? ev.result.tool_call_id} ${ev.result.is_error ? "failed" : "ok"}`;
    case "approval_required":
      return `approval required: ${ev.calls.map((c) => c.name).join(", ")}`;
    case "interrupted":
      return `interrupted during ${ev.during}`;
    case "compacted":
      return `compacted epoch ${ev.epoch} (up to #${ev.up_to_index})`;
    case "usage":
      return `usage in=${ev.usage.input_tokens} cached=${ev.usage.cache_read_tokens ?? 0} out=${ev.usage.output_tokens}`;
    case "llm_call":
      return `llm call ttft=${ev.stats.ttft_ms ?? "—"}ms total=${ev.stats.latency_ms}ms`;
    case "phase_changed":
      return `phase → ${ev.phase}`;
    case "steer_applied":
      return `steer applied (${ev.count})`;
    case "halted":
      return `halted: ${ev.reason}`;
    default:
      return ev.event;
  }
}

const timesKey = (id: string) => `oort.toolTimes.${id}`;

/**
 * Snapshot + SSE for one session.
 *
 * The browser's `EventSource` reconnects by itself after a dropped connection and sends
 * `Last-Event-ID`, so the server can replay what was missed (`hello.resumed`). When it cannot
 * (server restart, replay buffer exceeded, page reload) the hello carries the in-progress turn
 * and we refetch the snapshot. If the connection is closed for good (e.g. HTTP error), we
 * reconnect with backoff.
 */
export function useSessionStream(id: string | null, onDeleted?: () => void) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [live, setLive] = useState<Live>(EMPTY_LIVE);
  const [log, setLog] = useState<LogEntry[]>([]);
  const [conn, setConn] = useState<ConnState>("connecting");
  const [error, setError] = useState<string | null>(null);
  const [toolTimes, setToolTimes] = useState<ToolTimes>({});
  const [subagents, setSubagents] = useState<Record<string, SubagentLive>>({});
  const [plan, setPlan] = useState<PlanStep[] | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const onDeletedRef = useRef(onDeleted);
  onDeletedRef.current = onDeleted;

  const refresh = useCallback(async () => {
    if (!id) return;
    try {
      setDetail(await sessions.get(id));
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  }, [id]);

  // Coalesce bursts of events into one snapshot fetch.
  const scheduleRefresh = useCallback(() => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(refresh, 80);
  }, [refresh]);

  useEffect(() => {
    setDetail(null);
    setLive(EMPTY_LIVE);
    setSubagents({});
    setPlan(null);
    setLog([]);
    setError(null);
    if (!id) return;
    try {
      setToolTimes(JSON.parse(sessionStorage.getItem(timesKey(id)) ?? "{}"));
    } catch {
      setToolTimes({});
    }

    let es: EventSource | null = null;
    let retry: number | undefined;
    let backoff = 1000;
    let stopped = false;
    const push = (seq: number | null, label: string) =>
      setLog((l) => [...l.slice(-299), { seq, label, at: Date.now() }]);
    const recordTime = (callId: string, update: (t: ToolTimes[string] | undefined) => ToolTimes[string]) =>
      setToolTimes((all) => {
        const next = { ...all, [callId]: update(all[callId]) };
        sessionStorage.setItem(timesKey(id), JSON.stringify(next));
        return next;
      });

    const apply = (ev: AgentEvent) => {
      switch (ev.event) {
        case "text_delta":
          setLive((l) => ({ ...l, text: l.text + ev.delta }));
          return;
        case "thinking_delta":
          setLive((l) => ({ ...l, thinking: l.thinking + ev.delta }));
          return;
        case "turn_started":
          setLive({ ...EMPTY_LIVE, turn: ev.turn });
          return;
        case "tool_started":
          recordTime(ev.call.id, () => ({ startedAt: Date.now() }));
          setLive((l) => ({ ...l, runningTools: [...l.runningTools, ev.call] }));
          return;
        case "tool_finished":
          recordTime(ev.result.tool_call_id, (t) => ({
            startedAt: t?.startedAt ?? Date.now(),
            ms: t ? Date.now() - t.startedAt : undefined,
          }));
          setLive((l) => ({ ...l, runningTools: l.runningTools.filter((c) => c.id !== ev.result.tool_call_id) }));
          scheduleRefresh();
          return;
        case "assistant_message":
        case "interrupted":
          setLive((l) => ({ ...l, text: "", thinking: "" }));
          scheduleRefresh();
          return;
        case "completed":
        case "halted":
          setLive(EMPTY_LIVE);
          scheduleRefresh();
          return;
        case "usage":
          return;
        case "plan_updated":
          setPlan(ev.steps);
          return;
        case "subagent": {
          const inner = ev.inner;
          setSubagents((all) => {
            const now = Date.now();
            const cur: SubagentLive = all[ev.tool_call_id] ?? {
              subSessionId: ev.sub_session_id,
              profile: ev.profile,
              turns: 0,
              currentTool: null,
              toolsRun: 0,
              startedAt: now,
              lastAt: now,
              done: false,
            };
            const next = { ...cur, lastAt: now };
            if (inner.event === "turn_started") next.turns = inner.turn;
            if (inner.event === "tool_started") next.currentTool = inner.call.name;
            if (inner.event === "tool_finished") {
              next.currentTool = null;
              next.toolsRun += 1;
            }
            if (inner.event === "completed" || inner.event === "halted") next.done = true;
            return { ...all, [ev.tool_call_id]: next };
          });
          return;
        }
        default:
          scheduleRefresh();
      }
    };

    const connect = () => {
      if (stopped) return;
      setConn((c) => (c === "open" ? "reconnecting" : c));
      es = new EventSource(sessions.eventsUrl(id));
      es.onerror = () => {
        if (stopped || !es) return;
        if (es.readyState === EventSource.CLOSED) {
          // The browser gave up (HTTP error): retry ourselves, slowly.
          setConn("reconnecting");
          es.close();
          retry = window.setTimeout(connect, backoff);
          backoff = Math.min(backoff * 2, 15000);
        } else {
          setConn("reconnecting"); // the browser retries with Last-Event-ID
        }
      };
      es.onmessage = (msg) => {
        const env: Envelope = JSON.parse(msg.data);
        switch (env.kind) {
          case "hello":
            backoff = 1000;
            setConn("open");
            push(env.seq, env.resumed ? "reconnected (missed events replayed)" : "connected");
            if (!env.resumed) {
              setLive(fromWire(env.live));
              refresh();
            }
            return;
          case "agent":
            apply(env.event);
            const inner = env.event.event === "subagent" ? env.event.inner : env.event;
            if (inner.event !== "text_delta" && inner.event !== "thinking_delta") {
              push(env.seq, describe(env.event));
            }
            return;
          case "loop_exited":
            push(env.seq, `loop exited${env.error ? `: ${env.error}` : ""}`);
            setLive(EMPTY_LIVE);
            scheduleRefresh();
            return;
          case "session_updated":
            push(env.seq, `updated: ${env.approval_policy}`);
            scheduleRefresh();
            return;
          case "deleted":
            push(env.seq, "session deleted");
            stopped = true;
            es?.close();
            setConn("closed");
            onDeletedRef.current?.();
            return;
          case "lagged":
            push(null, `lagged (${env.skipped} events skipped) — resynced`);
            scheduleRefresh();
            return;
        }
      };
    };
    setConn("connecting");
    connect();

    return () => {
      stopped = true;
      es?.close();
      window.clearTimeout(retry);
      window.clearTimeout(timer.current);
    };
  }, [id, refresh, scheduleRefresh]);

  return { detail, live, log, conn, error, refresh, toolTimes, subagents, plan };
}
