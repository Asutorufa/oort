import { useEffect, useState } from "react";
import { errorText, mcpServers, presets, providers, sessions, skills } from "../../api";
import type { CreateSessionRequest } from "../../api/types";
import { ToolIcon } from "../../brand/Icons";
import { OortMark } from "../../brand/OortMark";
import { useOnReturn } from "../../hooks/useOnReturn";
import { useResource } from "../../hooks/useResource";
import { ErrorBar } from "../common";
import { GroupToggles } from "../forms";
import { Composer } from "./Composer";
import { ModelPicker } from "./ModelPicker";
import { useT } from "../../i18n";

/** The new-chat screen: pick a preset (or a model, skills and tools) and just start typing. */
export function Welcome({ visible, onCreated }: { visible: boolean; onCreated: (id: string) => void }) {
  const presetList = useResource(presets.list);
  const providerList = useResource(providers.list);
  const serverList = useResource(mcpServers.list);
  const skillList = useResource(skills.list);
  const [presetId, setPresetId] = useState<string | null>(null);
  const t = useT();
  const [providerId, setProviderId] = useState<string>("");
  const [model, setModel] = useState("");
  const [skillIds, setSkillIds] = useState<string[]>([]);
  const [serverIds, setServerIds] = useState<string[]>([]);
  const [groups, setGroups] = useState<string[]>(() => JSON.parse(localStorage.getItem("oort.groups") ?? '["core_ux","memory","utilities"]'));
  const [workspace, setWorkspace] = useState(() => localStorage.getItem("oort.workspace") ?? "");
  const [error, setError] = useState<string | null>(null);
  useOnReturn(visible, () => {
    presetList.reload();
    providerList.reload();
    serverList.reload();
    skillList.reload();
  });

  // A preset deleted in settings is no longer selected.
  useEffect(() => {
    if (presetId && presetList.data && !presetList.data.some((p) => p.id === presetId)) setPresetId(null);
  }, [presetId, presetList.data]);

  const enabled = (providerList.data ?? []).filter((p) => p.is_enabled);
  const provider = enabled.find((p) => p.id === providerId) ?? enabled[0];
  const toggle = (list: string[], id: string) => (list.includes(id) ? list.filter((x) => x !== id) : [...list, id]);

  const start = async (message: string) => {
    setError(null);
    const req: CreateSessionRequest = presetId
      ? { preset_id: presetId, message }
      : {
          provider_id: provider?.id,
          model: model.trim() || undefined,
          skill_ids: skillIds,
          mcp_server_ids: serverIds,
          builtin_groups: groups,
          workspace_root: groups.includes("workspace") ? workspace.trim() : undefined,
          message,
        };
    localStorage.setItem("oort.groups", JSON.stringify(groups));
    localStorage.setItem("oort.workspace", workspace);
    try {
      const d = await sessions.create(req);
      onCreated(d.id);
    } catch (e) {
      setError(errorText(e));
      throw e;
    }
  };

  return (
    <div className="welcome">
      {/* Empty, but keeps the 48px top line continuous with the sidebar header. */}
      <header className="chat-top" />
      <div className="welcome-inner">
        <OortMark size={36} />
        <h1>{t("welcome.title")}</h1>
        {(presetList.data ?? []).length > 0 && (
          <div className="preset-row">
            {(presetList.data ?? []).map((p) => (
              <button key={p.id} className={`preset-chip ${presetId === p.id ? "on" : ""}`} onClick={() => setPresetId(presetId === p.id ? null : p.id)} title={p.description}>
                {p.name}
              </button>
            ))}
          </div>
        )}
        {!presetId && (
          <div className="quick-setup">
            <ModelPicker providers={enabled} provider={provider} model={model} onProvider={setProviderId} onModel={setModel} />
            {(skillList.data ?? []).map((s) => (
              <button key={s.id} className={`toggle-chip ${skillIds.includes(s.id) ? "on" : ""}`} onClick={() => setSkillIds(toggle(skillIds, s.id))} title={s.description}>
                /{s.name}
              </button>
            ))}
            {(serverList.data ?? []).filter((s) => s.is_enabled).map((s) => (
              <button key={s.id} className={`toggle-chip ${serverIds.includes(s.id) ? "on" : ""}`} onClick={() => setServerIds(toggle(serverIds, s.id))}>
                <ToolIcon /> {s.name}
              </button>
            ))}
          </div>
        )}
        {!presetId && (
          <GroupToggles value={groups} onChange={setGroups} workspaceRoot={workspace} onWorkspaceRoot={setWorkspace} compact />
        )}
        <ErrorBar error={error} onDismiss={() => setError(null)} />
      </div>
      <Composer busy={false} commands={[]} onSend={(text) => start(text)} autoFocus placeholder={t("composer.placeholderNew")} />
    </div>
  );
}
