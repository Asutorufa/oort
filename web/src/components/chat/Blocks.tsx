import type { ToolCall, ToolResult } from "../../api/types";
import { Badge, fmtMs, JsonView } from "../common";

export function ThinkingCard({ text, signed, live }: { text: string; signed?: boolean; live?: boolean }) {
  return (
    <details className="card thinking" open={live}>
      <summary>
        <span className="card-kind">Thinking</span>
        <span className="muted">
          {text.length} chars{signed ? " · signed" : ""}
        </span>
        {live && <Badge tone="info">streaming</Badge>}
      </summary>
      <pre>{text || "(redacted)"}</pre>
    </details>
  );
}

export type ToolState = "running" | "pending" | "ok" | "error";

/** Status shown on a call's collapsed card. */
export function toolState(result: ToolResult | undefined, running: boolean): ToolState {
  if (result) return result.is_error ? "error" : "ok";
  return running ? "running" : "pending";
}

const TONE = { running: "info", pending: "warn", ok: "ok", error: "error" } as const;
const LABEL = { running: "running", pending: "pending", ok: "ok", error: "error" } as const;

/**
 * One tool call with its result. The collapsed summary shows the tool name, duration and
 * status, so a long agentic run can be scanned without opening anything.
 */
export function ToolCard({
  call,
  result,
  running,
  durationMs,
}: {
  call: ToolCall;
  result?: ToolResult;
  running?: boolean;
  durationMs?: number;
}) {
  const state = toolState(result, !!running);
  return (
    <details className={`card tool ${state}`} open={state === "error"}>
      <summary>
        <span className="card-kind">Tool</span>
        <b className="mono">{call.name}</b>
        <Badge tone={TONE[state]}>{LABEL[state]}</Badge>
        <span className="muted">{durationMs != null ? fmtMs(durationMs) : state === "running" ? "…" : ""}</span>
        <span className="spacer" />
        <code className="muted">{call.id}</code>
      </summary>
      <div className="card-section">
        <div className="card-label">arguments</div>
        <JsonView value={call.arguments} />
      </div>
      {result && (
        <div className="card-section">
          <div className="card-label">result{result.is_error ? " (is_error)" : ""}</div>
          <pre className={result.is_error ? "error-text" : ""}>{result.content}</pre>
        </div>
      )}
    </details>
  );
}
