import { useState } from "react";
import { api } from "../api";
import type { InputRequest, SessionDetail } from "../types";

/** Human-in-the-loop card for every call awaiting a decision. */
export function ApprovalCard({ detail, onError }: { detail: SessionDetail; onError: (e: string) => void }) {
  const [reasons, setReasons] = useState<Record<string, string>>({});
  if (detail.pending_approvals.length === 0) return null;
  const send = (input: InputRequest) => api.sendInput(detail.id, input).catch((e) => onError(String(e)));

  return (
    <div className="approval">
      <h3>Approval required</h3>
      {detail.pending_approvals.map(({ call, reason }) => (
        <div key={call.id} className="approval-item">
          <div>
            <b>{call.name}</b> <code>{call.id}</code> <span className="muted">{reason}</span>
          </div>
          <pre>{JSON.stringify(call.arguments, null, 2)}</pre>
          <div className="row">
            <button className="primary" onClick={() => send({ type: "approval", call_id: call.id, decision: "approve" })}>
              Approve
            </button>
            <input
              placeholder="deny reason (optional)"
              value={reasons[call.id] ?? ""}
              onChange={(e) => setReasons({ ...reasons, [call.id]: e.target.value })}
            />
            <button
              className="danger"
              onClick={() =>
                send({ type: "approval", call_id: call.id, decision: "deny", reason: reasons[call.id] || "denied by user" })
              }
            >
              Deny
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}

/** Message box: Send/Steer (soft insert) vs Interrupt (hard), plus Compact and Cancel. */
export function Composer({ detail, onError }: { detail: SessionDetail; onError: (e: string) => void }) {
  const [text, setText] = useState("");
  const send = async (input: InputRequest, clear = true) => {
    try {
      await api.sendInput(detail.id, input);
      if (clear) setText("");
    } catch (e) {
      onError(String(e));
    }
  };
  const busy = detail.status === "running";
  const halted = detail.status === "halted";

  return (
    <div className="composer">
      <textarea
        value={text}
        disabled={halted}
        placeholder={halted ? "session is halted" : "Message… (Ctrl+Enter = Send / Steer)"}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && text.trim()) send({ type: "steer", text });
        }}
      />
      <div className="row">
        <button className="primary" disabled={!text.trim() || halted} onClick={() => send({ type: "steer", text })}
          title="Soft insert: never cancels anything; applied at the next safe checkpoint">
          {busy ? "Steer" : "Send"}
        </button>
        <button className="danger" disabled={!text.trim() || halted} onClick={() => send({ type: "interrupt", text })}
          title="Hard interrupt: cancels the current turn, then appends this message">
          Interrupt
        </button>
        <span className="spacer" />
        <button disabled={halted} onClick={() => send({ type: "compact" }, false)} title="Run an epoch compaction before the next LLM call">
          Compact
        </button>
        <button onClick={() => api.cancel(detail.id).catch((e) => onError(String(e)))} title="Stop the background loop (state is kept)">
          Stop loop
        </button>
      </div>
    </div>
  );
}
