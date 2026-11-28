import { useEffect, useState } from "react";
import { errorText, skills } from "../api";
import type { Skill, SkillInput, SkillsReload } from "../api/types";
import { Markdown } from "../components/chat/LazyMarkdown";
import { ErrorBar, Modal } from "../components/common";
import { Field, Toggle } from "../components/forms";
import { SegmentedControl } from "../components/ui";
import { AddButton, EmptyState, HeaderActions, ListItem, useAutoSelect } from "../components/settings/MasterDetail";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";
import { useT } from "../i18n";
import { MemoryIcon, RefreshIcon, SparkIcon, ToolIcon } from "../brand/Icons";

interface Form {
  name: string;
  description: string;
  body: string;
  allowed_tools: string;
  tags: string;
  user_invocable: boolean;
  argument_hint: string;
  execution_mode: "inline" | "subagent";
  subagent_profile: string;
}

const blank: Form = {
  name: "",
  description: "",
  body: "",
  allowed_tools: "",
  tags: "",
  user_invocable: true,
  argument_hint: "",
  execution_mode: "inline",
  subagent_profile: "",
};
const toForm = (k: Skill): Form => ({
  name: k.name,
  description: k.description,
  body: k.body,
  allowed_tools: k.allowed_tools.join(", "),
  tags: k.tags.join(", "),
  user_invocable: k.user_invocable,
  argument_hint: k.argument_hint ?? "",
  execution_mode: k.execution_mode,
  subagent_profile: k.subagent_profile ?? "",
});
const list = (s: string) =>
  s
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);
const toInput = (f: Form): SkillInput => ({ ...f, allowed_tools: list(f.allowed_tools), tags: list(f.tags) });

/** SKILL.md files (read-only here) and skills created in the app, with preview and editor. */
export function SkillsView({ selected }: { selected: string | null }) {
  const all = useResource(skills.list);
  const [query, setQuery] = useState("");
  const [form, setForm] = useState<Form>(blank);
  const t = useT();
  const [tab, setTab] = useState<"preview" | "edit">("preview");
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState<SkillsReload | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [copyFrom, setCopyFrom] = useState<Form | null>(null);
  const current = all.data?.find((k) => k.id === selected) ?? null;
  const isNew = selected === "new";

  useEffect(() => {
    setForm(current ? toForm(current) : (copyFrom ?? { ...blank, body: t("skills.bodyTemplate").replace(/\\n/g, "\n") }));
    setCopyFrom(null);
    setTab(isNew ? "edit" : "preview");
    setError(null);
  }, [current?.id, current?.updated_at, isNew]); // eslint-disable-line react-hooks/exhaustive-deps

  useAutoSelect("skills", all.data, selected);

  const q = query.trim().toLowerCase();
  const shown = (all.data ?? []).filter((k) => !q || `${k.name} ${k.description} ${k.tags.join(" ")}`.toLowerCase().includes(q));
  const set = <K extends keyof Form>(k: K, v: Form[K]) => setForm((f) => ({ ...f, [k]: v }));

  const save = async () => {
    try {
      const saved = current?.editable ? await skills.update(current.id, toInput(form)) : await skills.create(toInput(form));
      await all.reload();
      navigate("skills", saved.id);
    } catch (e) {
      setError(errorText(e));
    }
  };
  const rescan = async () => {
    try {
      setReload(await skills.reload());
      await all.reload();
    } catch (e) {
      setError(errorText(e));
    }
  };
  const remove = async () => {
    setConfirm(false);
    if (!current) return;
    try {
      await skills.remove(current.id);
      await all.reload();
      navigate("skills");
    } catch (e) {
      setError(errorText(e));
    }
  };
  /** A file skill becomes an editable copy in the database (same name shadows the file). */
  const copyToDb = () => {
    if (!current) return;
    setCopyFrom(toForm(current));
    navigate("skills", "new");
  };

  const editable = isNew || !!current?.editable;

  if (all.data?.length === 0 && !isNew)
    return (
      <>
        <HeaderActions>
          <button type="button" className="head-action" onClick={rescan} title={t("skills.rescanHint")}>
            <RefreshIcon size={14} />
            {t("skills.reload")}
          </button>
          <AddButton label={t("skills.create")} onClick={() => navigate("skills", "new")} />
        </HeaderActions>
        <ErrorBar error={all.error} />
        <ErrorBar error={error} onDismiss={() => setError(null)} />
        {reload && <div className="reload-note">{t("skills.loaded", { count: reload.file_skills })}{reload.warnings.map((w) => <div key={w} className="warn-text">{w}</div>)}</div>}
        <EmptyState icon={<SparkIcon size={20} />} title={t("skills.none")} action={{ label: t("skills.create"), onClick: () => navigate("skills", "new") }}>
          {t("skills.intro")} <code>~/.oort/skills/&lt;name&gt;/SKILL.md</code> {t("skills.introAfter")}
        </EmptyState>
      </>
    );

  return (
    <div className="master-detail">
      <HeaderActions>
        <button type="button" className="head-action" onClick={rescan} title={t("skills.rescanHint")}>
          <RefreshIcon size={14} />
          {t("skills.reload")}
        </button>
        <AddButton label={t("skills.create")} onClick={() => navigate("skills", "new")} />
      </HeaderActions>
      <aside className="pane left">
        <input className="filter" placeholder={t("skills.search")} value={query} onChange={(e) => setQuery(e.target.value)} />
        {reload && (
          <div className="reload-note">
            {t("skills.loaded", { count: reload.file_skills })}
            {reload.warnings.map((w) => (
              <div key={w} className="warn-text">
                {w}
              </div>
            ))}
          </div>
        )}
        <ErrorBar error={all.error} />
        <ul className="md-list">
          {shown.map((k) => (
            <ListItem
              key={k.id}
              selected={k.id === selected}
              onSelect={() => navigate("skills", k.id)}
              icon={k.source.kind === "file" ? <MemoryIcon size={15} /> : <SparkIcon size={15} />}
              iconTitle={k.source.kind === "file" ? "SKILL.md" : t("skills.app")}
              title={`/${k.name}`}
              sub={k.execution_mode === "subagent" ? `Subagent · ${k.description}` : k.description}
            />
          ))}
          {shown.length === 0 && <li className="md-none">{t("skills.none")}</li>}
        </ul>
      </aside>

      <main className="pane detail">
        <ErrorBar error={error} onDismiss={() => setError(null)} />
        {current || isNew ? (
          <div className="form wide">
            <div className="row">
              <h2>{isNew ? t("skills.newTitle") : `/${current?.name}`}</h2>
              <span className="spacer" />
              <SegmentedControl
                size="sm"
                ariaLabel={t("skills.view")}
                value={tab}
                onChange={(v) => setTab(v)}
                options={[
                  { value: "preview", label: t("skills.preview") },
                  { value: "edit", label: editable ? t("skills.edit") : t("skills.source") },
                ]}
              />
            </div>
            {current?.source.kind === "file" && (
              <div className="note">
                {t("skills.fileNoteBefore")}
                <code>{current.source.path}</code>
                {t("skills.fileNoteMid")}{" "}
                <button className="text-btn" onClick={copyToDb}>
                  {t("skills.copyToApp")}
                </button>
                {t("skills.fileNoteAfter")}
              </div>
            )}
            {tab === "preview" ? (
              <div className="skill-preview">
                <p className="lead">{form.description}</p>
                {(form.tags || form.allowed_tools) && (
                  <div className="row">
                    {list(form.tags).map((x) => (
                      <span key={x} className="chip">
                        #{x}
                      </span>
                    ))}
                    {list(form.allowed_tools).map((x) => (
                      <span key={x} className="chip mono">
                        <ToolIcon size={11} /> {x}
                      </span>
                    ))}
                  </div>
                )}
                <Markdown text={form.body || t("skills.noBody")} />
              </div>
            ) : (
              <>
                <div className="grid2">
                  <Field label={t("skills.name")} hint={t("skills.nameHint")}>
                    <input className="mono" value={form.name} disabled={!editable} onChange={(e) => set("name", e.target.value)} />
                  </Field>
                  <Field label={t("skills.tags")} hint={t("skills.tagsHint")}>
                    <input value={form.tags} disabled={!editable} onChange={(e) => set("tags", e.target.value)} />
                  </Field>
                </div>
                <Field label={t("skills.description")} hint={t("skills.descriptionHint")}>
                  <input value={form.description} disabled={!editable} onChange={(e) => set("description", e.target.value)} />
                </Field>
                <div className="grid2">
                  <Field label={t("skills.invocation")} hint={t("skills.invocationHint")}>
                    <Toggle checked={form.user_invocable} onChange={(v) => editable && set("user_invocable", v)} label={t("skills.showAsCommand")} />
                  </Field>
                  <Field label={t("skills.argHint")} hint={t("skills.argHintHint")}>
                    <input className="mono" value={form.argument_hint} disabled={!editable} placeholder="<target>" onChange={(e) => set("argument_hint", e.target.value)} />
                  </Field>
                </div>
                <div className="grid2">
                  <Field label={t("skills.execMode")} hint={t("skills.execModeHint")}>
                    <SegmentedControl
                      ariaLabel={t("skills.execMode")}
                      disabled={!editable}
                      value={form.execution_mode}
                      onChange={(v) => set("execution_mode", v)}
                      options={[
                        { value: "inline", label: t("skills.inline"), title: t("skills.inlineHint") },
                        { value: "subagent", label: t("skills.subagent"), title: t("skills.subagentHint") },
                      ]}
                    />
                  </Field>
                  {form.execution_mode === "subagent" && (
                    <Field label={t("skills.profile")} hint={t("skills.profileHint")}>
                      <input
                        className="mono"
                        list="subagent-profiles"
                        value={form.subagent_profile}
                        disabled={!editable}
                        placeholder="explorer"
                        onChange={(e) => set("subagent_profile", e.target.value)}
                      />
                      <datalist id="subagent-profiles">
                        <option value="explorer">{t("subagent.explorer")}</option>
                        <option value="researcher">{t("subagent.researcher")}</option>
                        <option value="reviewer">{t("subagent.reviewer")}</option>
                      </datalist>
                    </Field>
                  )}
                </div>
                <Field label={t("skills.tools")} hint={t("skills.toolsHint")}>
                  <input className="mono" value={form.allowed_tools} disabled={!editable} onChange={(e) => set("allowed_tools", e.target.value)} />
                </Field>
                <Field label={t("skills.body")}>
                  <textarea className="mono" rows={16} value={form.body} disabled={!editable} onChange={(e) => set("body", e.target.value)} />
                </Field>
                {editable && (
                  <div className="row actions">
                    <button className="btn primary" onClick={save}>
                      {isNew ? t("common.create") : t("skills.save")}
                    </button>
                    <span className="spacer" />
                    {current && (
                      <button className="btn danger-outline" onClick={() => setConfirm(true)}>
                        {t("common.delete")}
                      </button>
                    )}
                  </div>
                )}
              </>
            )}
          </div>
        ) : (
          null
        )}
      </main>
      {confirm && current && (
        <Modal title={t("skills.deleteTitle")} onClose={() => setConfirm(false)}>
          <p>
            {t("skills.deleteBody", { name: current.name })}
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
