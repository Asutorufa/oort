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
  type ServerToolConfig,
  type Vendor,
} from "../api/types";
import { getProtocolOption, getVendorPreset, VENDOR_PRESETS } from "../api/vendorCatalog";
import { AlertIcon, CheckIcon, ChevronDown, KindIcon, PlusIcon, RefreshIcon, Spinner, TrashIcon } from "../brand/Icons";
import { Empty, ErrorBar, Modal } from "../components/common";
import { useI18n } from "../i18n";
import { KeyValueEditor, SecretInput } from "../components/forms";
import { Checkbox, Collapse, Combobox, SegmentedControl, Switch } from "../components/ui";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";
import { enter, spring, tap } from "../lib/motion";

interface Form {
  name: string;
  vendor: Vendor;
  kind: ProviderKind;
  base_url: string;
  api_key: string;
  default_model: string;
  extra_headers: Record<string, string>;
  default_server_tools: ServerToolConfig;
  extra_body_text: string;
  is_enabled: boolean;
  auto_retry: { enabled: boolean; max_attempts: number };
  defaults: ProviderDefaults;
}

const KNOWN_DEFAULT_URLS = new Set(
  VENDOR_PRESETS.flatMap((v) => v.protocols.map((p) => p.defaultBaseUrl)).filter(Boolean),
);

const blank: Form = {
  name: "",
  vendor: "openai",
  kind: "openai",
  base_url: "",
  api_key: "",
  default_model: "",
  extra_headers: {},
  default_server_tools: {},
  extra_body_text: "",
  is_enabled: true,
  auto_retry: { enabled: false, max_attempts: 3 },
  defaults: DEFAULT_PROVIDER_DEFAULTS,
};

const toForm = (p: Provider): Form => ({
  name: p.name,
  vendor: p.vendor ?? "custom",
  kind: p.kind,
  base_url: p.base_url ?? "",
  api_key: "",
  default_model: p.default_model,
  extra_headers: p.extra_headers,
  default_server_tools: p.default_server_tools ?? {},
  extra_body_text: p.extra_body ? JSON.stringify(p.extra_body, null, 2) : "",
  is_enabled: p.is_enabled,
  auto_retry: p.auto_retry ?? { enabled: false, max_attempts: 3 },
  defaults: p.defaults ?? DEFAULT_PROVIDER_DEFAULTS,
});

function parseExtraBody(raw: string): { value: Record<string, unknown> | null; error: boolean } {
  const trimmed = raw.trim();
  if (!trimmed) return { value: null, error: false };
  try {
    const parsed = JSON.parse(trimmed);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      return { value: parsed as Record<string, unknown>, error: false };
    }
    return { value: null, error: true };
  } catch {
    return { value: null, error: true };
  }
}

const toInput = (f: Form): ProviderInput => {
  const parsedExtra = parseExtraBody(f.extra_body_text);
  return {
    name: f.name.trim(),
    vendor: f.vendor,
    kind: f.kind,
    base_url: f.base_url.trim(),
    api_key: f.api_key,
    default_model: f.default_model,
    extra_headers: f.extra_headers,
    default_server_tools: f.default_server_tools,
    extra_body: parsedExtra.value,
    is_enabled: f.is_enabled,
    auto_retry: f.auto_retry,
    defaults: f.defaults,
  };
};

const OUTPUT_PRESETS = [4096, 8192, 16384, 32768, 65536];
const CONTEXT_PRESETS = [32_000, 128_000, 200_000, 1_000_000];
const fmtTokens = (n: number) =>
  n >= 1_000_000 && n % 1_000_000 === 0 ? `${n / 1_000_000}M` : n % 1024 === 0 ? `${n / 1024}k` : n % 1000 === 0 ? `${n / 1000}k` : String(n);

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
  const { t, fmt } = useI18n();
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
                <span>{t("prov.checking")}</span>
              </>
            ) : result.ok ? (
              <>
                <span className="pf-banner-icon">
                  <CheckIcon size={15} stroke={2} />
                </span>
                <span className="pf-banner-main">
                  <b>{t("prov.connected")}</b>
                  <span className="tabular">{fmt.ms(result.latency_ms)}</span>
                  {result.model_checked && (
                    <span>
                      · <span className="mono">{result.model_checked}</span>
                      {result.model_found === false ? t("prov.notListed") : result.model_found ? t("prov.available") : ""}
                    </span>
                  )}
                  {result.models.length > 0 && <span>· {t("prov.modelCount", { count: result.models.length })}</span>}
                </span>
              </>
            ) : (
              <>
                <span className="pf-banner-icon">
                  <AlertIcon size={15} />
                </span>
                <span className="pf-banner-main">
                  <b>{t("prov.failed")}</b>
                  {result.latency_ms > 0 && <span className="tabular">{fmt.ms(result.latency_ms)}</span>}
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
  const { t, fmt } = useI18n();
  if (!result) return null;
  if (result === "running") return <span className="muted">{t("prov.listChecking")}</span>;
  return (
    <span className={`test-result ${result.ok ? "ok" : "fail"}`}>
      {result.ok ? <CheckIcon size={12} stroke={2.2} /> : <AlertIcon size={12} />}
      {result.ok ? t("prov.listOk", { ms: fmt.ms(result.latency_ms) }) : result.error ?? t("prov.listFailed")}
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
  const { t, fmt } = useI18n();
  const preset = getVendorPreset(form.vendor);
  const proto = getProtocolOption(form.vendor, form.kind) ?? preset.protocols[0];
  const isMock = form.kind === "mock";
  const extraBodyStatus = parseExtraBody(form.extra_body_text);

  const selectVendor = (v: Vendor) => {
    if (current) return;
    const nextPreset = getVendorPreset(v);
    const nextProto = nextPreset.protocols[0];
    setForm((f) => {
      const autoUrl = !f.base_url.trim() || KNOWN_DEFAULT_URLS.has(f.base_url.trim());
      return {
        ...f,
        vendor: v,
        kind: nextProto.kind,
        base_url: autoUrl ? (nextPreset.id === "openai" || nextPreset.id === "anthropic" || nextPreset.id === "gemini" ? "" : nextProto.defaultBaseUrl) : f.base_url,
        default_server_tools: {},
      };
    });
  };

  const selectProtocol = (kind: ProviderKind) => {
    if (current) return;
    const nextProto = getProtocolOption(form.vendor, kind);
    if (!nextProto) return;
    setForm((f) => {
      const autoUrl = !f.base_url.trim() || KNOWN_DEFAULT_URLS.has(f.base_url.trim());
      const nextUrl =
        nextProto.kind === "openai" && f.vendor === "openai"
          ? ""
          : nextProto.defaultBaseUrl;
      return {
        ...f,
        kind,
        base_url: autoUrl ? nextUrl : f.base_url,
      };
    });
  };

  const setServerTool = (updater: (cur: ServerToolConfig) => ServerToolConfig) => {
    setForm((f) => ({ ...f, default_server_tools: updater(f.default_server_tools ?? {}) }));
  };

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
    if (extraBodyStatus.error) {
      setError(t("prov.extraBodyInvalid"));
      return;
    }
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
  const basePlaceholder = isMock
    ? t("prov.baseUnused")
    : preset.baseRequired
      ? t("prov.baseRequired")
      : t("prov.baseDefault", { url: proto.defaultBaseUrl });
  const supportedTools = proto.serverTools;
  const hasAnyServerTools = Object.keys(supportedTools).length > 0;

  return (
    <div className="master-detail">
      <aside className="pane left">
        <div className="pane-head">
          <h3>{t("prov.title")}</h3>
          <span className="spacer" />
          <motion.button whileTap={tap} className="btn small primary" onClick={() => navigate("models", "new")}>
            <PlusIcon size={13} /> {t("common.add")}
          </motion.button>
        </div>
        <ErrorBar error={list.error} />
        <ul className="item-list provider-list">
          {(list.data ?? []).map((p) => (
            <li key={p.id} className={p.id === selected ? "selected" : ""} onClick={() => navigate("models", p.id)}>
              {p.id === selected && <motion.span layoutId="provider-list-active" className="list-active" transition={spring} />}
              <div className="provider-row">
                <span className="provider-kind">
                  <KindIcon kind={p.kind === "mock" ? "mock" : p.vendor ?? p.kind} size={16} />
                </span>
                <span className="provider-text">
                  <span className="provider-name">
                    {p.name}
                    {!p.is_enabled && <span className="pill tiny">{t("common.disabled")}</span>}
                  </span>
                  <span className="provider-sub mono">{p.default_model || t("prov.noDefaultModel")}</span>
                </span>
                <button
                  className="btn ghost small"
                  title={t("prov.testConnection")}
                  onClick={(e) => {
                    e.stopPropagation();
                    runTest(p.id);
                  }}
                >
                  {t("prov.test")}
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
                <h2>{current ? current.name : t("prov.add")}</h2>
                <p>
                  {current
                    ? isMock
                      ? `Mock · ${t("prov.sub.mock")}`
                      : `${preset.name} · ${proto.label}`
                    : t("prov.addSub")}
                </p>
              </div>
              <div className="pf-head-aside">
                <span className="pf-status">{form.is_enabled ? t("common.enabled") : t("common.disabled")}</span>
                <Switch checked={form.is_enabled} onChange={(v) => set("is_enabled", v)} />
              </div>
            </header>
            <ErrorBar error={error} onDismiss={() => setError(null)} />

            {!isMock && (
              <>
                <div className="kind-tiles vendor-grid" role="radiogroup" aria-label={t("prov.kinds")}>
                  {VENDOR_PRESETS.map((v) => {
                    const on = form.vendor === v.id;
                    return (
                      <button
                        key={v.id}
                        type="button"
                        role="radio"
                        aria-checked={on}
                        disabled={!!current && !on}
                        className={`kind-tile ${on ? "on" : ""}`}
                        onClick={() => selectVendor(v.id)}
                        title={current && !on ? t("prov.kindLocked") : undefined}
                      >
                        {on && <motion.span layoutId="kind-tile-active" className="kind-tile-ring" transition={spring} />}
                        <span className="kind-tile-icon">
                          <KindIcon kind={v.id} />
                        </span>
                        <span className="kind-tile-name">{v.name}</span>
                        <span className="kind-tile-sub">{v.sub}</span>
                      </button>
                    );
                  })}
                </div>
                <Collapse open={preset.protocols.length > 1}>
                  <div className="pf-protocol-bar">
                    <Label hint={current ? <span className="field-hint">{t("prov.protocolLocked")}</span> : undefined}>
                      {t("prov.protocol")}
                    </Label>
                    <SegmentedControl<ProviderKind>
                      full
                      ariaLabel={t("prov.protocol")}
                      value={form.kind}
                      onChange={(k) => !current && selectProtocol(k)}
                      options={preset.protocols.map((p) => ({
                        value: p.kind,
                        label: p.recommended ? `${p.label} (${t("prov.protocolRecommended")})` : p.label,
                        disabled: !!current && p.kind !== form.kind,
                      }))}
                    />
                  </div>
                </Collapse>
                {current && <p className="pf-note">{t("prov.kindLockedNote")}</p>}
              </>
            )}

            <Section title={t("prov.connection")} description={t("prov.connectionSub")}>
              <div className="pf-grid">
                <div className="pf-field">
                  <Label htmlFor={ids.name}>{t("prov.name")}</Label>
                  <input id={ids.name} value={form.name} placeholder={t("prov.example", { value: preset.name })} onChange={(e) => set("name", e.target.value)} />
                </div>
                <div className="pf-field">
                  <Label htmlFor={ids.base}>{t("prov.baseUrl")}</Label>
                  <input id={ids.base} value={form.base_url} placeholder={basePlaceholder} disabled={isMock} onChange={(e) => set("base_url", e.target.value)} />
                  <span className="field-hint">{preset.baseRequired ? t("prov.baseRequiredHint") : t("prov.baseOptionalHint")}</span>
                </div>
              </div>
              {!isMock && (
                <div className="pf-field">
                  <Label hint={preset.apiKeyOptional ? <span className="field-hint">{t("prov.apiKeyOptional")}</span> : undefined}>
                    {t("prov.apiKey")}
                  </Label>
                  <SecretInput current={current?.api_key} source={current?.api_key_source} value={form.api_key} onChange={(v) => set("api_key", v)} envName={preset.env} />
                </div>
              )}
            </Section>

            {!isMock && (
              <Section title={t("prov.serverTools")} description={t("prov.serverToolsSub")}>
                {hasAnyServerTools ? (
                  <div className="pf-server-tools">
                    {supportedTools.web_search && (
                      <div className="pf-tool-row">
                        <div className="pf-tool-info">
                          <div className="pf-tool-title">
                            <span>{t("prov.toolWebSearch")}</span>
                            <span className="pill tiny mono">{supportedTools.web_search}</span>
                          </div>
                          <span className="field-hint">{t("prov.toolWebSearchDesc")}</span>
                        </div>
                        <Switch
                          checked={!!form.default_server_tools.web_search?.enabled}
                          onChange={(on) =>
                            setServerTool((cur) => ({
                              ...cur,
                              web_search: on ? { enabled: true, max_uses: cur.web_search?.max_uses ?? 5 } : null,
                            }))
                          }
                        />
                      </div>
                    )}
                    {supportedTools.url_context && (
                      <div className="pf-tool-row">
                        <div className="pf-tool-info">
                          <div className="pf-tool-title">
                            <span>{t("prov.toolUrlContext")}</span>
                            <span className="pill tiny mono">{supportedTools.url_context}</span>
                          </div>
                          <span className="field-hint">{t("prov.toolUrlContextDesc")}</span>
                        </div>
                        <Switch
                          checked={!!form.default_server_tools.url_context}
                          onChange={(on) => setServerTool((cur) => ({ ...cur, url_context: on }))}
                        />
                      </div>
                    )}
                    {supportedTools.code_execution && (
                      <div className="pf-tool-row">
                        <div className="pf-tool-info">
                          <div className="pf-tool-title">
                            <span>{t("prov.toolCodeExecution")}</span>
                            <span className="pill tiny mono">{supportedTools.code_execution}</span>
                          </div>
                          <span className="field-hint">{t("prov.toolCodeExecutionDesc")}</span>
                        </div>
                        <Switch
                          checked={!!form.default_server_tools.code_execution}
                          onChange={(on) => setServerTool((cur) => ({ ...cur, code_execution: on }))}
                        />
                      </div>
                    )}
                    {supportedTools.file_search && (
                      <div className="pf-tool-row pf-tool-row-col">
                        <div className="pf-tool-row-top">
                          <div className="pf-tool-info">
                            <div className="pf-tool-title">
                              <span>{t("prov.toolFileSearch")}</span>
                              <span className="pill tiny mono">{supportedTools.file_search}</span>
                            </div>
                            <span className="field-hint">{t("prov.toolFileSearchDesc")}</span>
                          </div>
                          <Switch
                            checked={!!form.default_server_tools.file_search?.enabled}
                            onChange={(on) =>
                              setServerTool((cur) => ({
                                ...cur,
                                file_search: on
                                  ? { enabled: true, vector_store_ids: cur.file_search?.vector_store_ids ?? [] }
                                  : null,
                              }))
                            }
                          />
                        </div>
                        <Collapse open={!!form.default_server_tools.file_search?.enabled}>
                          <input
                            className="mono"
                            placeholder={t("prov.vectorStorePlaceholder")}
                            value={(form.default_server_tools.file_search?.vector_store_ids ?? []).join(", ")}
                            onChange={(e) =>
                              setServerTool((cur) => ({
                                ...cur,
                                file_search: {
                                  enabled: true,
                                  vector_store_ids: e.target.value
                                    .split(",")
                                    .map((s) => s.trim())
                                    .filter(Boolean),
                                },
                              }))
                            }
                          />
                        </Collapse>
                      </div>
                    )}
                  </div>
                ) : (
                  <p className="field-hint">{t("prov.serverToolsUnsupported")}</p>
                )}
              </Section>
            )}

            <Section title={t("prov.model")} description={t("prov.modelSub")}>
              <div className="pf-field">
                <Label
                  htmlFor={ids.model}
                  hint={
                    <button type="button" className="btn ghost small" onClick={fetchModels} disabled={modelsState === "loading"}>
                      {modelsState === "loading" ? <Spinner size={13} /> : <RefreshIcon size={13} />}
                      {t("prov.fetchModels")}
                    </button>
                  }
                >
                  {t("prov.defaultModel")}
                </Label>
                <Combobox value={form.default_model} onChange={(v) => set("default_model", v)} options={models} placeholder={preset.defaultModel ? t("prov.example", { value: preset.defaultModel }) : "model-id"} />
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
                      ? t("prov.modelsError", { error: modelsState.error })
                      : models.length
                        ? t("prov.modelsFound", { count: models.length })
                        : t("prov.modelsHint")}
                  </motion.span>
                </AnimatePresence>
              </div>
            </Section>

            <Section title={t("prov.generation")} description={t("prov.generationSub")}>
              <div className="pf-field">
                <Label>{t("reasoning.label")}</Label>
                <SegmentedControl<ReasoningEffort>
                  full
                  ariaLabel={t("reasoning.label")}
                  value={form.defaults.reasoning_effort}
                  onChange={(v) => setD("reasoning_effort", v)}
                  options={REASONING_LEVELS.map((l) => ({ value: l.value, label: t(l.label), title: t(l.description) }))}
                />
                <AnimatePresence mode="wait" initial={false}>
                  <motion.span key={level.value} className="field-hint" initial={{ opacity: 0, y: -2 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} transition={{ duration: 0.12 }}>
                    {t(level.description)}
                    {level.value !== "default" && ` · ${t("prov.reasoningIgnored")}`}
                  </motion.span>
                </AnimatePresence>
              </div>

              <div className="pf-field">
                <Label
                  htmlFor={ids.temp}
                  hint={
                    <span className="pf-temp-value">
                      <span className="tabular">{temp == null ? t("prov.default") : fmt.number(temp, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}</span>
                      {temp != null && (
                        <button type="button" className="text-btn" onClick={() => setD("temperature", null)}>
                          {t("common.reset")}
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
                <span className="field-hint">{t("prov.temperatureHint")}</span>
              </div>

              <div className="pf-grid">
                <div className="pf-field">
                  <Label>{t("prov.maxOutput")}</Label>
                  <NumberField value={form.defaults.max_output_tokens} onChange={(v) => setD("max_output_tokens", v)} placeholder={t("prov.defaultValue", { n: fmt.number(4096) })} presets={OUTPUT_PRESETS} />
                </div>
                <div className="pf-field">
                  <Label>{t("prov.contextWindow")}</Label>
                  <NumberField value={form.defaults.context_window} onChange={(v) => setD("context_window", v)} placeholder={t("prov.defaultValue", { n: fmt.number(200000) })} presets={CONTEXT_PRESETS} />
                  <span className="field-hint">{t("prov.contextHint")}</span>
                </div>
              </div>
            </Section>

            <section className="pf-section">
              <button type="button" className="pf-disclosure" aria-expanded={advanced} onClick={() => setAdvanced(!advanced)}>
                <div>
                  <h3>{t("prov.advanced")}</h3>
                  <p>{t("prov.advancedSub")}</p>
                </div>
                <motion.span animate={{ rotate: advanced ? 180 : 0 }} transition={spring} className="pf-disclosure-chev">
                  <ChevronDown size={16} />
                </motion.span>
              </button>
              <Collapse open={advanced}>
                <div className="pf-section-body pf-advanced">
                  <div className="pf-grid">
                    <div className="pf-field">
                      <Label htmlFor={ids.timeout}>{t("prov.timeout")}</Label>
                      <div className="input-affix">
                        <input
                          id={ids.timeout}
                          inputMode="numeric"
                          className="tabular"
                          value={form.defaults.timeout_secs}
                          onChange={(e) => setD("timeout_secs", Number(e.target.value.replace(/[^\d]/g, "")) || 0)}
                        />
                        <span className="affix-text">{t("prov.seconds")}</span>
                      </div>
                      <span className="field-hint">{t("prov.timeoutHint")}</span>
                    </div>
                    <div className="pf-field">
                      <Label>{t("prov.autoRetry")}</Label>
                      <div className="pf-inline">
                        <Switch checked={form.auto_retry.enabled} onChange={(v) => set("auto_retry", { ...form.auto_retry, enabled: v })} />
                        <span className="muted">{t("prov.upTo")}</span>
                        <div className="input-affix narrow">
                          <input
                            inputMode="numeric"
                            className="tabular"
                            disabled={!form.auto_retry.enabled}
                            value={form.auto_retry.max_attempts}
                            onChange={(e) => set("auto_retry", { ...form.auto_retry, max_attempts: Math.min(10, Math.max(1, Number(e.target.value.replace(/[^\d]/g, "")) || 1)) })}
                          />
                          <span className="affix-text">{t("prov.times")}</span>
                        </div>
                      </div>
                      <span className="field-hint">{t("prov.autoRetryHint")}</span>
                    </div>
                  </div>
                  <KeyValueEditor
                    title={t("prov.headers")}
                    hint={t("prov.headersHint")}
                    addLabel={t("prov.addHeader")}
                    emptyText={t("prov.noHeaders")}
                    keyPlaceholder={t("prov.headerName")}
                    value={form.extra_headers}
                    onChange={(v) => set("extra_headers", v)}
                  />
                  <div className="pf-field">
                    <Label>{t("prov.extraBody")}</Label>
                    <textarea
                      rows={4}
                      className="mono"
                      placeholder='{"enable_thinking": true}'
                      value={form.extra_body_text}
                      onChange={(e) => set("extra_body_text", e.target.value)}
                    />
                    <span className={`field-hint ${extraBodyStatus.error ? "error-text" : ""}`}>
                      {extraBodyStatus.error ? t("prov.extraBodyInvalid") : t("prov.extraBodyHint")}
                    </span>
                  </div>
                </div>
              </Collapse>
            </section>

            {current && (
              <div className="pf-danger">
                <div>
                  <b>{t("prov.deleteSection")}</b>
                  <p>{t("prov.deleteSectionBody")}</p>
                </div>
                <button className="btn danger-outline" onClick={() => setConfirm(true)}>
                  <TrashIcon size={14} /> {t("common.delete")}
                </button>
              </div>
            )}

            <div className="pf-footer">
              <ResultBanner result={formTest} />
              <div className="pf-actions">
                <motion.button whileTap={tap} className="btn" onClick={testForm} disabled={formTest === "running"} title={t("prov.testHint")}>
                  {formTest === "running" ? <Spinner size={13} /> : null}
                  {t("prov.testConnection")}
                </motion.button>
                <Checkbox checked={ping} onChange={setPing} label={t("prov.ping")} />
                <span className="spacer" />
                <button className="btn ghost" onClick={() => (current ? setForm(toForm(current)) : navigate("models"))}>
                  {current ? t("prov.revert") : t("common.cancel")}
                </button>
                <motion.button whileTap={tap} className="btn primary" onClick={save} disabled={saving}>
                  {saving && <Spinner size={13} />}
                  {current ? t("common.save") : t("common.create")}
                </motion.button>
              </div>
            </div>
          </motion.div>
        ) : (
          <Empty>{t("prov.empty")}</Empty>
        )}
      </main>
      {confirm && current && (
        <Modal title={t("prov.deleteTitle")} onClose={() => setConfirm(false)}>
          <p>
            {t("prov.deleteBody", { name: current.name })}
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
