import { useEffect, useState } from "react";
import { errorText, mcpServers, oauth as oauthApi } from "../api";
import type { McpServer, McpServerInput, McpTest, McpToolInfo, McpTransport, ToolCallResult } from "../api/types";
import { ToolIcon } from "../brand/Icons";
import { Badge, ErrorBar, JsonView, Modal } from "../components/common";
import { Field, JsonEditor, KeyValueEditor, LinesEditor, SecretInput, Toggle } from "../components/forms";
import { SegmentedControl, Select } from "../components/ui";
import { AddButton, EmptyState, HeaderActions, ListItem, useAutoSelect } from "../components/settings/MasterDetail";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";
import { useI18n, useT } from "../i18n";

interface Form {
  name: string;
  transport: McpTransport;
  url: string;
  headers: Record<string, string>;
  auth: "none" | "bearer" | "oauth";
  token: string;
  oauthClientId: string;
  oauthClientSecret: string;
  oauthScopes: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  is_enabled: boolean;
}

const blank: Form = {
  name: "",
  transport: "streamable_http",
  url: "",
  headers: {},
  auth: "none",
  token: "",
  oauthClientId: "",
  oauthClientSecret: "",
  oauthScopes: "",
  command: "",
  args: [],
  env: {},
  is_enabled: true,
};

const toForm = (m: McpServer): Form => ({
  name: m.name,
  transport: m.transport,
  url: m.url ?? "",
  headers: m.headers,
  auth: m.auth.type === "bearer" ? "bearer" : m.auth.type === "oauth" ? "oauth" : "none",
  token: "",
  oauthClientId: m.auth.type === "oauth" ? String(m.auth.client_id ?? "") : "",
  // Shown masked by the server; sent back unchanged it keeps the stored secret.
  oauthClientSecret: m.auth.type === "oauth" ? String(m.auth.client_secret ?? "") : "",
  oauthScopes: m.auth.type === "oauth" ? String(m.auth.scopes ?? "") : "",
  command: m.command ?? "",
  args: m.args,
  env: m.env,
  is_enabled: m.is_enabled,
});

function toInput(f: Form): McpServerInput {
  const common = { name: f.name, transport: f.transport, is_enabled: f.is_enabled };
  return f.transport === "stdio"
    ? { ...common, command: f.command, args: f.args, env: f.env }
    : {
        ...common,
        url: f.url,
        headers: f.headers,
        auth:
          f.auth === "bearer"
            ? { type: "bearer", token: f.token }
            : f.auth === "oauth"
              ? { type: "oauth", client_id: f.oauthClientId.trim(), client_secret: f.oauthClientSecret, scopes: f.oauthScopes.trim() }
              : { type: "none" },
      };
}

/** A starting point for the arguments of `schema` (every property with an empty value). */
function skeleton(schema: unknown): unknown {
  const s = schema as { type?: string; properties?: Record<string, unknown>; default?: unknown } | null;
  if (!s || typeof s !== "object") return {};
  if (s.default !== undefined) return s.default;
  switch (s.type) {
    case "string":
      return "";
    case "number":
    case "integer":
      return 0;
    case "boolean":
      return false;
    case "array":
      return [];
    default:
      return Object.fromEntries(Object.entries(s.properties ?? {}).map(([k, v]) => [k, skeleton(v)]));
  }
}

function ToolList({ tools, selected, onPick }: { tools: McpToolInfo[]; selected: string | null; onPick: (t: McpToolInfo) => void }) {
  const t = useT();
  return (
    <ul className="tool-list">
      {tools.map((x) => (
        <li key={x.name} className={x.name === selected ? "selected" : ""}>
          <div className="row">
            <b className="mono">{x.name}</b>
            {x.read_only ? <Badge tone="ok">{t("mcp.readOnly")}</Badge> : <Badge tone="warn">{t("mcp.sideEffects")}</Badge>}
            {x.title && <span className="muted">{x.title}</span>}
            <span className="spacer" />
            <button className="btn small" onClick={() => onPick(x)}>
              {t("mcp.try")}
            </button>
          </div>
          {x.description && <div className="muted">{x.description}</div>}
          <details>
            <summary className="muted">{t("mcp.schema")}</summary>
            <JsonView value={x.input_schema} />
          </details>
        </li>
      ))}
    </ul>
  );
}

function Playground({ server, tools, tool, onTool }: { server: McpServer; tools: McpToolInfo[]; tool: string | null; onTool: (t: string) => void }) {
  const [args, setArgs] = useState("{}");
  const [parsed, setParsed] = useState<unknown>({});
  const [timeout, setTimeoutMs] = useState("");
  const [result, setResult] = useState<ToolCallResult | null>(null);
  const [running, setRunning] = useState(false);
  const [raw, setRaw] = useState(false);
  const info = tools.find((x) => x.name === tool);
  const { t, fmt } = useI18n();

  useEffect(() => {
    if (!info) return;
    const sk = skeleton(info.input_schema);
    setArgs(JSON.stringify(sk, null, 2));
    setParsed(sk);
    setResult(null);
  }, [info?.name]); // eslint-disable-line react-hooks/exhaustive-deps

  const run = async () => {
    if (!tool || parsed === undefined) return;
    setRunning(true);
    try {
      setResult(await mcpServers.callTool(server.id, tool, parsed, timeout ? Number(timeout) : undefined));
    } catch (e) {
      setResult({ ok: false, latency_ms: 0, is_error: true, content: null, structured_content: null, text: null, error: errorText(e) });
    } finally {
      setRunning(false);
    }
  };

  return (
    <section className="playground">
      <h3>{t("mcp.playground")}</h3>
      <div className="muted">{t("mcp.playgroundHint")}</div>
      <div className="grid2">
        <Field label={t("mcp.tool")}>
          <Select
            ariaLabel={t("mcp.tool")}
            placeholder={tools.length ? t("mcp.pickTool") : t("mcp.discoverFirst")}
            disabled={tools.length === 0}
            value={tool ?? null}
            onChange={onTool}
            menuClassName="mono-menu"
            options={tools.map((x) => ({ value: x.name, label: x.name, description: x.read_only ? undefined : t("mcp.hasSideEffects") }))}
          />
        </Field>
        <Field label={t("mcp.timeout")}>
          <input value={timeout} placeholder={t("mcp.serverDefault")} inputMode="numeric" onChange={(e) => setTimeoutMs(e.target.value.replace(/\D/g, ""))} />
        </Field>
      </div>
      <Field label={t("mcp.args")}>
        <JsonEditor
          value={args}
          rows={8}
          onChange={(text, p) => {
            setArgs(text);
            setParsed(p);
          }}
        />
      </Field>
      <div className="row">
        <button className="primary" disabled={!tool || parsed === undefined || running || !server.is_enabled} onClick={run}>
          {running ? t("mcp.running") : t("mcp.run")}
        </button>
        {!server.is_enabled && <span className="muted">{t("mcp.serverDisabled")}</span>}
        {info && !info.read_only && <Badge tone="warn">{t("mcp.toolSideEffects")}</Badge>}
      </div>
      {result && (
        <div className="tool-output">
          <div className="row">
            <Badge tone={!result.ok ? "error" : result.is_error ? "warn" : "ok"}>
              {!result.ok ? t("mcp.callFailed") : result.is_error ? t("mcp.toolError") : t("mcp.success")}
            </Badge>
            <span className="muted">{fmt.ms(result.latency_ms)}</span>
            <span className="spacer" />
            <SegmentedControl
              size="sm"
              ariaLabel={t("mcp.format")}
              value={raw ? "raw" : "text"}
              onChange={(v) => setRaw(v === "raw")}
              options={[
                { value: "text", label: t("mcp.text") },
                { value: "raw", label: "JSON" },
              ]}
            />
          </div>
          {result.error && <pre className="error-text">{result.error}</pre>}
          {raw ? (
            <JsonView value={{ content: result.content, structuredContent: result.structured_content, isError: result.is_error }} />
          ) : (
            result.text != null && <pre>{result.text || t("common.empty")}</pre>
          )}
        </div>
      )}
    </section>
  );
}

/** OAuth connection: authorize in a popup, see scopes and expiry, reconnect or disconnect. */
function OAuthPanel({ server, onChanged }: { server: McpServer; onChanged: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { t, fmt } = useI18n();
  const o = server.oauth;

  useEffect(() => {
    // The callback page posts this to its opener when the authorization finishes.
    const onMsg = (e: MessageEvent) => {
      const d = e.data as { type?: string; server_id?: string; ok?: boolean; error?: string };
      if (d?.type !== "oort-oauth" || d.server_id !== server.id) return;
      setBusy(false);
      if (!d.ok) setError(d.error ?? t("mcp.authFailed"));
      onChanged();
    };
    window.addEventListener("message", onMsg);
    return () => window.removeEventListener("message", onMsg);
  }, [server.id, onChanged]);

  const authorize = async () => {
    setError(null);
    setBusy(true);
    // Open first (synchronously) so the popup is not blocked, then point it at the AS.
    const popup = window.open("about:blank", "oort-oauth", "width=520,height=720");
    try {
      const { authorization_url } = await oauthApi.start(server.id);
      if (popup) popup.location.href = authorization_url;
      else window.location.href = authorization_url;
    } catch (e) {
      popup?.close();
      setError(errorText(e));
      setBusy(false);
    }
  };
  const disconnect = async () => {
    try {
      await oauthApi.disconnect(server.id);
      onChanged();
    } catch (e) {
      setError(errorText(e));
    }
  };
  const connected = o?.status === "connected";
  const expires = o?.expires_at ? fmt.date(new Date(o.expires_at).getTime()) : null;

  return (
    <section className="oauth-panel">
      <div className="row">
        <span className={`dot ${connected ? "ok" : o ? "warn" : "muted"}`} />
        <b>{connected ? t("mcp.connected") : o ? t("mcp.reauthNeeded") : t("mcp.notConnected")}</b>
        {o && <span className="muted">{o.issuer}</span>}
        <span className="spacer" />
        {connected ? (
          <>
            <button className="btn small" disabled={busy} onClick={authorize}>
              {t("mcp.reauth")}
            </button>
            <button className="btn small danger-outline" onClick={disconnect}>
              {t("mcp.disconnect")}
            </button>
          </>
        ) : (
          <button className="btn primary small" disabled={busy || !server.is_enabled} onClick={authorize}>
            {busy ? t("mcp.authorizing") : t("mcp.authorize")}
          </button>
        )}
      </div>
      {o && (
        <div className="oauth-meta">
          <span>
            {t("mcp.scopes")} <code>{o.scopes || t("mcp.noScopes")}</code>
          </span>
          {expires && <span>{t("mcp.expires", { when: expires })}</span>}
          <span>
            {t("mcp.client", { how: o.client_source === "dcr" ? t("mcp.dcr") : t("mcp.manual"), id: o.client_id ?? "" })}
          </span>
          {o.has_refresh_token && <span>{t("mcp.autoRefresh")}</span>}
        </div>
      )}
      {error && <div className="error-text">{error}</div>}
    </section>
  );
}

export function McpView({ selected }: { selected: string | null }) {
  const list = useResource(mcpServers.list);
  const current = list.data?.find((m) => m.id === selected) ?? null;
  const isNew = selected === "new";
  const [form, setForm] = useState<Form>(blank);
  const [error, setError] = useState<string | null>(null);
  const [test, setTest] = useState<McpTest | "running" | null>(null);
  const [tool, setTool] = useState<string | null>(null);
  const [confirm, setConfirm] = useState(false);
  const { t, fmt } = useI18n();

  useEffect(() => {
    setForm(current ? toForm(current) : blank);
    setError(null);
  }, [current?.id, current?.updated_at, isNew]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    setTest(null);
    setTool(null);
  }, [current?.id, isNew]);

  useAutoSelect("mcp", list.data, selected, (m) => m.is_enabled);

  const set = <K extends keyof Form>(k: K, v: Form[K]) => setForm((f) => ({ ...f, [k]: v }));

  const discover = async () => {
    setTest("running");
    try {
      setTest(await mcpServers.test(current?.id ?? null, toInput(form)));
    } catch (e) {
      setTest({ ok: false, latency_ms: 0, server_info: null, tools: [], error: errorText(e) });
    }
  };

  const save = async () => {
    try {
      const saved = current ? await mcpServers.update(current.id, toInput(form)) : await mcpServers.create(toInput(form));
      await list.reload();
      navigate("mcp", saved.id);
    } catch (e) {
      setError(errorText(e));
    }
  };

  const remove = async () => {
    setConfirm(false);
    if (!current) return;
    try {
      await mcpServers.remove(current.id);
      await list.reload();
      navigate("mcp");
    } catch (e) {
      setError(errorText(e));
    }
  };

  const tools = test && test !== "running" ? test.tools : [];

  if (list.data?.length === 0 && !isNew)
    return (
      <>
        <HeaderActions>
          <AddButton label={t("common.add")} onClick={() => navigate("mcp", "new")} />
        </HeaderActions>
        <ErrorBar error={list.error} />
        <EmptyState icon={<ToolIcon size={20} />} title={t("mcp.emptyTitle")} action={{ label: t("common.add"), onClick: () => navigate("mcp", "new") }}>
          {t("mcp.emptyHint")}
        </EmptyState>
      </>
    );

  return (
    <div className="master-detail">
      <HeaderActions>
        <AddButton label={t("common.add")} onClick={() => navigate("mcp", "new")} />
      </HeaderActions>
      <aside className="pane left">
        <ErrorBar error={list.error} />
        <ul className="md-list">
          {(list.data ?? []).map((m) => (
            <ListItem
              key={m.id}
              selected={m.id === selected}
              onSelect={() => navigate("mcp", m.id)}
              icon={<ToolIcon size={15} />}
              title={m.name}
              sub={m.transport === "stdio" ? `stdio · ${[m.command, ...m.args].join(" ")}` : m.url}
              mono
              off={m.is_enabled ? undefined : t("common.disabled")}
            />
          ))}
        </ul>
      </aside>

      <main className="pane detail">
        {current || isNew ? (
          <>
            <div className="form">
              <h2>{current ? current.name : t("mcp.add")}</h2>
              <ErrorBar error={error} onDismiss={() => setError(null)} />
              <div className="grid2">
                <Field label={t("mcp.name")} hint={t("mcp.nameHint")}>
                  <input value={form.name} placeholder="[A-Za-z0-9_-]" onChange={(e) => set("name", e.target.value)} />
                </Field>
                <Field label={t("mcp.transport")}>
                  <SegmentedControl<McpTransport>
                    ariaLabel={t("mcp.transport")}
                    value={form.transport}
                    onChange={(v) => set("transport", v)}
                    options={[
                      { value: "streamable_http", label: "HTTP", title: "Streamable HTTP" },
                      { value: "stdio", label: "stdio", title: t("mcp.stdioHint") },
                    ]}
                  />
                </Field>
              </div>
              {form.transport === "streamable_http" ? (
                <>
                  <Field label="URL">
                    <input value={form.url} placeholder="https://example.com/mcp" onChange={(e) => set("url", e.target.value)} />
                  </Field>
                  <KeyValueEditor
                    title={t("mcp.headers")}
                    hint={t("mcp.headersHint")}
                    addLabel={t("mcp.addHeader")}
                    emptyText={t("mcp.noHeaders")}
                    keyPlaceholder={t("mcp.headerName")}
                    value={form.headers}
                    onChange={(v) => set("headers", v)}
                  />
                  <div className="grid2">
                    <Field label={t("mcp.auth")}>
                      <Select<Form["auth"]>
                        ariaLabel={t("mcp.auth")}
                        value={form.auth}
                        onChange={(v) => set("auth", v)}
                        options={[
                          { value: "none", label: t("common.none") },
                          { value: "bearer", label: t("mcp.bearer"), description: t("mcp.bearerHint") },
                          { value: "oauth", label: "OAuth 2.0", description: t("mcp.oauthHint") },
                        ]}
                      />
                    </Field>
                    {form.auth === "oauth" && (
                      <Field label={t("mcp.scopesOptional")} hint={t("mcp.scopesHint")}>
                        <input value={form.oauthScopes} placeholder="mcp:tools" onChange={(e) => set("oauthScopes", e.target.value)} />
                      </Field>
                    )}
                    {form.auth === "bearer" && (
                      <Field label={t("mcp.token")}>
                        <SecretInput
                          current={current?.auth.type === "bearer" ? current.auth.token : null}
                          source={current?.auth.type === "bearer" ? current.auth.token_source : "none"}
                          value={form.token}
                          onChange={(v) => set("token", v)}
                        />
                      </Field>
                    )}
                  </div>
                  {form.auth === "oauth" && (
                    <details className="advanced">
                      <summary>{t("mcp.manualClient")}</summary>
                      <div className="grid2">
                        <Field label="client_id">
                          <input value={form.oauthClientId} placeholder={t("mcp.clientIdPlaceholder")} onChange={(e) => set("oauthClientId", e.target.value)} />
                        </Field>
                        <Field label="client_secret" hint={t("mcp.clientSecretHint")}>
                          <input type="password" value={form.oauthClientSecret} onChange={(e) => set("oauthClientSecret", e.target.value)} />
                        </Field>
                      </div>
                    </details>
                  )}
                </>
              ) : (
                <>
                  <Field label={t("mcp.command")}>
                    <input className="mono" value={form.command} placeholder="npx" onChange={(e) => set("command", e.target.value)} />
                  </Field>
                  <Field label={t("mcp.argsLines")} hint={t("mcp.argsLinesHint")}>
                    <LinesEditor value={form.args} onChange={(v) => set("args", v)} placeholder={"-y\n@modelcontextprotocol/server-filesystem\n."} />
                  </Field>
                  <KeyValueEditor
                    title={t("mcp.env")}
                    hint={t("mcp.envHint")}
                    addLabel={t("mcp.addVar")}
                    emptyText={t("mcp.noVars")}
                    keyPlaceholder="VAR"
                    value={form.env}
                    onChange={(v) => set("env", v)}
                  />
                </>
              )}
              <Toggle checked={form.is_enabled} onChange={(v) => set("is_enabled", v)} label={t("mcp.enabled")} />
              <div className="row actions">
                <button className="primary" onClick={save}>
                  {current ? t("common.save") : t("common.create")}
                </button>
                <button onClick={discover} disabled={test === "running"} title={t("mcp.testHint")}>
                  {test === "running" ? t("mcp.connecting") : t("mcp.discover")}
                </button>
                <span className="spacer" />
                {current && (
                  <button className="danger-outline" onClick={() => setConfirm(true)}>
                    {t("common.delete")}
                  </button>
                )}
              </div>
              <div className="muted">{t("mcp.changesNote")}</div>
            </div>

            {current?.auth.type === "oauth" && <OAuthPanel server={current} onChanged={list.reload} />}
            {test && test !== "running" && (
              <section className="discovery">
                <div className="row">
                  <Badge tone={test.ok ? "ok" : "error"}>{test.ok ? t("mcp.statusConnected") : t("mcp.statusFailed")}</Badge>
                  <span className="muted">{fmt.ms(test.latency_ms)}</span>
                  {test.server_info && (
                    <span className="muted mono">
                      {test.server_info.name} {test.server_info.version} · {t("mcp.protocol", { v: test.server_info.protocol_version ?? "" })}
                    </span>
                  )}
                  {test.ok && <span className="muted">· {t("mcp.toolCount", { count: test.tools.length })}</span>}
                </div>
                {test.error && <pre className="error-text">{test.error}</pre>}
                {test.server_info?.instructions && <div className="muted">{test.server_info.instructions}</div>}
                <ToolList tools={test.tools} selected={tool} onPick={(t) => setTool(t.name)} />
              </section>
            )}
            {current && <Playground server={current} tools={tools} tool={tool} onTool={setTool} />}
          </>
        ) : (
          null
        )}
      </main>
      {confirm && current && (
        <Modal title={t("mcp.deleteTitle")} onClose={() => setConfirm(false)}>
          <p>{t("mcp.deleteBody", { name: current.name })}</p>
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
