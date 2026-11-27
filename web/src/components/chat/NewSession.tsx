import { useState } from "react";
import { errorText, mcpServers, presets, providers, sessions } from "../../api";
import { APPROVAL_POLICIES, type ApprovalPolicy, type CreateSessionRequest } from "../../api/types";
import { useResource } from "../../hooks/useResource";
import { Badge, Modal } from "../common";
import { Field, Toggle } from "../forms";

/** Start a session from a preset in one click, or from an ad-hoc combination. */
export function NewSessionDialog({ onCreated, onClose }: { onCreated: (id: string) => void; onClose: () => void }) {
  const presetList = useResource(presets.list);
  const providerList = useResource(providers.list);
  const serverList = useResource(mcpServers.list);
  const [tab, setTab] = useState<"preset" | "custom">("preset");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState("");

  const [providerId, setProviderId] = useState("");
  const [model, setModel] = useState("");
  const [serverIds, setServerIds] = useState<string[]>([]);
  const [builtin, setBuiltin] = useState(true);
  const [policy, setPolicy] = useState<ApprovalPolicy>("auto_readonly");
  const [system, setSystem] = useState("You are a helpful assistant.");
  const [title, setTitle] = useState("");

  const enabledProviders = (providerList.data ?? []).filter((p) => p.is_enabled);
  const provider = enabledProviders.find((p) => p.id === providerId) ?? enabledProviders[0];

  const create = async (req: CreateSessionRequest) => {
    setBusy(true);
    setError(null);
    try {
      const d = await sessions.create({ ...req, message: message.trim() || undefined });
      onCreated(d.id);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="New session" onClose={onClose}>
      <div className="seg">
        <button className={tab === "preset" ? "on" : ""} onClick={() => setTab("preset")}>
          From preset
        </button>
        <button className={tab === "custom" ? "on" : ""} onClick={() => setTab("custom")}>
          Custom
        </button>
      </div>

      <Field label="First message (optional)">
        <textarea rows={2} value={message} onChange={(e) => setMessage(e.target.value)} placeholder="starts the first turn right away" />
      </Field>

      {tab === "preset" ? (
        <ul className="preset-picker">
          {(presetList.data ?? []).map((p) => (
            <li key={p.id}>
              <div>
                <b>{p.name}</b>
                <div className="muted">{p.description || p.system_prompt.slice(0, 80)}</div>
                <div className="muted mono">
                  {providerList.data?.find((x) => x.id === p.provider_id)?.name ?? p.provider_id} / {p.model || "default model"} ·{" "}
                  {p.mcp_server_ids.length} MCP · {p.approval_policy}
                </div>
              </div>
              <button className="primary" disabled={busy} onClick={() => create({ preset_id: p.id })}>
                Start
              </button>
            </li>
          ))}
          {presetList.data?.length === 0 && <li className="muted">No presets yet — create one in Presets, or use Custom.</li>}
        </ul>
      ) : (
        <div className="form">
          <div className="grid2">
            <Field label="Provider">
              <select value={provider?.id ?? ""} onChange={(e) => setProviderId(e.target.value)}>
                {enabledProviders.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name} ({p.kind})
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Model">
              <input value={model} placeholder={provider?.default_model || "model id"} onChange={(e) => setModel(e.target.value)} />
            </Field>
          </div>
          <Field label="MCP servers" hint="Their tools are frozen into the session when it is created.">
            <div className="checks">
              {(serverList.data ?? []).map((s) => (
                <Toggle
                  key={s.id}
                  checked={serverIds.includes(s.id)}
                  onChange={(on) => setServerIds(on ? [...serverIds, s.id] : serverIds.filter((x) => x !== s.id))}
                  label={
                    <>
                      <span className="mono">{s.name}</span> {!s.is_enabled && <Badge tone="muted">disabled</Badge>}
                    </>
                  }
                />
              ))}
              {serverList.data?.length === 0 && <span className="muted">none configured</span>}
            </div>
          </Field>
          <div className="grid2">
            <Field label="Approval policy">
              <select value={policy} onChange={(e) => setPolicy(e.target.value as ApprovalPolicy)}>
                {APPROVAL_POLICIES.map((p) => (
                  <option key={p.value} value={p.value}>
                    {p.label} — {p.hint}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Title">
              <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="optional" />
            </Field>
          </div>
          <Toggle checked={builtin} onChange={setBuiltin} label="Built-in tools (get_time, list_dir, read_file, write_note)" />
          <Field label="System prompt" hint="Frozen for the lifetime of the session (prefix cache).">
            <textarea rows={4} value={system} onChange={(e) => setSystem(e.target.value)} />
          </Field>
          <div className="row">
            <span className="spacer" />
            <button
              className="primary"
              disabled={busy || !provider}
              onClick={() =>
                create({
                  provider_id: provider?.id,
                  model: model.trim() || undefined,
                  mcp_server_ids: serverIds,
                  builtin_tools: builtin,
                  approval_policy: policy,
                  system_prompt: system,
                  title: title.trim() || undefined,
                })
              }
            >
              {busy ? "Connecting…" : "Create session"}
            </button>
          </div>
        </div>
      )}
      {error && <div className="field-error">{error}</div>}
    </Modal>
  );
}
