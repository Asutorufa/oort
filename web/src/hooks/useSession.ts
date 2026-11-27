import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { AgentEvent, Envelope, SessionDetail } from "../types";

/** Uncommitted output of the turn in progress (deltas are never part of the snapshot). */
export interface Live {
  thinking: string;
  text: string;
  runningTools: string[];
}

export interface LogEntry {
  seq: number;
  label: string;
  at: number;
}

const EMPTY_LIVE: Live = { thinking: "", text: "", runningTools: [] };

/**
 * Snapshot + event stream for one session.
 *
 * The snapshot (GET /sessions/:id) is the source of truth; SSE events drive the live
 * streaming block and tell us when to refetch it. This keeps client state trivial.
 */
export function useSession(id: string | null) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [live, setLive] = useState<Live>(EMPTY_LIVE);
  const [log, setLog] = useState<LogEntry[]>([]);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<number | undefined>(undefined);

  const refresh = useCallback(async () => {
    if (!id) return;
    try {
      setDetail(await api.getSession(id));
      setError(null);
    } catch (e) {
      setError(String(e));
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
    setLog([]);
    if (!id) return;
    refresh();
    const es = new EventSource(api.eventsUrl(id));
    es.onopen = () => setConnected(true);
    es.onerror = () => setConnected(false);
    es.onmessage = (msg) => {
      const env: Envelope = JSON.parse(msg.data);
      if (env.kind === "agent") {
        applyEvent(env.event);
        if (env.event.event !== "text_delta" && env.event.event !== "thinking_delta") {
          setLog((l) => [...l.slice(-199), { seq: env.seq, label: describe(env.event), at: Date.now() }]);
        }
      } else if (env.kind === "loop_exited") {
        setLog((l) => [...l, { seq: env.seq, label: `loop exited${env.error ? `: ${env.error}` : ""}`, at: Date.now() }]);
        setLive(EMPTY_LIVE);
        scheduleRefresh();
      } else {
        scheduleRefresh();
      }
    };
    return () => {
      es.close();
      window.clearTimeout(timer.current);
    };
  }, [id]);

  function applyEvent(ev: AgentEvent) {
    switch (ev.event) {
      case "text_delta":
        setLive((l) => ({ ...l, text: l.text + ev.delta }));
        return;
      case "thinking_delta":
        setLive((l) => ({ ...l, thinking: l.thinking + ev.delta }));
        return;
      case "turn_started":
        setLive(EMPTY_LIVE);
        return;
      case "tool_started":
        setLive((l) => ({ ...l, runningTools: [...l.runningTools, ev.call.name] }));
        return;
      case "tool_finished":
        setLive((l) => ({ ...l, runningTools: l.runningTools.filter((n) => n !== ev.result.name) }));
        scheduleRefresh();
        return;
      case "assistant_message":
      case "interrupted":
        setLive(EMPTY_LIVE);
        scheduleRefresh();
        return;
      case "usage":
        return;
      default:
        scheduleRefresh();
    }
  }

  return { detail, live, log, connected, error, refresh };
}

function describe(ev: AgentEvent): string {
  switch (ev.event) {
    case "turn_started":
      return `turn ${ev.turn} started`;
    case "assistant_message":
      return "assistant message committed";
    case "tool_started":
      return `tool ▶ ${ev.call.name}`;
    case "tool_finished":
      return `tool ■ ${ev.result.name ?? ev.result.tool_call_id}${ev.result.is_error ? " (error)" : ""}`;
    case "approval_required":
      return `approval required: ${ev.calls.map((c) => c.name).join(", ")}`;
    case "steer_applied":
      return `steer applied ×${ev.count}`;
    case "interrupted":
      return `interrupted during ${ev.during}`;
    case "compacted":
      return `compacted → epoch ${ev.epoch} (up to #${ev.up_to_index})`;
    case "usage":
      return `usage in=${ev.usage.input_tokens} cached=${ev.usage.cache_read_tokens ?? 0} out=${ev.usage.output_tokens}`;
    case "phase_changed":
      return `phase → ${ev.phase}`;
    case "completed":
      return "completed";
    case "halted":
      return `halted: ${ev.reason}`;
    default:
      return ev.event;
  }
}
