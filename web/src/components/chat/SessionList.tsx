import { useState } from "react";
import { errorText, sessions } from "../../api";
import type { SessionSummary } from "../../api/types";
import { StatusBadge } from "../common";

const ago = (ms: number) => {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return new Date(ms).toLocaleDateString();
};

/** Session list with inline rename, export and delete. */
export function SessionList({
  items,
  selected,
  onSelect,
  onChanged,
  onDelete,
  onError,
}: {
  items: SessionSummary[];
  selected: string | null;
  onSelect: (id: string) => void;
  onChanged: () => void;
  onDelete: (s: SessionSummary) => void;
  onError: (e: string) => void;
}) {
  const [editing, setEditing] = useState<string | null>(null);
  const [title, setTitle] = useState("");
  const [filter, setFilter] = useState("");
  const shown = items.filter((s) => `${s.title} ${s.model} ${s.provider} ${s.id}`.toLowerCase().includes(filter.toLowerCase()));

  const rename = async (id: string, value: string) => {
    if (editing !== id) return; // already committed (Enter, then blur)
    setEditing(null);
    try {
      await sessions.patch(id, { title: value });
      onChanged();
    } catch (e) {
      onError(errorText(e));
    }
  };

  return (
    <div className="session-list">
      <input className="filter" placeholder="filter sessions" value={filter} onChange={(e) => setFilter(e.target.value)} />
      <ul>
        {shown.map((s) => (
          <li key={s.id} className={s.id === selected ? "selected" : ""} onClick={() => onSelect(s.id)}>
            {editing === s.id ? (
              <input
                autoFocus
                value={title}
                onClick={(e) => e.stopPropagation()}
                onChange={(e) => setTitle(e.target.value)}
                onBlur={(e) => rename(s.id, e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") rename(s.id, e.currentTarget.value);
                  if (e.key === "Escape") setEditing(null);
                }}
              />
            ) : (
              <div className="row">
                <span className="session-title" title={s.id}>
                  {s.title || <span className="muted">untitled</span>}
                </span>
                {s.active && <span className="dot info" title="loop running" />}
                <StatusBadge status={s.status} />
              </div>
            )}
            <div className="row session-meta">
              <span className="muted mono">{s.model}</span>
              <span className="muted">
                · {s.message_count} msgs · {ago(s.updated_at)}
              </span>
              <span className="spacer" />
              <span className="item-actions" onClick={(e) => e.stopPropagation()}>
                <button
                  className="small ghost"
                  title="rename"
                  onClick={() => {
                    setTitle(s.title);
                    setEditing(s.id);
                  }}
                >
                  ✎
                </button>
                <a className="button small ghost" title="export JSON trace" href={sessions.exportUrl(s.id)} download={`oort-session-${s.id}.json`}>
                  ⤓
                </a>
                <button className="small ghost" title="delete" onClick={() => onDelete(s)}>
                  🗑
                </button>
              </span>
            </div>
          </li>
        ))}
        {shown.length === 0 && <li className="muted">no sessions</li>}
      </ul>
    </div>
  );
}
