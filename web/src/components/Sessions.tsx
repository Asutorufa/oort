import { useState } from "react";
import { api } from "../api";
import type { ApprovalMode, ProviderKind, SessionSummary } from "../types";

const DEFAULT_MODEL: Record<ProviderKind, string> = {
  mock: "mock",
  anthropic: "claude-sonnet-5-5",
  openai: "gpt-4.1",
  openai_responses: "gpt-4.1",
  gemini: "gemini-2.5-flash",
};

export function SessionList({ sessions, selected, onSelect }: {
  sessions: SessionSummary[];
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  return (
    <ul className="sessions">
      {sessions.map((s) => (
        <li key={s.id} className={s.id === selected ? "selected" : ""} onClick={() => onSelect(s.id)}>
          <div className="row">
            <b>{s.title || s.id.slice(0, 8)}</b>
            <span className={`badge ${s.status}`}>{s.status}</span>
          </div>
          <div className="muted">
            {s.provider}/{s.model} · {s.message_count} msgs · {s.active ? "● live" : "○"}
          </div>
        </li>
      ))}
    </ul>
  );
}

export function NewSessionForm({ onCreated, onError }: { onCreated: (id: string) => void; onError: (e: string) => void }) {
  const [provider, setProvider] = useState<ProviderKind>("mock");
  const [model, setModel] = useState(DEFAULT_MODEL.mock);
  const [title, setTitle] = useState("");
  const [system, setSystem] = useState("You are a helpful agent. Use tools when useful.");
  const [maxCtx, setMaxCtx] = useState(200_000);
  const [approval, setApproval] = useState<ApprovalMode>("mutating");
  const [message, setMessage] = useState("");

  const submit = async () => {
    try {
      const d = await api.createSession({
        title, provider, model, system_prompt: system, max_context_tokens: maxCtx, approval,
        message: message.trim() || undefined,
      });
      setMessage("");
      onCreated(d.id);
    } catch (e) {
      onError(String(e));
    }
  };

  return (
    <details className="new-session">
      <summary>+ New session</summary>
      <label>Title<input value={title} onChange={(e) => setTitle(e.target.value)} /></label>
      <label>
        Provider
        <select value={provider} onChange={(e) => {
          const p = e.target.value as ProviderKind;
          setProvider(p);
          setModel(DEFAULT_MODEL[p]);
        }}>
          {Object.keys(DEFAULT_MODEL).map((p) => <option key={p}>{p}</option>)}
        </select>
      </label>
      <label>Model<input value={model} onChange={(e) => setModel(e.target.value)} /></label>
      <label>System prompt<textarea rows={3} value={system} onChange={(e) => setSystem(e.target.value)} /></label>
      <label>Max context tokens<input type="number" value={maxCtx} onChange={(e) => setMaxCtx(Number(e.target.value))} /></label>
      <label>
        Approval
        <select value={approval} onChange={(e) => setApproval(e.target.value as ApprovalMode)}>
          <option value="mutating">mutating tools need approval</option>
          <option value="never">never ask</option>
        </select>
      </label>
      <label>First message (optional)<textarea rows={2} value={message} onChange={(e) => setMessage(e.target.value)} /></label>
      <button className="primary" onClick={submit}>Create</button>
      {provider === "mock" && (
        <p className="muted">Mock keywords: time, list, read &lt;path&gt;, note &lt;text&gt;, think, slow, or any tool name.</p>
      )}
    </details>
  );
}
