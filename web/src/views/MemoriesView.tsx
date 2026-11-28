import { useEffect, useState } from "react";
import { errorText, memories } from "../api";
import type { Memory } from "../api/types";
import { Empty, ErrorBar, fmtTime, Modal } from "../components/common";
import { Field } from "../components/forms";
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

  return (
    <div className="master-detail">
      <aside className="pane left">
        <div className="pane-head">
          <h3>メモリ</h3>
          <span className="spacer" />
          <button className="btn primary small" onClick={() => navigate("memories", "new")}>
            ＋ 追加
          </button>
        </div>
        <input className="filter" placeholder="全文検索（日本語・中国語・英語）" value={query} onChange={(e) => setQuery(e.target.value)} />
        <ErrorBar error={list.error} />
        <ul className="item-list">
          {(list.data ?? []).map((m) => (
            <li key={m.id} className={m.id === selected ? "selected" : ""} onClick={() => navigate("memories", m.id)}>
              <div className="row">
                <b className="ellipsis">{m.title}</b>
                <span className="spacer" />
                {m.scope !== "global" && <span className="chip">{m.scope}</span>}
              </div>
              <div className="muted ellipsis">{m.content}</div>
              {m.tags.length > 0 && (
                <div>
                  {m.tags.map((t) => (
                    <span key={t} className="chip">
                      #{t}
                    </span>
                  ))}
                </div>
              )}
            </li>
          ))}
          {list.data?.length === 0 && <li className="muted">{debounced ? "見つかりません" : "まだ何も覚えていません"}</li>}
        </ul>
      </aside>
      <main className="pane detail">
        <ErrorBar error={error} onDismiss={() => setError(null)} />
        {current || isNew ? (
          <div className="form">
            <h2>{isNew ? "メモリを追加" : current?.title}</h2>
            {current && <div className="muted">更新 {fmtTime(current.updated_at)}</div>}
            <Field label="タイトル">
              <input value={form.title} onChange={(e) => set("title", e.target.value)} />
            </Field>
            <Field label="内容">
              <textarea rows={8} value={form.content} onChange={(e) => set("content", e.target.value)} />
            </Field>
            <div className="grid2">
              <Field label="タグ" hint="カンマ区切り">
                <input value={form.tags} onChange={(e) => set("tags", e.target.value)} />
              </Field>
              <Field label="スコープ" hint="global またはプロジェクト名">
                <input value={form.scope} onChange={(e) => set("scope", e.target.value)} />
              </Field>
            </div>
            <div className="row actions">
              <button className="btn primary" onClick={save}>
                {current ? "保存" : "追加"}
              </button>
              <span className="spacer" />
              {current && (
                <button className="btn danger-outline" onClick={() => setConfirm(true)}>
                  削除
                </button>
              )}
            </div>
          </div>
        ) : (
          <Empty>会話をまたいで覚えておくこと（好み・環境の決まりごと・プロジェクトの前提）です。エージェントは必要なときにだけ検索します。</Empty>
        )}
      </main>
      {confirm && current && (
        <Modal title="メモリを削除しますか？" onClose={() => setConfirm(false)}>
          <p>「{current.title}」を削除します。</p>
          <div className="row">
            <span className="spacer" />
            <button className="btn" onClick={() => setConfirm(false)}>
              やめる
            </button>
            <button className="btn danger" onClick={remove}>
              削除する
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
