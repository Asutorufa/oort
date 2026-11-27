import { useState } from "react";
import { errorText, sessions } from "../../api";
import type { InputRequest, SessionDetail } from "../../api/types";
import { JsonView } from "../common";

/** Human-in-the-loop card for every call awaiting a decision. */
export function ApprovalCard({ detail, onError }: { detail: SessionDetail; onError: (e: string) => void }) {
  const [reasons, setReasons] = useState<Record<string, string>>({});
  if (detail.pending_approvals.length === 0) return null;
  const send = (input: InputRequest) => sessions.input(detail.id, input).catch((e) => onError(errorText(e)));

  return (
    <div className="approval">
      <div className="approval-title">Approval required</div>
      {detail.pending_approvals.map(({ call, reason }) => (
        <div key={call.id} className="approval-item">
          <div className="row">
            <b className="mono">{call.name}</b>
            <span className="muted">{reason}</span>
            <span className="spacer" />
            <code className="muted">{call.id}</code>
          </div>
          <JsonView value={call.arguments} />
          <div className="row">
            <button className="primary" onClick={() => send({ type: "approval", call_id: call.id, decision: "approve" })}>
              Approve
            </button>
            <input
              placeholder="reason for denying (optional, shown to the model)"
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

/** Message box: Send/Steer (soft insert) vs Interrupt (hard), plus Compact and Stop. */
export function Composer({ detail, onError }: { detail: SessionDetail; onError: (e: string) => void }) {
  const [text, setText] = useState("");
  const send = async (input: InputRequest, clear = true) => {
    try {
      await sessions.input(detail.id, input);
      if (clear) setText("");
    } catch (e) {
      onError(errorText(e));
    }
  };
  const busy = detail.status === "running";
  const halted = detail.status === "halted";
  const empty = !text.trim();

  return (
    <div className="composer">
      <textarea
        value={text}
        disabled={halted}
        placeholder={halted ? "session is halted" : "Message… (Ctrl+Enter: Send / Steer)"}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !empty) send({ type: "steer", text });
        }}
      />
      <div className="row">
        <button
          className="primary"
          disabled={empty || halted}
          onClick={() => send({ type: "steer", text })}
          title="Soft insert: never cancels anything; applied at the next safe checkpoint"
        >
          {busy ? "Steer" : "Send"}
        </button>
        <button
          className="danger"
          disabled={empty || halted || detail.status === "idle"}
          onClick={() => send({ type: "interrupt", text })}
          title="Hard interrupt: cancels the current turn (or pending approvals), then appends this message"
        >
          Interrupt
        </button>
        <span className="spacer" />
        {detail.pending_steers > 0 && <span className="muted">{detail.pending_steers} queued</span>}
        <button disabled={halted} onClick={() => send({ type: "compact" }, false)} title="Run an epoch compaction before the next LLM call">
          Compact
        </button>
        <button
          disabled={!detail.active}
          onClick={() => sessions.cancel(detail.id).catch((e) => onError(errorText(e)))}
          title="Stop the background loop (state is kept and resumes on the next input)"
        >
          Stop loop
        </button>
      </div>
    </div>
  );
}
