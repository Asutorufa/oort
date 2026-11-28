import { useEffect, useState } from "react";
import { errorText, mcpServers, presets, providers, sessions, skills } from "../api";
import { APPROVAL_POLICIES, REASONING_LEVELS, type ApprovalPolicy, type Preset, type PresetInput, type ReasoningEffort } from "../api/types";
import { KindIcon, LayersIcon } from "../brand/Icons";
import { Checkbox, SegmentedControl, Select, Switch } from "../components/ui";
import { Badge, ErrorBar, Modal } from "../components/common";
import { Field, GroupToggles } from "../components/forms";
import { AddButton, EmptyState, HeaderActions, ListItem, useAutoSelect } from "../components/settings/MasterDetail";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";
import { useT } from "../i18n";

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
  reasoning_effort: "default",
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
  reasoning_effort: p.reasoning_effort ?? "default",
});

export function PresetsView({ selected }: { selected: string | null }) {
  const t = useT();
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

  useAutoSelect("presets", list.data, selected);

  const set = <K extends keyof Form>(k: K, v: Form[K]) => setForm((f) => ({ ...f, [k]: v }));
  const providerName = (id: string) => providerList.data?.find((p) => p.id === id)?.name ?? id;
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

  if (list.data?.length === 0 && !isNew)
    return (
      <>
        <HeaderActions>
          <AddButton label={t("common.add")} onClick={() => navigate("presets", "new")} />
        </HeaderActions>
        <ErrorBar error={list.error} />
        <EmptyState icon={<LayersIcon size={20} />} title={t("presets.emptyTitle")} action={{ label: t("common.add"), onClick: () => navigate("presets", "new") }}>
          {t("presets.emptyHint")}
        </EmptyState>
      </>
    );

  return (
    <div className="master-detail">
      <HeaderActions>
        <AddButton label={t("common.add")} onClick={() => navigate("presets", "new")} />
      </HeaderActions>
      <aside className="pane left">
        <ErrorBar error={list.error} />
        <ul className="md-list">
          {(list.data ?? []).map((p) => (
            <ListItem
              key={p.id}
              selected={p.id === selected}
              onSelect={() => navigate("presets", p.id)}
              icon={<LayersIcon size={15} />}
              title={p.name}
              sub={`${providerName(p.provider_id)} / ${p.model || t("presets.defaultModel")}`}
              mono
            />
          ))}
        </ul>
      </aside>

      <main className="pane detail">
        <ErrorBar error={error} onDismiss={() => setError(null)} />
        {current || isNew ? (
          <div className="form">
            <div className="row">
              <h2>{current ? current.name : t("presets.new")}</h2>
              <span className="spacer" />
              {current && (
                <button className="btn small" disabled={starting === current.id} title={t("presets.startHint")} onClick={() => start(current)}>
                  {t("presets.start")}
                </button>
              )}
            </div>
            <div className="grid2">
              <Field label={t("presets.name")}>
                <input value={form.name} onChange={(e) => set("name", e.target.value)} />
              </Field>
              <Field label={t("presets.description")}>
                <input value={form.description} onChange={(e) => set("description", e.target.value)} />
              </Field>
            </div>
            <div className="grid2">
              <Field label={t("presets.provider")}>
                <Select
                  ariaLabel={t("presets.provider")}
                  placeholder={t("presets.pickProvider")}
                  value={form.provider_id || null}
                  onChange={(v) => set("provider_id", v)}
                  options={(providerList.data ?? []).map((p) => ({
                    value: p.id,
                    label: p.name,
                    description: `${p.default_model || t("presets.noDefaultModel")}${p.is_enabled ? "" : ` · ${t("common.disabled")}`}`,
                    icon: <KindIcon kind={p.kind} size={15} />,
                  }))}
                />
              </Field>
              <Field label={t("presets.model")} hint={t("presets.modelHint")}>
                <input value={form.model} placeholder={provider?.default_model || t("presets.modelName")} onChange={(e) => set("model", e.target.value)} />
              </Field>
            </div>
            <Field label={t("presets.systemPrompt")}>
              <textarea rows={6} value={form.system_prompt} onChange={(e) => set("system_prompt", e.target.value)} />
            </Field>
            <Field label={t("presets.mcp")}>
              <div className="checks">
                {(serverList.data ?? []).map((s) => (
                  <Checkbox
                    key={s.id}
                    checked={form.mcp_server_ids.includes(s.id)}
                    onChange={(on) =>
                      set("mcp_server_ids", on ? [...form.mcp_server_ids, s.id] : form.mcp_server_ids.filter((x) => x !== s.id))
                    }
                    label={
                      <>
                        <span className="mono">{s.name}</span> {!s.is_enabled && <Badge>{t("common.disabled")}</Badge>}
                      </>
                    }
                  />
                ))}
                {serverList.data?.length === 0 && <span className="muted">{t("presets.noMcp")}</span>}
              </div>
            </Field>
            <Field label={t("presets.groups")} hint={t("presets.groupsHint")}>
              <GroupToggles
                value={form.builtin_groups}
                onChange={(v) => set("builtin_groups", v)}
                workspaceRoot={form.workspace_root ?? ""}
                onWorkspaceRoot={(v) => set("workspace_root", v)}
              />
            </Field>
            <Field label={t("presets.skills")} hint={t("presets.skillsHint")}>
              <div className="checks">
                {(skillList.data ?? []).map((k) => (
                  <Checkbox
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
                {skillList.data?.length === 0 && <span className="muted">{t("skills.none")}</span>}
              </div>
            </Field>
            <div className="grid2">
              <Field label={t("policy.label")}>
                <Select<ApprovalPolicy>
                  ariaLabel={t("policy.label")}
                  value={form.approval_policy}
                  onChange={(v) => set("approval_policy", v)}
                  options={APPROVAL_POLICIES.map((p) => ({ value: p.value, label: t(p.label), description: t(p.hint) }))}
                />
              </Field>
              <Field label={t("reasoning.label")} hint={t(REASONING_LEVELS.find((l) => l.value === form.reasoning_effort)?.description ?? "reasoning.default.desc")}>
                <SegmentedControl<ReasoningEffort>
                  full
                  size="sm"
                  ariaLabel={t("reasoning.label")}
                  value={form.reasoning_effort}
                  onChange={(v) => set("reasoning_effort", v)}
                  options={REASONING_LEVELS.map((l) => ({ value: l.value, label: t(l.label), title: t(l.description) }))}
                />
              </Field>
            </div>
            <Switch
              checked={form.builtin_tools}
              onChange={(v) => set("builtin_tools", v)}
              label={t("presets.legacy")}
              description={t("presets.legacyHint")}
            />
            <div className="row actions">
              <button className="primary" onClick={save}>
                {current ? t("common.save") : t("common.create")}
              </button>
              <span className="spacer" />
              {current && (
                <button className="danger-outline" onClick={() => setConfirm(true)}>
                  {t("common.delete")}
                </button>
              )}
            </div>
          </div>
        ) : (
          null
        )}
      </main>
      {confirm && current && (
        <Modal title={t("presets.deleteTitle")} onClose={() => setConfirm(false)}>
          <p>
            {t("presets.deleteBody", { name: current.name })}
          </p>
          <div className="row">
            <span className="spacer" />
            <button className="btn" onClick={() => setConfirm(false)}>
              {t("common.keep")}
            </button>
            <button className="btn danger" onClick={remove}>
              {t("common.deleteConfirm")}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
