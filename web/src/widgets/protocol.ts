import type { RawMessage, ToolResult, WidgetSpec } from "../api/types";

/** Marker of a user message that answers a widget (see the server's render_widget docs). */
export const RESPONSE_PREFIX = "[widget_response id=";

export interface WidgetResponse {
  type: string;
  payload: unknown;
  seq: number;
}

/** `[widget_response id=<id> type=<type>]\n<json>` — an ordinary user message, appended at the tail. */
export function formatResponse(id: string, type: string, payload: unknown): string {
  return `${RESPONSE_PREFIX}${id} type=${type}]\n${JSON.stringify(payload)}`;
}

export function parseResponse(text: string): { id: string; type: string; payload: unknown } | null {
  if (!text.startsWith(RESPONSE_PREFIX)) return null;
  const nl = text.indexOf("\n");
  const head = nl < 0 ? text : text.slice(0, nl);
  const m = head.match(/^\[widget_response id=(\S+?)(?: type=(\S+?))?\]/);
  if (!m) return null;
  let payload: unknown = null;
  try {
    payload = nl < 0 ? null : JSON.parse(text.slice(nl + 1));
  } catch {
    payload = text.slice(nl + 1);
  }
  return { id: m[1], type: m[2] ?? "", payload };
}

/** Widget answers found in the history (the persisted "submitted" state). */
export function responsesById(raw: RawMessage[]): Map<string, WidgetResponse> {
  const out = new Map<string, WidgetResponse>();
  for (const m of raw) {
    if (m.role !== "user") continue;
    for (const p of m.content) {
      if (p.kind !== "text") continue;
      const r = parseResponse(p.text);
      if (r) out.set(r.id, { type: r.type, payload: r.payload, seq: m.seq });
    }
  }
  return out;
}

export function asWidget(args: unknown): WidgetSpec | null {
  const a = args as Partial<WidgetSpec> | null;
  return a && typeof a === "object" && typeof a.widget_type === "string" ? { widget_type: a.widget_type, title: a.title, data: (a.data ?? {}) as Record<string, unknown> } : null;
}

export interface WidgetActions {
  /** Sends a widget answer (Steer). */
  respond: (id: string, type: string, payload: unknown) => Promise<void>;
  /** Sends a plain prompt (Steer). */
  prompt: (text: string) => Promise<void>;
  compact: () => Promise<void>;
  approve: (callId: string) => Promise<void>;
  deny: (callId: string, reason: string) => Promise<void>;
}

export interface WidgetProps {
  id: string;
  spec: WidgetSpec;
  /** Answer already in the history. */
  response?: WidgetResponse;
  /** For approval_diff: waiting for a decision, or the recorded result. */
  pendingApproval?: boolean;
  result?: ToolResult;
  /** Later user messages (action_chips marks the chip that was sent). */
  laterUserTexts: string[];
  actions: WidgetActions;
  disabled?: boolean;
}
