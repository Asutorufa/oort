import { AnimatePresence, motion } from "motion/react";
import { useEffect, useId, useState, type ReactNode } from "react";
import { errorText, providers } from "../api";
import {
  DEFAULT_PROVIDER_DEFAULTS,
  REASONING_LEVELS,
  type Provider,
  type ProviderDefaults,
  type ProviderInput,
  type ProviderKind,
  type ProviderTest,
  type ReasoningEffort,
} from "../api/types";
import { AlertIcon, CheckIcon, ChevronDown, KindIcon, PlusIcon, RefreshIcon, Spinner, TrashIcon } from "../brand/Icons";
import { Empty, ErrorBar, fmtMs, Modal } from "../components/common";
import { KeyValueEditor, SecretInput } from "../components/forms";
import { Checkbox, Collapse, Combobox, SegmentedControl, Switch } from "../components/ui";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";
import { enter, spring, tap } from "../lib/motion";

interface Form {
  name: string;
  kind: ProviderKind;
  base_url: string;
  api_key: string;
  default_model: string;
  extra_headers: Record<string, string>;
  is_enabled: boolean;
  auto_retry: { enabled: boolean; max_attempts: number };
  defaults: ProviderDefaults;
}

const blank: Form = {
  name: "",
  kind: "openai",
  base_url: "",
  api_key: "",
  default_model: "",
  extra_headers: {},
  is_enabled: true,
  auto_retry: { enabled: false, max_attempts: 3 },
  defaults: DEFAULT_PROVIDER_DEFAULTS,
};

const toForm = (p: Provider): Form => ({
  name: p.name,
  kind: p.kind,
  base_url: p.base_url ?? "",
  api_key: "",
  default_model: p.default_model,
  extra_headers: p.extra_headers,
  is_enabled: p.is_enabled,
  auto_retry: p.auto_retry ?? { enabled: false, max_attempts: 3 },
  defaults: p.defaults ?? DEFAULT_PROVIDER_DEFAULTS,
});

const toInput = (f: Form): ProviderInput => ({ ...f, base_url: f.base_url.trim(), name: f.name.trim() });

interface KindInfo {
  value: ProviderKind;
  name: string;
  sub: string;
  base: string;
  model: string;
  env: string;
  /** Base URL is required. */
  needsBase?: boolean;
}

const KINDS: KindInfo[] = [
  { value: "openai", name: "OpenAI", sub: "Responses API", base: "https://api.openai.com/v1（既定）", model: "例: gpt-5.1", env: "OPENAI_API_KEY" },
  { value: "anthropic", name: "Anthropic", sub: "Messages API", base: "https://api.anthropic.com（既定）", model: "例: claude-sonnet-4-5", env: "ANTHROPIC_API_KEY" },
  { value: "gemini", name: "Gemini", sub: "Google AI Studio", base: "https://generativelanguage.googleapis.com/v1beta（既定）", model: "例: gemini-2.5-flash", env: "GEMINI_API_KEY" },
  { value: "openai_compatible", name: "OpenAI 互換", sub: "Chat Completions / ローカル LLM", base: "必須（例: http://localhost:11434/v1）", model: "例: llama3.1:8b", env: "LOCAL_API_KEY", needsBase: true },
];
const MOCK: KindInfo = { value: "mock", name: "Mock", sub: "オフライン", base: "使いません", model: "mock", env: "API_KEY" };
const kindInfo = (k: ProviderKind) => KINDS.find((x) => x.value === k) ?? MOCK;

const OUTPUT_PRESETS = [4096, 8192, 16384, 32768, 65536];
const CONTEXT_PRESETS = [32_000, 128_000, 200_000, 1_000_000];
const fmtTokens = (n: number) => (n >= 1_000_000 ? `${n / 1_000_000}M` : n >= 1000 ? `${Math.round(n / 1024) === n / 1024 ? n / 1024 : Math.round(n / 1000)}k` : String(n));

// ----- small building blocks -----------------------------------------------------------------

function Section({ title, description, aside, children }: { title: string; description?: string; aside?: ReactNode; children: ReactNode }) {
  return (
    <section className="pf-section">
      <header className="pf-section-head">
        <div>
          <h3>{title}</h3>
          {description && <p>{description}</p>}
        </div>
        {aside}
      </header>
      <div className="pf-section-body">{children}</div>
    </section>
  );
}

function Label({ children, hint, htmlFor }: { children: ReactNode; hint?: ReactNode; htmlFor?: string }) {
  return (
    <div className="pf-label">
      <label htmlFor={htmlFor}>{children}</label>
      {hint}
    </div>
  );
}

function NumberField({
  value,
  onChange,
  placeholder,
  presets,
  suffix,
}: {
  value: number | null;
  onChange: (v: number | null) => void;
  placeholder: string;
  presets: number[];
  suffix?: string;
}) {
  return (
    <div className="pf-number">
      <div className="input-affix">
        <input
          inputMode="numeric"
          className="tabular"
          value={value ?? ""}
          placeholder={placeholder}
          onChange={(e) => {
            const raw = e.target.value.replace(/[^\d]/g, "");
            onChange(raw ? Number(raw) : null);
          }}
        />
        {suffix && <span className="affix-text">{suffix}</span>}
      </div>
      <div className="preset-chips">
        {presets.map((p) => (
          <button key={p} type="button" className={`mini-chip ${value === p ? "on" : ""}`} onClick={() => onChange(value === p ? null : p)}>
            {fmtTokens(p)}
          </button>
        ))}
      </div>
    </div>
  );
}

function ResultBanner({ result }: { result: ProviderTest | "running" | null }) {
  return (
    <AnimatePresence mode="wait">
      {result && (
        <motion.div
          key={result === "running" ? "running" : `${result.ok}-${result.latency_ms}`}
          className={`pf-banner ${result === "running" ? "running" : result.ok ? "ok" : "fail"}`}
          initial={{ opacity: 0, y: 6, height: 0 }}
          animate={{ opacity: 1, y: 0, height: "auto" }}
          exit={{ opacity: 0, y: 4, height: 0 }}
          transition={{ duration: 0.18, ease: [0.2, 0, 0, 1] }}
        >
          <div className="pf-banner-inner">
            {result === "running" ? (
              <>
                <Spinner />
                <span>接続を確認しています…</span>
              </>
            ) : result.ok ? (
              <>
                <span className="pf-banner-icon">
                  <CheckIcon size={15} stroke={2} />
                </span>
                <span className="pf-banner-main">
                  <b>接続できました</b>
                  <span className="tabular">{fmtMs(result.latency_ms)}</span>
                  {result.model_checked && (
                    <span>
                      · <span className="mono">{result.model_checked}</span>
                      {result.model_found === false ? "（一覧にありません）" : result.model_found ? "（利用可能）" : ""}
                    </span>
                  )}
                  {result.models.length > 0 && <span>· {result.models.length} モデル</span>}
                </span>
              </>
            ) : (
              <>
                <span className="pf-banner-icon">
                  <AlertIcon size={15} />
                </span>
                <span className="pf-banner-main">
                  <b>接続できませんでした</b>
                  {result.latency_ms > 0 && <span className="tabular">{fmtMs(result.latency_ms)}</span>}
                  {result.error && <span className="pf-banner-error">{result.error}</span>}
                </span>
              </>
            )}
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

/** Compact status line in the provider list. */
export function TestResult({ result }: { result: ProviderTest | null | "running" }) {
  if (!result) return null;
  if (result === "running") return <span className="muted">確認中…</span>;
  return (
    <span className={`test-result ${result.ok ? "ok" : "fail"}`}>
      {result.ok ? <CheckIcon size={12} stroke={2.2} /> : <AlertIcon size={12} />}
      {result.ok ? `接続 OK · ${fmtMs(result.latency_ms)}` : result.error ?? "失敗"}
    </span>
  );
}

// ----- view ----------------------------------------------------------------------------------

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
  const [models, setModels] = useState<string[]>([]);
  const [modelsState, setModelsState] = useState<"idle" | "loading" | { error: string }>("idle");
  const [advanced, setAdvanced] = useState(false);
  const [saving, setSaving] = useState(false);
  const ids = { name: useId(), base: useId(), model: useId(), temp: useId(), timeout: useId() };

  useEffect(() => {
    setForm(current ? toForm(current) : blank);
    setFormTest(null);
    setError(null);
    setModels([]);
    setModelsState("idle");
  }, [current?.id, current?.updated_at, isNew]); // eslint-disable-line react-hooks/exhaustive-deps

  const set = <K extends keyof Form>(k: K, v: Form[K]) => setForm((f) => ({ ...f, [k]: v }));
  const setD = <K extends keyof ProviderDefaults>(k: K, v: ProviderDefaults[K]) => setForm((f) => ({ ...f, defaults: { ...f.defaults, [k]: v } }));
  const info = kindInfo(form.kind);

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
      const r = await providers.test(current?.id ?? null, { ...toInput(form), ping });
      setFormTest(r);
      if (r.models.length) setModels(r.models.map((m) => m.replace(/^models\//, "")));
    } catch (e) {
      setFormTest({ ok: false, latency_ms: 0, model_checked: null, model_found: null, models: [], error: errorText(e) });
    }
  };

  const fetchModels = async () => {
    setModelsState("loading");
    try {
      const r = await providers.models(current?.id ?? null, toInput(form));
      setModels(r.models);
      setModelsState("idle");
    } catch (e) {
      setModelsState({ error: errorText(e).replace(/^list models:\s*/, "") });
    }
  };

  const save = async () => {
    setSaving(true);
    try {
      const saved = current ? await providers.update(current.id, toInput(form)) : await providers.create(toInput(form));
      await list.reload();
      navigate("models", saved.id);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSaving(false);
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

  const temp = form.defaults.temperature;
  const level = REASONING_LEVELS.find((l) => l.value === form.defaults.reasoning_effort) ?? REASONING_LEVELS[0];
  const kinds = current?.kind === "mock" ? [...KINDS, MOCK] : KINDS;

  return (
    <div className="master-detail">
      <aside className="pane left">
        <div className="pane-head">
          <h3>モデル設定</h3>
          <span className="spacer" />
          <motion.button whileTap={tap} className="btn small primary" onClick={() => navigate("models", "new")}>
            <PlusIcon size={13} /> 追加
          </motion.button>
        </div>
        <ErrorBar error={list.error} />
        <ul className="item-list provider-list">
          {(list.data ?? []).map((p) => (
            <li key={p.id} className={p.id === selected ? "selected" : ""} onClick={() => navigate("models", p.id)}>
              {p.id === selected && <motion.span layoutId="provider-list-active" className="list-active" transition={spring} />}
              <div className="provider-row">
                <span className="provider-kind">
                  <KindIcon kind={p.kind} size={16} />
                </span>
                <span className="provider-text">
                  <span className="provider-name">
                    {p.name}
                    {!p.is_enabled && <span className="pill tiny">無効</span>}
                  </span>
                  <span className="provider-sub mono">{p.default_model || "既定モデルなし"}</span>
                </span>
                <button
                  className="btn ghost small"
                  title="接続テスト"
                  onClick={(e) => {
                    e.stopPropagation();
                    runTest(p.id);
                  }}
                >
                  テスト
                </button>
              </div>
              {tests[p.id] && (
                <div className="provider-test" onClick={(e) => e.stopPropagation()}>
                  <TestResult result={tests[p.id]} />
                </div>
              )}
            </li>
          ))}
        </ul>
      </aside>

      <main className="pane detail pf-pane">
        {current || isNew ? (
          <motion.div className="pf" key={current?.id ?? "new"} variants={enter} initial="hidden" animate="shown">
            <header className="pf-head">
              <div>
                <h2>{current ? current.name : "プロバイダを追加"}</h2>
                <p>{current ? `${kindInfo(current.kind).name} · ${kindInfo(current.kind).sub}` : "AI モデルの接続先と、会話で使う既定の設定を登録します"}</p>
              </div>
              <div className="pf-head-aside">
                <span className="pf-status">{form.is_enabled ? "有効" : "無効"}</span>
                <Switch checked={form.is_enabled} onChange={(v) => set("is_enabled", v)} />
              </div>
            </header>
            <ErrorBar error={error} onDismiss={() => setError(null)} />

            <div className="kind-tiles" role="radiogroup" aria-label="プロバイダの種類">
              {kinds.map((k) => {
                const on = form.kind === k.value;
                return (
                  <button
                    key={k.value}
                    type="button"
                    role="radio"
                    aria-checked={on}
                    disabled={!!current && !on}
                    className={`kind-tile ${on ? "on" : ""}`}
                    onClick={() => !current && set("kind", k.value)}
                    title={current && !on ? "作成後は種類を変更できません" : undefined}
                  >
                    {on && <motion.span layoutId="kind-tile-active" className="kind-tile-ring" transition={spring} />}
                    <span className="kind-tile-icon">
                      <KindIcon kind={k.value} />
                    </span>
                    <span className="kind-tile-name">{k.name}</span>
                    <span className="kind-tile-sub">{k.sub}</span>
                  </button>
                );
              })}
            </div>
            {current && <p className="pf-note">種類は作成後に変更できません（会話が通信形式に依存するため）。</p>}

            <Section title="接続" description="接続先と認証情報">
              <div className="pf-grid">
                <div className="pf-field">
                  <Label htmlFor={ids.name}>名前</Label>
                  <input id={ids.name} value={form.name} placeholder={`例: ${info.name}`} onChange={(e) => set("name", e.target.value)} />
                </div>
                <div className="pf-field">
                  <Label htmlFor={ids.base}>ベース URL</Label>
                  <input id={ids.base} value={form.base_url} placeholder={info.base} disabled={form.kind === "mock"} onChange={(e) => set("base_url", e.target.value)} />
                  <span className="field-hint">{info.needsBase ? "必須です" : "空欄なら既定の URL を使います"}</span>
                </div>
              </div>
              {form.kind !== "mock" && (
                <div className="pf-field">
                  <Label>API キー</Label>
                  <SecretInput current={current?.api_key} source={current?.api_key_source} value={form.api_key} onChange={(v) => set("api_key", v)} envName={info.env} />
                </div>
              )}
            </Section>

            <Section title="モデル" description="新しい会話で最初に選ばれるモデル">
              <div className="pf-field">
                <Label
                  htmlFor={ids.model}
                  hint={
                    <button type="button" className="btn ghost small" onClick={fetchModels} disabled={modelsState === "loading"}>
                      {modelsState === "loading" ? <Spinner size={13} /> : <RefreshIcon size={13} />}
                      モデル一覧を取得
                    </button>
                  }
                >
                  既定のモデル
                </Label>
                <Combobox value={form.default_model} onChange={(v) => set("default_model", v)} options={models} placeholder={info.model} />
                <AnimatePresence mode="wait" initial={false}>
                  <motion.span
                    key={typeof modelsState === "object" ? "err" : models.length ? "n" : "none"}
                    className={`field-hint ${typeof modelsState === "object" ? "error-text" : ""}`}
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    exit={{ opacity: 0 }}
                    transition={{ duration: 0.12 }}
                  >
                    {typeof modelsState === "object"
                      ? `一覧を取得できませんでした（${modelsState.error}）。モデル名は直接入力できます。`
                      : models.length
                        ? `${models.length} 個のモデルが見つかりました。入力して絞り込めます。`
                        : "一覧を取得するか、モデル名を直接入力してください。"}
                  </motion.span>
                </AnimatePresence>
              </div>
            </Section>

            <Section title="思考と生成" description="このプロバイダで作る会話の既定値（会話ごとに変更できます）">
              <div className="pf-field">
                <Label>思考レベル</Label>
                <SegmentedControl<ReasoningEffort>
                  full
                  ariaLabel="思考レベル"
                  value={form.defaults.reasoning_effort}
                  onChange={(v) => setD("reasoning_effort", v)}
                  options={REASONING_LEVELS.map((l) => ({ value: l.value, label: l.label, title: l.description }))}
                />
                <AnimatePresence mode="wait" initial={false}>
                  <motion.span key={level.value} className="field-hint" initial={{ opacity: 0, y: -2 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} transition={{ duration: 0.12 }}>
                    {level.description}
                    {level.value !== "default" && "。対応していないモデルでは無視されることがあります。"}
                  </motion.span>
                </AnimatePresence>
              </div>

              <div className="pf-field">
                <Label
                  htmlFor={ids.temp}
                  hint={
                    <span className="pf-temp-value">
                      <span className="tabular">{temp == null ? "既定" : temp.toFixed(2)}</span>
                      {temp != null && (
                        <button type="button" className="text-btn" onClick={() => setD("temperature", null)}>
                          リセット
                        </button>
                      )}
                    </span>
                  }
                >
                  Temperature
                </Label>
                <input
                  id={ids.temp}
                  type="range"
                  min={0}
                  max={2}
                  step={0.05}
                  className={temp == null ? "unset" : ""}
                  style={{ ["--fill" as string]: `${((temp ?? 0) / 2) * 100}%` }}
                  value={temp ?? 1}
                  onChange={(e) => setD("temperature", Number(e.target.value))}
                />
                <span className="field-hint">低いほど安定、高いほど多様な応答になります。思考を使う推論モデルでは送信しません。</span>
              </div>

              <div className="pf-grid">
                <div className="pf-field">
                  <Label>最大出力トークン</Label>
                  <NumberField value={form.defaults.max_output_tokens} onChange={(v) => setD("max_output_tokens", v)} placeholder="既定（4,096）" presets={OUTPUT_PRESETS} />
                </div>
                <div className="pf-field">
                  <Label>コンテキスト長</Label>
                  <NumberField value={form.defaults.context_window} onChange={(v) => setD("context_window", v)} placeholder="既定（200,000）" presets={CONTEXT_PRESETS} />
                  <span className="field-hint">会話の自動要約はこの長さを基準に始まります</span>
                </div>
              </div>
            </Section>

            <section className="pf-section">
              <button type="button" className="pf-disclosure" aria-expanded={advanced} onClick={() => setAdvanced(!advanced)}>
                <div>
                  <h3>詳細設定</h3>
                  <p>タイムアウト・自動再試行・追加ヘッダー</p>
                </div>
                <motion.span animate={{ rotate: advanced ? 180 : 0 }} transition={spring} className="pf-disclosure-chev">
                  <ChevronDown size={16} />
                </motion.span>
              </button>
              <Collapse open={advanced}>
                <div className="pf-section-body pf-advanced">
                  <div className="pf-grid">
                    <div className="pf-field">
                      <Label htmlFor={ids.timeout}>タイムアウト</Label>
                      <div className="input-affix">
                        <input
                          id={ids.timeout}
                          inputMode="numeric"
                          className="tabular"
                          value={form.defaults.timeout_secs}
                          onChange={(e) => setD("timeout_secs", Number(e.target.value.replace(/[^\d]/g, "")) || 0)}
                        />
                        <span className="affix-text">秒</span>
                      </div>
                      <span className="field-hint">応答が途切れてから諦めるまでの時間（5〜1800 秒）</span>
                    </div>
                    <div className="pf-field">
                      <Label>自動再試行</Label>
                      <div className="pf-inline">
                        <Switch checked={form.auto_retry.enabled} onChange={(v) => set("auto_retry", { ...form.auto_retry, enabled: v })} />
                        <span className="muted">最大</span>
                        <div className="input-affix narrow">
                          <input
                            inputMode="numeric"
                            className="tabular"
                            disabled={!form.auto_retry.enabled}
                            value={form.auto_retry.max_attempts}
                            onChange={(e) => set("auto_retry", { ...form.auto_retry, max_attempts: Math.min(10, Math.max(1, Number(e.target.value.replace(/[^\d]/g, "")) || 1)) })}
                          />
                          <span className="affix-text">回</span>
                        </div>
                      </div>
                      <span className="field-hint">429・5xx・接続エラーで止まったとき、待ってから再試行します</span>
                    </div>
                  </div>
                  <KeyValueEditor
                    title="追加ヘッダー"
                    hint="認証系の値はマスク表示され、変更しなければそのまま保持されます"
                    addLabel="ヘッダーを追加"
                    emptyText="追加ヘッダーはありません"
                    keyPlaceholder="ヘッダー名"
                    valuePlaceholder="値 または env:VAR"
                    value={form.extra_headers}
                    onChange={(v) => set("extra_headers", v)}
                  />
                </div>
              </Collapse>
            </section>

            {current && (
              <div className="pf-danger">
                <div>
                  <b>プロバイダを削除</b>
                  <p>このプロバイダで作った会話は読めますが、続きは実行できなくなります。</p>
                </div>
                <button className="btn danger-outline" onClick={() => setConfirm(true)}>
                  <TrashIcon size={14} /> 削除
                </button>
              </div>
            )}

            <div className="pf-footer">
              <ResultBanner result={formTest} />
              <div className="pf-actions">
                <motion.button whileTap={tap} className="btn" onClick={testForm} disabled={formTest === "running"} title="未保存の変更も含めて接続を確認します">
                  {formTest === "running" ? <Spinner size={13} /> : null}
                  接続テスト
                </motion.button>
                <Checkbox checked={ping} onChange={setPing} label="1 トークン送って応答も確認" />
                <span className="spacer" />
                <button className="btn ghost" onClick={() => (current ? setForm(toForm(current)) : navigate("models"))}>
                  {current ? "変更を戻す" : "キャンセル"}
                </button>
                <motion.button whileTap={tap} className="btn primary" onClick={save} disabled={saving}>
                  {saving && <Spinner size={13} />}
                  {current ? "変更を保存" : "作成"}
                </motion.button>
              </div>
            </div>
          </motion.div>
        ) : (
          <Empty>プロバイダを選ぶか、追加してください。API キーは env:NAME と書くとサーバーの環境変数から読み込みます。</Empty>
        )}
      </main>
      {confirm && current && (
        <Modal title="プロバイダを削除しますか？" onClose={() => setConfirm(false)}>
          <p>
            <b>{current.name}</b> を削除します。このプロバイダで作った会話は読めますが、続きは実行できなくなります。
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
