import { useEffect, useState } from "react";
import { errorText, mcpServers, presets, providers, sessions, skills } from "../api";
import { APPROVAL_POLICIES, type ApprovalPolicy, type Preset, type PresetInput } from "../api/types";
import { Badge, Empty, ErrorBar, Modal } from "../components/common";
import { Field, GroupToggles, Toggle } from "../components/forms";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";

type Form = Required<PresetInput>;

const blank: Form = {
  name: "",
  description: "",
  provider_id: "",
  model: "",
  system_prompt: "You are a helpful assistant.",
  mcp_server_ids: [],
  builtin_tools: false,
  approval_policy: "auto_readonly",
  skill_ids: [],
  builtin_groups: ["core_ux", "memory", "utilities"],
  workspace_root: null,
};

const toForm = (p: Preset): Form => ({
  name: p.name,
  description: p.description,
  provider_id: p.provider_id,
  model: p.model,
  system_prompt: p.system_prompt,
  mcp_server_ids: p.mcp_server_ids,
  builtin_tools: p.builtin_tools,
  approval_policy: p.approval_policy,
  skill_ids: p.skill_ids ?? [],
  builtin_groups: p.builtin_groups ?? [],
  workspace_root: p.workspace_root,
});

export function PresetsView({ selected }: { selected: string | null }) {
  const list = useResource(presets.list);
  const providerList = useResource(providers.list);
  const serverList = useResource(mcpServers.list);
  const skillList = useResource(skills.list);
  const current = list.data?.find((p) => p.id === selected) ?? null;
  const isNew = selected === "new";
  const [form, setForm] = useState<Form>(blank);
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState<string | null>(null);
  const [confirm, setConfirm] = useState(false);

  useEffect(() => {
    setForm(current ? toForm(current) : { ...blank, provider_id: providerList.data?.[0]?.id ?? "" });
    setError(null);
  }, [current?.id, current?.updated_at, isNew, providerList.data]); // eslint-disable-line react-hooks/exhaustive-deps

  const set = <K extends keyof Form>(k: K, v: Form[K]) => setForm((f) => ({ ...f, [k]: v }));
  const providerName = (id: string) => providerList.data?.find((p) => p.id === id)?.name ?? id;
  const serverName = (id: string) => serverList.data?.find((s) => s.id === id)?.name ?? "(deleted)";
  const provider = providerList.data?.find((p) => p.id === form.provider_id);

  const save = async () => {
    try {
      const saved = current ? await presets.update(current.id, form) : await presets.create(form);
      await list.reload();
      navigate("presets", saved.id);
    } catch (e) {
      setError(errorText(e));
    }
  };

  const start = async (p: Preset) => {
    setStarting(p.id);
    setError(null);
    try {
      const d = await sessions.create({ preset_id: p.id });
      navigate("chat", d.id);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setStarting(null);
    }
  };

  const remove = async () => {
    setConfirm(false);
    if (!current) return;
    try {
      await presets.remove(current.id);
      await list.reload();
      navigate("presets");
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <div className="master-detail">
      <aside className="pane left">
        <div className="pane-head">
          <h3>Presets</h3>
          <button className="primary small" onClick={() => navigate("presets", "new")}>
            + Add
          </button>
        </div>
        <ErrorBar error={list.error} />
        <ul className="item-list">
          {(list.data ?? []).map((p) => (
            <li key={p.id} className={p.id === selected ? "selected" : ""} onClick={() => navigate("presets", p.id)}>
              <div className="row">
                <b>{p.name}</b>
                <span className="spacer" />
                <button
                  className="small primary"
                  disabled={starting === p.id}
                  title="Start a new session with this preset"
                  onClick={(e) => {
                    e.stopPropagation();
                    start(p);
                  }}
                >
                  {starting === p.id ? "…" : "▶ Start"}
                </button>
              </div>
              <div className="muted ellipsis">{p.description || p.system_prompt}</div>
              <div className="row muted">
                <span>{providerName(p.provider_id)}</span>
                <span className="mono">/ {p.model || "default"}</span>
                {p.mcp_server_ids.length > 0 && <span>· {p.mcp_server_ids.map(serverName).join(", ")}</span>}
              </div>
            </li>
          ))}
          {list.data?.length === 0 && <li className="muted">none yet</li>}
        </ul>
      </aside>

      <main className="pane detail">
        <ErrorBar error={error} onDismiss={() => setError(null)} />
        {current || isNew ? (
          <div className="form">
            <div className="row">
              <h2>{current ? current.name : "New preset"}</h2>
              <span className="spacer" />
              {current && (
                <button className="primary" disabled={starting === current.id} onClick={() => start(current)}>
                  Start session with this preset
                </button>
              )}
            </div>
            <div className="grid2">
              <Field label="Name">
                <input value={form.name} onChange={(e) => set("name", e.target.value)} />
              </Field>
              <Field label="Description">
                <input value={form.description} onChange={(e) => set("description", e.target.value)} />
              </Field>
            </div>
            <div className="grid2">
              <Field label="Provider">
                <select value={form.provider_id} onChange={(e) => set("provider_id", e.target.value)}>
                  <option value="" disabled>
                    select a provider
                  </option>
                  {(providerList.data ?? []).map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name} ({p.kind}){p.is_enabled ? "" : " — disabled"}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="Model" hint="empty: the provider's default model">
                <input value={form.model} placeholder={provider?.default_model || "model id"} onChange={(e) => set("model", e.target.value)} />
              </Field>
            </div>
            <Field label="System prompt">
              <textarea rows={6} value={form.system_prompt} onChange={(e) => set("system_prompt", e.target.value)} />
            </Field>
            <Field label="MCP servers">
              <div className="checks">
                {(serverList.data ?? []).map((s) => (
                  <Toggle
                    key={s.id}
                    checked={form.mcp_server_ids.includes(s.id)}
                    onChange={(on) =>
                      set("mcp_server_ids", on ? [...form.mcp_server_ids, s.id] : form.mcp_server_ids.filter((x) => x !== s.id))
                    }
                    label={
                      <>
                        <span className="mono">{s.name}</span> {!s.is_enabled && <Badge>disabled</Badge>}
                      </>
                    }
                  />
                ))}
                {serverList.data?.length === 0 && <span className="muted">none configured</span>}
              </div>
            </Field>
            <Field label="組み込みツール" hint="会話の開始時に固定されます。Local Workspace はフォルダの指定が必要です">
              <GroupToggles
                value={form.builtin_groups}
                onChange={(v) => set("builtin_groups", v)}
                workspaceRoot={form.workspace_root ?? ""}
                onWorkspaceRoot={(v) => set("workspace_root", v)}
              />
            </Field>
            <Field label="Skills" hint="名前と説明だけが会話の最初に固定され、本文は必要なときに読み込まれます">
              <div className="checks">
                {(skillList.data ?? []).map((k) => (
                  <Toggle
                    key={k.id}
                    checked={form.skill_ids.includes(k.id)}
                    onChange={(on) => set("skill_ids", on ? [...form.skill_ids, k.id] : form.skill_ids.filter((x) => x !== k.id))}
                    label={
                      <span title={k.description}>
                        <span className="mono">/{k.name}</span>
                      </span>
                    }
                  />
                ))}
                {skillList.data?.length === 0 && <span className="muted">Skill がありません</span>}
              </div>
            </Field>
            <div className="grid2">
              <Field label="Approval policy">
                <select value={form.approval_policy} onChange={(e) => set("approval_policy", e.target.value as ApprovalPolicy)}>
                  {APPROVAL_POLICIES.map((p) => (
                    <option key={p.value} value={p.value}>
                      {p.label} — {p.hint}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="旧ツール（互換用）" hint="組み込みツールを 1 つでも選ぶと使われません">
                <Toggle checked={form.builtin_tools} onChange={(v) => set("builtin_tools", v)} label="get_time, list_dir, read_file, write_note" />
              </Field>
            </div>
            <div className="row actions">
              <button className="primary" onClick={save}>
                {current ? "Save" : "Create"}
              </button>
              <span className="spacer" />
              {current && (
                <button className="danger-outline" onClick={() => setConfirm(true)}>
                  Delete
                </button>
              )}
            </div>
          </div>
        ) : (
          <Empty>Presets combine a provider, model, system prompt, MCP servers and an approval policy. Select one or add one.</Empty>
        )}
      </main>
      {confirm && current && (
        <Modal title="Delete preset?" onClose={() => setConfirm(false)}>
          <p>
            <b>{current.name}</b> will be deleted. Sessions started from it are not affected.
          </p>
          <div className="row">
            <span className="spacer" />
            <button onClick={() => setConfirm(false)}>Cancel</button>
            <button className="danger" onClick={remove}>
              Delete
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
