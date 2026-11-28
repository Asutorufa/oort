import { useEffect, useState } from "react";
import { errorText, providers } from "../api";
import { PROVIDER_KINDS, type Provider, type ProviderInput, type ProviderKind, type ProviderTest } from "../api/types";
import { Badge, Empty, ErrorBar, fmtMs, Modal } from "../components/common";
import { Field, KeyValueEditor, SecretInput, Toggle } from "../components/forms";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";

interface Form {
  name: string;
  kind: ProviderKind;
  base_url: string;
  api_key: string;
  default_model: string;
  extra_headers: Record<string, string>;
  is_enabled: boolean;
}

const blank: Form = { name: "", kind: "openai", base_url: "", api_key: "", default_model: "", extra_headers: {}, is_enabled: true };

const toForm = (p: Provider): Form => ({
  name: p.name,
  kind: p.kind,
  base_url: p.base_url ?? "",
  api_key: "",
  default_model: p.default_model,
  extra_headers: p.extra_headers,
  is_enabled: p.is_enabled,
});

const toInput = (f: Form): ProviderInput => ({ ...f, base_url: f.base_url.trim() });

const BASE_HINT: Record<ProviderKind, string> = {
  openai: "default https://api.openai.com/v1",
  anthropic: "default https://api.anthropic.com",
  gemini: "default https://generativelanguage.googleapis.com/v1beta",
  openai_compatible: "required, e.g. http://localhost:11434/v1",
  mock: "not used",
};

export function TestResult({ result }: { result: ProviderTest | null | "running" }) {
  if (!result) return null;
  if (result === "running") return <span className="muted">確認中…</span>;
  return (
    <span className="test-result">
      <Badge tone={result.ok ? "ok" : "error"}>{result.ok ? "接続 OK" : "失敗"}</Badge>
      <span className="muted">{fmtMs(result.latency_ms)}</span>
      {result.ok && result.model_checked && (
        <span className="muted">
          {result.model_checked}{" "}
          {result.model_found === false ? <Badge tone="warn">一覧にないモデル</Badge> : result.model_found ? "✓ 利用可能" : ""}
        </span>
      )}
      {result.ok && <span className="muted">{result.models.length} モデル</span>}
      {result.error && <span className="error-text">{result.error}</span>}
    </span>
  );
}

export function ProvidersView({ selected }: { selected: string | null }) {
  const list = useResource(providers.list);
  const current = list.data?.find((p) => p.id === selected) ?? null;
  const isNew = selected === "new";
  const [form, setForm] = useState<Form>(blank);
  const [error, setError] = useState<string | null>(null);
  const [tests, setTests] = useState<Record<string, ProviderTest | "running">>({});
  const [formTest, setFormTest] = useState<ProviderTest | "running" | null>(null);
  const [ping, setPing] = useState(false);
  const [confirm, setConfirm] = useState(false);

  useEffect(() => {
    setForm(current ? toForm(current) : blank);
    setFormTest(null);
    setError(null);
  }, [current?.id, current?.updated_at, isNew]); // eslint-disable-line react-hooks/exhaustive-deps

  const set = <K extends keyof Form>(k: K, v: Form[K]) => setForm((f) => ({ ...f, [k]: v }));

  const runTest = async (id: string) => {
    setTests((t) => ({ ...t, [id]: "running" }));
    try {
      const r = await providers.test(id);
      setTests((t) => ({ ...t, [id]: r }));
    } catch (e) {
      setTests((t) => ({ ...t, [id]: { ok: false, latency_ms: 0, model_checked: null, model_found: null, models: [], error: errorText(e) } }));
    }
  };

  const testForm = async () => {
    setFormTest("running");
    try {
      setFormTest(await providers.test(current?.id ?? null, { ...toInput(form), ping }));
    } catch (e) {
      setFormTest({ ok: false, latency_ms: 0, model_checked: null, model_found: null, models: [], error: errorText(e) });
    }
  };

  const save = async () => {
    try {
      const saved = current ? await providers.update(current.id, toInput(form)) : await providers.create(toInput(form));
      await list.reload();
      navigate("models", saved.id);
    } catch (e) {
      setError(errorText(e));
    }
  };

  const remove = async () => {
    setConfirm(false);
    if (!current) return;
    try {
      await providers.remove(current.id);
      await list.reload();
      navigate("models");
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <div className="master-detail">
      <aside className="pane left">
        <div className="pane-head">
          <h3>モデル設定</h3>
          <button className="primary small" onClick={() => navigate("models", "new")}>
            ＋ 追加
          </button>
        </div>
        <ErrorBar error={list.error} />
        <ul className="item-list">
          {(list.data ?? []).map((p) => (
            <li key={p.id} className={p.id === selected ? "selected" : ""} onClick={() => navigate("models", p.id)}>
              <div className="row">
                <b>{p.name}</b>
                <span className="spacer" />
                {!p.is_enabled && <Badge>disabled</Badge>}
                <button
                  className="small"
                  onClick={(e) => {
                    e.stopPropagation();
                    runTest(p.id);
                  }}
                >
                  テスト
                </button>
              </div>
              <div className="row muted">
                <span className="mono">{p.kind}</span>
                <span>· {p.default_model || "no default model"}</span>
                <span>· key: {p.api_key_source}</span>
              </div>
              {tests[p.id] && (
                <div onClick={(e) => e.stopPropagation()}>
                  <TestResult result={tests[p.id]} />
                </div>
              )}
            </li>
          ))}
        </ul>
      </aside>

      <main className="pane detail">
        {current || isNew ? (
          <div className="form">
            <h2>{current ? current.name : "プロバイダを追加"}</h2>
            <ErrorBar error={error} onDismiss={() => setError(null)} />
            <div className="grid2">
              <Field label="名前">
                <input value={form.name} onChange={(e) => set("name", e.target.value)} />
              </Field>
              <Field label="種類" hint={current ? "作成後は変更できません（会話が通信形式に依存するため）" : undefined}>
                <select value={form.kind} disabled={!!current} onChange={(e) => set("kind", e.target.value as ProviderKind)}>
                  {PROVIDER_KINDS.map((k) => (
                    <option key={k.value} value={k.value}>
                      {k.label}
                    </option>
                  ))}
                </select>
              </Field>
            </div>
            <Field label="ベース URL" hint={BASE_HINT[form.kind]}>
              <input value={form.base_url} placeholder="(default)" onChange={(e) => set("base_url", e.target.value)} />
            </Field>
            {form.kind !== "mock" && (
              <Field
                label="API キー"
                hint={
                  <>
                    そのまま保存するか、<code>env:GEMINI_API_KEY</code> と書くと使用時にサーバーの環境変数から読み込みます（保存済みのキーは表示されません）。
                  </>
                }
              >
                <SecretInput current={current?.api_key} source={current?.api_key_source} value={form.api_key} onChange={(v) => set("api_key", v)} />
              </Field>
            )}
            <Field label="既定のモデル">
              <input value={form.default_model} placeholder="e.g. gemini-2.5-flash" onChange={(e) => set("default_model", e.target.value)} />
            </Field>
            <Field label="追加ヘッダー" hint="認証系の値はマスク表示され、変更しなければそのまま保持されます。">
              <KeyValueEditor value={form.extra_headers} onChange={(v) => set("extra_headers", v)} />
            </Field>
            <Toggle checked={form.is_enabled} onChange={(v) => set("is_enabled", v)} label="有効" />
            <div className="row actions">
              <button className="primary" onClick={save}>
                {current ? "保存" : "作成"}
              </button>
              <button onClick={testForm} title="未保存の変更も含めて接続を確認します">
                接続テスト
              </button>
              <Toggle checked={ping} onChange={setPing} label="1 トークンの ping も送る" />
              <span className="spacer" />
              {current && (
                <button className="danger-outline" onClick={() => setConfirm(true)}>
                  削除
                </button>
              )}
            </div>
            <TestResult result={formTest} />
            {formTest && formTest !== "running" && formTest.models.length > 0 && (
              <details className="card">
                <summary>
                  <span className="card-kind">Models</span> <span className="muted">{formTest.models.length}</span>
                </summary>
                <div className="model-list">
                  {formTest.models.map((m) => (
                    <button key={m} className="chip" onClick={() => set("default_model", m.replace(/^models\//, ""))} title="use as default">
                      {m}
                    </button>
                  ))}
                </div>
              </details>
            )}
          </div>
        ) : (
          <Empty>プロバイダを選ぶか、追加してください。キーは env:NAME で環境変数に置いたままにできます。</Empty>
        )}
      </main>
      {confirm && current && (
        <Modal title="プロバイダを削除しますか？" onClose={() => setConfirm(false)}>
          <p>
            <b>{current.name}</b> を削除します。このプロバイダで作った会話は読めますが、続きは実行できなくなります。
          </p>
          <div className="row">
            <span className="spacer" />
            <button onClick={() => setConfirm(false)}>やめる</button>
            <button className="danger" onClick={remove}>
              削除する
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
