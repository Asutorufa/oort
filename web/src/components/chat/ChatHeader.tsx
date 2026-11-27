import { useEffect, useState } from "react";
import { errorText, sessions } from "../../api";
import { APPROVAL_POLICIES, type ApprovalPolicy, type SessionDetail } from "../../api/types";
import type { ConnState } from "../../hooks/useSessionStream";
import { Dot, StatusBadge, type Tone } from "../common";

const CONN: Record<ConnState, [Tone, string]> = {
  connecting: ["muted", "connecting"],
  open: ["ok", "live"],
  reconnecting: ["warn", "reconnecting"],
  closed: ["error", "closed"],
};

export function ChatHeader({
  detail,
  conn,
  onChanged,
  onDelete,
  onError,
}: {
  detail: SessionDetail;
  conn: ConnState;
  onChanged: () => void;
  onDelete: () => void;
  onError: (e: string) => void;
}) {
  const [title, setTitle] = useState(detail.title);
  useEffect(() => setTitle(detail.title), [detail.title]);
  const [tone, label] = CONN[conn];

  const patch = async (req: { title?: string; approval_policy?: ApprovalPolicy }) => {
    try {
      await sessions.patch(detail.id, req);
      onChanged();
    } catch (e) {
      onError(errorText(e));
    }
  };

  return (
    <header className="chat-header">
      <input
        className="title-input"
        value={title}
        placeholder="(untitled session)"
        onChange={(e) => setTitle(e.target.value)}
        onBlur={() => title !== detail.title && patch({ title })}
        onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
      />
      <StatusBadge status={detail.status} />
      <span className="muted mono" title="provider / model (frozen at creation)">
        {detail.provider} / {detail.model}
      </span>
      <span className="spacer" />
      <label className="inline" title={APPROVAL_POLICIES.find((p) => p.value === detail.approval_policy)?.hint}>
        <span className="muted">approval</span>
        <select value={detail.approval_policy} onChange={(e) => patch({ approval_policy: e.target.value as ApprovalPolicy })}>
          {APPROVAL_POLICIES.map((p) => (
            <option key={p.value} value={p.value}>
              {p.label}
            </option>
          ))}
        </select>
      </label>
      <span className="conn" title={`event stream: ${label} · loop ${detail.active ? "running" : "stopped"}`}>
        <Dot tone={tone} /> {label}
      </span>
      <a className="button small" href={sessions.exportUrl(detail.id)} download={`oort-session-${detail.id}.json`}>
        Export
      </a>
      <button className="small danger-outline" onClick={onDelete}>
        Delete
      </button>
    </header>
  );
}
