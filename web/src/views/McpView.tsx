import { useEffect, useState } from "react";
import { errorText, mcpServers } from "../api";
import type { McpServer, McpServerInput, McpTest, McpToolInfo, McpTransport, ToolCallResult } from "../api/types";
import { Badge, Empty, ErrorBar, fmtMs, JsonView, Modal } from "../components/common";
import { Field, JsonEditor, KeyValueEditor, LinesEditor, SecretInput, Toggle } from "../components/forms";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";

interface Form {
  name: string;
  transport: McpTransport;
  url: string;
  headers: Record<string, string>;
  auth: "none" | "bearer";
  token: string;
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
  auth: m.auth.type === "bearer" ? "bearer" : "none",
  token: "",
  command: m.command ?? "",
  args: m.args,
  env: m.env,
  is_enabled: m.is_enabled,
});

function toInput(f: Form): McpServerInput {
  const common = { name: f.name, transport: f.transport, is_enabled: f.is_enabled };
  return f.transport === "stdio"
    ? { ...common, command: f.command, args: f.args, env: f.env }
    : { ...common, url: f.url, headers: f.headers, auth: f.auth === "bearer" ? { type: "bearer", token: f.token } : { type: "none" } };
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
  return (
    <ul className="tool-list">
      {tools.map((t) => (
        <li key={t.name} className={t.name === selected ? "selected" : ""}>
          <div className="row">
            <b className="mono">{t.name}</b>
            {t.read_only ? <Badge tone="ok">readOnly</Badge> : <Badge tone="warn">side effects</Badge>}
            {t.title && <span className="muted">{t.title}</span>}
            <span className="spacer" />
            <button className="small" onClick={() => onPick(t)}>
              Try
            </button>
          </div>
          {t.description && <div className="muted">{t.description}</div>}
          <details>
            <summary className="muted">input schema</summary>
            <JsonView value={t.input_schema} />
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
  const info = tools.find((t) => t.name === tool);

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
      <h3>Tool Playground</h3>
      <div className="muted">Calls the tool directly on this server — no LLM, no approval.</div>
      <div className="grid2">
        <Field label="Tool">
          <select value={tool ?? ""} onChange={(e) => onTool(e.target.value)}>
            <option value="" disabled>
              {tools.length ? "select a tool" : "run Test & Discover first"}
            </option>
            {tools.map((t) => (
              <option key={t.name} value={t.name}>
                {t.name}
                {t.read_only ? "" : " (side effects)"}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Timeout (ms)">
          <input value={timeout} placeholder="server default" inputMode="numeric" onChange={(e) => setTimeoutMs(e.target.value.replace(/\D/g, ""))} />
        </Field>
      </div>
      <Field label="Arguments (JSON)">
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
          {running ? "Running…" : "Run Tool"}
        </button>
        {!server.is_enabled && <span className="muted">server is disabled</span>}
        {info && !info.read_only && <Badge tone="warn">this tool may have side effects</Badge>}
      </div>
      {result && (
        <div className="tool-output">
          <div className="row">
            <Badge tone={!result.ok ? "error" : result.is_error ? "warn" : "ok"}>
              {!result.ok ? "call failed" : result.is_error ? "is_error" : "ok"}
            </Badge>
            <span className="muted">{fmtMs(result.latency_ms)}</span>
            <span className="spacer" />
            <span className="seg">
              <button className={raw ? "" : "on"} onClick={() => setRaw(false)}>
                Text
              </button>
              <button className={raw ? "on" : ""} onClick={() => setRaw(true)}>
                Raw JSON
              </button>
            </span>
          </div>
          {result.error && <pre className="error-text">{result.error}</pre>}
          {raw ? (
            <JsonView value={{ content: result.content, structuredContent: result.structured_content, isError: result.is_error }} />
          ) : (
            result.text != null && <pre>{result.text || "(empty)"}</pre>
          )}
        </div>
      )}
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

  useEffect(() => {
    setForm(current ? toForm(current) : blank);
    setError(null);
  }, [current?.id, current?.updated_at, isNew]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    setTest(null);
    setTool(null);
  }, [current?.id, isNew]);

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

  return (
    <div className="master-detail">
      <aside className="pane left">
        <div className="pane-head">
          <h3>MCP Servers</h3>
          <button className="primary small" onClick={() => navigate("mcp", "new")}>
            + Add
          </button>
        </div>
        <ErrorBar error={list.error} />
        <ul className="item-list">
          {(list.data ?? []).map((m) => (
            <li key={m.id} className={m.id === selected ? "selected" : ""} onClick={() => navigate("mcp", m.id)}>
              <div className="row">
                <b className="mono">{m.name}</b>
                <span className="spacer" />
                {!m.is_enabled && <Badge>disabled</Badge>}
                <Badge tone="info">{m.transport === "stdio" ? "stdio" : "http"}</Badge>
              </div>
              <div className="muted mono ellipsis">{m.transport === "stdio" ? [m.command, ...m.args].join(" ") : m.url}</div>
            </li>
          ))}
          {list.data?.length === 0 && <li className="muted">none yet</li>}
        </ul>
      </aside>

      <main className="pane detail">
        {current || isNew ? (
          <>
            <div className="form">
              <h2>{current ? current.name : "New MCP server"}</h2>
              <ErrorBar error={error} onDismiss={() => setError(null)} />
              <div className="grid2">
                <Field label="Name" hint="Also the tool prefix in new sessions: name__tool">
                  <input value={form.name} placeholder="[A-Za-z0-9_-]" onChange={(e) => set("name", e.target.value)} />
                </Field>
                <Field label="Transport">
                  <span className="seg">
                    <button className={form.transport === "streamable_http" ? "on" : ""} onClick={() => set("transport", "streamable_http")}>
                      Streamable HTTP
                    </button>
                    <button className={form.transport === "stdio" ? "on" : ""} onClick={() => set("transport", "stdio")}>
                      stdio
                    </button>
                  </span>
                </Field>
              </div>
              {form.transport === "streamable_http" ? (
                <>
                  <Field label="URL">
                    <input value={form.url} placeholder="https://example.com/mcp" onChange={(e) => set("url", e.target.value)} />
                  </Field>
                  <Field label="Headers" hint="Values may be env:NAME. Sensitive values are shown masked and kept unless changed.">
                    <KeyValueEditor value={form.headers} onChange={(v) => set("headers", v)} />
                  </Field>
                  <div className="grid2">
                    <Field label="Auth" hint="OAuth is reserved for later.">
                      <select value={form.auth} onChange={(e) => set("auth", e.target.value as Form["auth"])}>
                        <option value="none">none</option>
                        <option value="bearer">Bearer token</option>
                      </select>
                    </Field>
                    {form.auth === "bearer" && (
                      <Field label="Token">
                        <SecretInput
                          current={current?.auth.type === "bearer" ? current.auth.token : null}
                          source={current?.auth.type === "bearer" ? current.auth.token_source : "none"}
                          value={form.token}
                          onChange={(v) => set("token", v)}
                        />
                      </Field>
                    )}
                  </div>
                </>
              ) : (
                <>
                  <Field label="Command">
                    <input className="mono" value={form.command} placeholder="npx" onChange={(e) => set("command", e.target.value)} />
                  </Field>
                  <Field label="Arguments" hint="one per line">
                    <LinesEditor value={form.args} onChange={(v) => set("args", v)} placeholder={"-y\n@modelcontextprotocol/server-filesystem\n."} />
                  </Field>
                  <Field label="Environment" hint="Values may be env:NAME (resolved from the server's environment).">
                    <KeyValueEditor value={form.env} onChange={(v) => set("env", v)} keyPlaceholder="VAR" />
                  </Field>
                </>
              )}
              <Toggle checked={form.is_enabled} onChange={(v) => set("is_enabled", v)} label="Enabled" />
              <div className="row actions">
                <button className="primary" onClick={save}>
                  {current ? "Save" : "Create"}
                </button>
                <button onClick={discover} disabled={test === "running"} title="Connects with the form as shown (unsaved changes included)">
                  {test === "running" ? "Connecting…" : "Test & Discover"}
                </button>
                <span className="spacer" />
                {current && (
                  <button className="danger-outline" onClick={() => setConfirm(true)}>
                    Delete
                  </button>
                )}
              </div>
              <div className="muted">Changes apply to new sessions only; existing sessions keep the tool catalog they were created with.</div>
            </div>

            {test && test !== "running" && (
              <section className="discovery">
                <div className="row">
                  <Badge tone={test.ok ? "ok" : "error"}>{test.ok ? "connected" : "failed"}</Badge>
                  <span className="muted">{fmtMs(test.latency_ms)}</span>
                  {test.server_info && (
                    <span className="muted mono">
                      {test.server_info.name} {test.server_info.version} · protocol {test.server_info.protocol_version}
                    </span>
                  )}
                  {test.ok && <span className="muted">· {test.tools.length} tools</span>}
                </div>
                {test.error && <pre className="error-text">{test.error}</pre>}
                {test.server_info?.instructions && <div className="muted">{test.server_info.instructions}</div>}
                <ToolList tools={test.tools} selected={tool} onPick={(t) => setTool(t.name)} />
              </section>
            )}
            {current && <Playground server={current} tools={tools} tool={tool} onTool={setTool} />}
          </>
        ) : (
          <Empty>Select an MCP server, or add one.</Empty>
        )}
      </main>
      {confirm && current && (
        <Modal title="Delete MCP server?" onClose={() => setConfirm(false)}>
          <p>
            <b>{current.name}</b> will be deleted. Existing sessions keep their frozen tools; calls to this server's tools will return errors.
          </p>
          <div className="row">
            <span className="spacer" />
            <button onClick={() => setConfirm(false)}>Cancel</button>
            <button className="danger" onClick={remove}>
              Delete
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
