import { useEffect, useState } from "react";
import { errorText, memories } from "../api";
import type { Memory } from "../api/types";
import { MemoryIcon } from "../brand/Icons";
import { ErrorBar, Modal } from "../components/common";
import { useI18n } from "../i18n";
import { Field } from "../components/forms";
import { AddButton, EmptyState, HeaderActions, ListItem, useAutoSelect } from "../components/settings/MasterDetail";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";

interface Form {
  title: string;
  content: string;
  tags: string;
  scope: string;
}

const blank: Form = { title: "", content: "", tags: "", scope: "global" };
const toForm = (m: Memory): Form => ({ title: m.title, content: m.content, tags: m.tags.join(", "), scope: m.scope });
const tagsOf = (s: string) =>
  s
    .split(",")
    .map((t) => t.trim())
    .filter(Boolean);

/** What the agent (or you) saved across conversations: full-text search, edit, delete. */
export function MemoriesView({ selected }: { selected: string | null }) {
  const [query, setQuery] = useState("");
  const { t, fmt } = useI18n();
  const [debounced, setDebounced] = useState("");
  useEffect(() => {
    const t = window.setTimeout(() => setDebounced(query), 220);
    return () => window.clearTimeout(t);
  }, [query]);
  const list = useResource(() => memories.search(debounced), undefined);
  const reload = list.reload;
  useEffect(() => {
    reload();
  }, [debounced, reload]);
  const current = list.data?.find((m) => m.id === selected) ?? null;
  const isNew = selected === "new";
  const [form, setForm] = useState<Form>(blank);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState(false);

  useEffect(() => {
    setForm(current ? toForm(current) : blank);
    setError(null);
  }, [current?.id, current?.updated_at, isNew]); // eslint-disable-line react-hooks/exhaustive-deps

  useAutoSelect("memories", list.data, selected);

  const set = <K extends keyof Form>(k: K, v: Form[K]) => setForm((f) => ({ ...f, [k]: v }));
  const save = async () => {
    try {
      const body = { title: form.title, content: form.content, tags: tagsOf(form.tags), scope: form.scope };
      const saved = current ? await memories.update(current.id, body) : await memories.create(body);
      await list.reload();
      navigate("memories", saved.id);
    } catch (e) {
      setError(errorText(e));
    }
  };
  const remove = async () => {
    setConfirm(false);
    if (!current) return;
    try {
      await memories.remove(current.id);
      await list.reload();
      navigate("memories");
    } catch (e) {
      setError(errorText(e));
    }
  };

  if (list.data?.length === 0 && !query && !debounced && !isNew)
    return (
      <>
        <HeaderActions>
          <AddButton label={t("common.add")} onClick={() => navigate("memories", "new")} />
        </HeaderActions>
        <ErrorBar error={list.error} />
        <EmptyState icon={<MemoryIcon size={20} />} title={t("mem.empty")} action={{ label: t("common.add"), onClick: () => navigate("memories", "new") }}>
          {t("mem.intro")}
        </EmptyState>
      </>
    );

  return (
    <div className="master-detail">
      <HeaderActions>
        <AddButton label={t("common.add")} onClick={() => navigate("memories", "new")} />
      </HeaderActions>
      <aside className="pane left">
        <input className="filter" placeholder={t("mem.search")} value={query} onChange={(e) => setQuery(e.target.value)} />
        <ErrorBar error={list.error} />
        <ul className="md-list">
          {(list.data ?? []).map((m) => (
            <ListItem
              key={m.id}
              selected={m.id === selected}
              onSelect={() => navigate("memories", m.id)}
              icon={<MemoryIcon size={15} />}
              title={m.title}
              sub={m.scope !== "global" ? `${m.scope} · ${m.content}` : m.content}
            />
          ))}
          {list.data?.length === 0 && <li className="md-none">{t("mem.notFound")}</li>}
        </ul>
      </aside>
      <main className="pane detail">
        <ErrorBar error={error} onDismiss={() => setError(null)} />
        {current || isNew ? (
          <div className="form">
            <h2>{isNew ? t("mem.new") : current?.title}</h2>
            {current && <div className="muted">{t("mem.updated", { when: fmt.date(current.updated_at) })}</div>}
            <Field label={t("mem.fieldTitle")}>
              <input value={form.title} onChange={(e) => set("title", e.target.value)} />
            </Field>
            <Field label={t("mem.content")}>
              <textarea rows={8} value={form.content} onChange={(e) => set("content", e.target.value)} />
            </Field>
            <div className="grid2">
              <Field label={t("mem.tags")} hint={t("mem.tagsHint")}>
                <input value={form.tags} onChange={(e) => set("tags", e.target.value)} />
              </Field>
              <Field label={t("mem.scope")} hint={t("mem.scopeHint")}>
                <input value={form.scope} onChange={(e) => set("scope", e.target.value)} />
              </Field>
            </div>
            <div className="row actions">
              <button className="btn primary" onClick={save}>
                {current ? t("mem.save") : t("common.add")}
              </button>
              <span className="spacer" />
              {current && (
                <button className="btn danger-outline" onClick={() => setConfirm(true)}>
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
        <Modal title={t("mem.deleteTitle")} onClose={() => setConfirm(false)}>
          <p>{t("mem.deleteBody", { title: current.title })}</p>
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
