import { useState } from "react";
import { errorText, mcpServers, presets, providers, sessions, skills } from "../../api";
import type { CreateSessionRequest } from "../../api/types";
import { ToolIcon } from "../../brand/Icons";
import { OortMark } from "../../brand/OortMark";
import { useResource } from "../../hooks/useResource";
import { ErrorBar } from "../common";
import { Composer } from "./Composer";
import { ModelPicker } from "./ModelPicker";

/** The new-chat screen: pick a preset (or a model, skills and tools) and just start typing. */
export function Welcome({ onCreated }: { onCreated: (id: string) => void }) {
  const presetList = useResource(presets.list);
  const providerList = useResource(providers.list);
  const serverList = useResource(mcpServers.list);
  const skillList = useResource(skills.list);
  const [presetId, setPresetId] = useState<string | null>(null);
  const [providerId, setProviderId] = useState<string>("");
  const [model, setModel] = useState("");
  const [skillIds, setSkillIds] = useState<string[]>([]);
  const [serverIds, setServerIds] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  const enabled = (providerList.data ?? []).filter((p) => p.is_enabled);
  const provider = enabled.find((p) => p.id === providerId) ?? enabled[0];
  const toggle = (list: string[], id: string) => (list.includes(id) ? list.filter((x) => x !== id) : [...list, id]);

  const start = async (message: string) => {
    setError(null);
    // The first line of the first message names the conversation (rename any time).
    const firstLine = message.split("\n")[0].trim();
    const title = firstLine.length > 40 ? `${firstLine.slice(0, 40)}…` : firstLine;
    const req: CreateSessionRequest = presetId
      ? { preset_id: presetId, message, title }
      : { provider_id: provider?.id, model: model.trim() || undefined, skill_ids: skillIds, mcp_server_ids: serverIds, message, title };
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
      <div className="welcome-inner">
        <OortMark size={48} />
        <h1>何をお手伝いしましょう？</h1>
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
                ✦ {s.name}
              </button>
            ))}
            {(serverList.data ?? []).filter((s) => s.is_enabled).map((s) => (
              <button key={s.id} className={`toggle-chip ${serverIds.includes(s.id) ? "on" : ""}`} onClick={() => setServerIds(toggle(serverIds, s.id))}>
                <ToolIcon /> {s.name}
              </button>
            ))}
          </div>
        )}
        <ErrorBar error={error} onDismiss={() => setError(null)} />
      </div>
      <Composer busy={false} skills={[]} onSend={start} autoFocus placeholder="メッセージを入力して新しいチャットを始める" />
    </div>
  );
}
