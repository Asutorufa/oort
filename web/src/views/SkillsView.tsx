import { useEffect, useState } from "react";
import { errorText, skills } from "../api";
import type { Skill, SkillInput, SkillsReload } from "../api/types";
import { Markdown } from "../components/chat/LazyMarkdown";
import { Badge, Empty, ErrorBar, Modal } from "../components/common";
import { Field, Toggle } from "../components/forms";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";

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
  body: "## 手順\n\n1. ",
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
  const [tab, setTab] = useState<"preview" | "edit">("preview");
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState<SkillsReload | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [copyFrom, setCopyFrom] = useState<Form | null>(null);
  const current = all.data?.find((k) => k.id === selected) ?? null;
  const isNew = selected === "new";

  useEffect(() => {
    setForm(current ? toForm(current) : (copyFrom ?? blank));
    setCopyFrom(null);
    setTab(isNew ? "edit" : "preview");
    setError(null);
  }, [current?.id, current?.updated_at, isNew]); // eslint-disable-line react-hooks/exhaustive-deps

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

  return (
    <div className="master-detail">
      <aside className="pane left">
        <div className="pane-head">
          <h3>Skills</h3>
          <span className="spacer" />
          <button className="btn small ghost" onClick={rescan} title="SKILL.md ディレクトリを再スキャン">
            ↻ 再読み込み
          </button>
          <button className="btn primary small" onClick={() => navigate("skills", "new")}>
            ＋ 作成
          </button>
        </div>
        <input className="filter" placeholder="検索…" value={query} onChange={(e) => setQuery(e.target.value)} />
        {reload && (
          <div className="reload-note">
            {reload.file_skills} 件の SKILL.md を読み込みました
            {reload.warnings.map((w) => (
              <div key={w} className="warn-text">
                {w}
              </div>
            ))}
          </div>
        )}
        <ErrorBar error={all.error} />
        <ul className="item-list">
          {shown.map((k) => (
            <li key={k.id} className={k.id === selected ? "selected" : ""} onClick={() => navigate("skills", k.id)}>
              <div className="row">
                <b className="mono">/{k.name}</b>
                <span className="spacer" />
                {k.execution_mode === "subagent" && <Badge tone="info">Subagent</Badge>}
                <Badge tone={k.source.kind === "file" ? "muted" : "info"}>{k.source.kind === "file" ? "SKILL.md" : "アプリ"}</Badge>
              </div>
              <div className="muted ellipsis">{k.description}</div>
            </li>
          ))}
          {shown.length === 0 && <li className="muted">Skill がありません</li>}
        </ul>
      </aside>

      <main className="pane detail">
        <ErrorBar error={error} onDismiss={() => setError(null)} />
        {current || isNew ? (
          <div className="form wide">
            <div className="row">
              <h2>{isNew ? "Skill を作成" : `/${current?.name}`}</h2>
              <span className="spacer" />
              <span className="seg">
                <button className={tab === "preview" ? "on" : ""} onClick={() => setTab("preview")}>
                  プレビュー
                </button>
                <button className={tab === "edit" ? "on" : ""} onClick={() => setTab("edit")}>
                  {editable ? "編集" : "内容"}
                </button>
              </span>
            </div>
            {current?.source.kind === "file" && (
              <div className="note">
                ファイル由来の Skill です（<code>{current.source.path}</code>）。編集するにはファイルを書き換えるか、
                <button className="text-btn" onClick={copyToDb}>
                  アプリにコピーして編集
                </button>
                してください。
              </div>
            )}
            {tab === "preview" ? (
              <div className="skill-preview">
                <p className="lead">{form.description}</p>
                {(form.tags || form.allowed_tools) && (
                  <div className="row">
                    {list(form.tags).map((t) => (
                      <span key={t} className="chip">
                        #{t}
                      </span>
                    ))}
                    {list(form.allowed_tools).map((t) => (
                      <span key={t} className="chip mono">
                        ⚙ {t}
                      </span>
                    ))}
                  </div>
                )}
                <Markdown text={form.body || "_（本文なし）_"} />
              </div>
            ) : (
              <>
                <div className="grid2">
                  <Field label="名前" hint="小文字・数字・- のみ。/名前 で呼び出せます">
                    <input className="mono" value={form.name} disabled={!editable} onChange={(e) => set("name", e.target.value)} />
                  </Field>
                  <Field label="タグ" hint="カンマ区切り">
                    <input value={form.tags} disabled={!editable} onChange={(e) => set("tags", e.target.value)} />
                  </Field>
                </div>
                <Field label="説明" hint="会話の最初に固定されるのはこの説明だけです。いつ使うべきかを短く書いてください">
                  <input value={form.description} disabled={!editable} onChange={(e) => set("description", e.target.value)} />
                </Field>
                <div className="grid2">
                  <Field label="呼び出し" hint="/名前 でユーザーが明示的に呼び出せるか">
                    <Toggle checked={form.user_invocable} onChange={(v) => editable && set("user_invocable", v)} label="/ コマンドとして表示する" />
                  </Field>
                  <Field label="引数のヒント" hint="例: <domain>（本文の $ARGUMENTS に入ります）">
                    <input className="mono" value={form.argument_hint} disabled={!editable} placeholder="<target>" onChange={(e) => set("argument_hint", e.target.value)} />
                  </Field>
                </div>
                <div className="grid2">
                  <Field label="実行方法" hint="Subagent は隔離された環境で実行し、要約だけを会話に返します">
                    <span className="seg">
                      <button className={form.execution_mode === "inline" ? "on" : ""} disabled={!editable} onClick={() => set("execution_mode", "inline")}>
                        会話に読み込む
                      </button>
                      <button className={form.execution_mode === "subagent" ? "on" : ""} disabled={!editable} onClick={() => set("execution_mode", "subagent")}>
                        Subagent に任せる
                      </button>
                    </span>
                  </Field>
                  {form.execution_mode === "subagent" && (
                    <Field label="Subagent プロファイル" hint="preset:名前 で Preset のモデルとプロンプトも使えます">
                      <input
                        className="mono"
                        list="subagent-profiles"
                        value={form.subagent_profile}
                        disabled={!editable}
                        placeholder="explorer"
                        onChange={(e) => set("subagent_profile", e.target.value)}
                      />
                      <datalist id="subagent-profiles">
                        <option value="explorer">探索</option>
                        <option value="researcher">リサーチ</option>
                        <option value="reviewer">レビュー</option>
                      </datalist>
                    </Field>
                  )}
                </div>
                <Field label="想定するツール（任意）" hint="カンマ区切り。本文の冒頭に案内として表示されます">
                  <input className="mono" value={form.allowed_tools} disabled={!editable} onChange={(e) => set("allowed_tools", e.target.value)} />
                </Field>
                <Field label="本文（Markdown）">
                  <textarea className="mono" rows={16} value={form.body} disabled={!editable} onChange={(e) => set("body", e.target.value)} />
                </Field>
                {editable && (
                  <div className="row actions">
                    <button className="btn primary" onClick={save}>
                      {isNew ? "作成" : "保存"}
                    </button>
                    <span className="spacer" />
                    {current && (
                      <button className="btn danger-outline" onClick={() => setConfirm(true)}>
                        削除
                      </button>
                    )}
                  </div>
                )}
              </>
            )}
          </div>
        ) : (
          <Empty>
            Skill は再利用できる手順書です。会話では名前と説明だけが共有され、必要なときに本文が読み込まれます（
            <code>~/.oort/skills/&lt;name&gt;/SKILL.md</code> も自動で読み込みます）。
          </Empty>
        )}
      </main>
      {confirm && current && (
        <Modal title="Skill を削除しますか？" onClose={() => setConfirm(false)}>
          <p>
            <b>/{current.name}</b> を削除します。既存の会話の一覧には名前が残りますが、読み込もうとするとエラーになります。
          </p>
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
