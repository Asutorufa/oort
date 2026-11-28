import { useState } from "react";
import type { ApprovalPreview, PendingApproval, ToolCall, ToolResult } from "../../api/types";
import { DiffView } from "../../widgets/ApprovalDiff";
import { DecisionFooter } from "./Decision";
import { JsonView } from "../common";
import { Markdown } from "./LazyMarkdown";
import { useT, type Key, type T } from "../../i18n";

// ----- ask_user ------------------------------------------------------------------------------

interface Question {
  question: string;
  kind?: "single" | "multiple" | "text";
  options?: string[];
  placeholder?: string;
}

/** A structured question from the agent; answering resumes the turn. */
export function AskUserCard({
  call,
  pending,
  result,
  onAnswer,
}: {
  call: ToolCall;
  pending: boolean;
  result?: ToolResult;
  onAnswer: (content: string) => Promise<void>;
}) {
  const t = useT();
  const q = (call.arguments ?? {}) as Question;
  const options = q.options ?? [];
  const kind = q.kind ?? (options.length ? "single" : "text");
  const [picked, setPicked] = useState<string[]>([]);
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const answered = result && !result.is_error ? result.content : null;
  const value = kind === "text" ? text.trim() : kind === "multiple" ? picked.join(", ") : picked[0] ?? "";
  const send = async (v: string) => {
    if (!v) return;
    setSending(true);
    try {
      await onAnswer(v);
    } finally {
      setSending(false);
    }
  };

  return (
    <section className={`widget ask ${answered ? "answered" : ""}`}>
      <header className="widget-head">
        <h4>{q.question}</h4>
        <span className="spacer" />
        {answered ? <span className="pill ok">{t("ask.answered")}</span> : pending ? <span className="pill warn">{t("ask.waiting")}</span> : null}
      </header>
      <div className="widget-body">
        {answered ? (
          <div className="answer">{answered}</div>
        ) : result?.is_error ? (
          <div className="muted">{t("ask.notAnswered", { reason: result.content })}</div>
        ) : (
          <>
            {kind !== "text" && (
              <div className="ask-options">
                {options.map((o) => {
                  const on = picked.includes(o);
                  return (
                    <button
                      key={o}
                      className={`chip-btn ${on ? "chosen" : ""}`}
                      disabled={!pending || sending}
                      onClick={() => {
                        if (kind === "single") send(o);
                        else setPicked(on ? picked.filter((x) => x !== o) : [...picked, o]);
                      }}
                    >
                      {kind === "multiple" && <span className="check-box">{on ? "☑" : "☐"}</span>}
                      {o}
                    </button>
                  );
                })}
              </div>
            )}
            {(kind === "text" || kind === "multiple") && (
              <div className="wactions">
                {kind === "text" && (
                  <input
                    className="reason"
                    value={text}
                    disabled={!pending || sending}
                    placeholder={q.placeholder ?? t("ask.placeholder")}
                    onChange={(e) => setText(e.target.value)}
                    onKeyDown={(e) => e.key === "Enter" && !e.nativeEvent.isComposing && send(value)}
                  />
                )}
                <button className="btn primary" disabled={!pending || sending || !value} onClick={() => send(value)}>
                  {t("ask.submit")}
                </button>
              </div>
            )}
          </>
        )}
      </div>
    </section>
  );
}

// ----- approvals -----------------------------------------------------------------------------

const TITLE: Record<string, Key> = {
  write_file: "approval.title.write_file",
  run_command: "approval.title.run_command",
  save_skill: "approval.title.save_skill",
  configure_mcp_server: "approval.title.configure_mcp_server",
};

/** `/home/me/projects/x` → `~/…/projects/x` style: the last two segments are what matter. */
function shortPath(t: T, p: string | null | undefined): string {
  if (!p) return t("approval.cwd");
  const parts = p.split("/").filter(Boolean);
  return parts.length <= 2 ? p : `…/${parts.slice(-2).join("/")}`;
}

function Preview({ p }: { p: ApprovalPreview }) {
  const t = useT();
  switch (p.kind) {
    case "diff":
      return (
        <>
          <div className="diff-meta">
            <code className="chip">{p.path}</code>
            {p.created && <span className="pill">{t("approval.newFile")}</span>}
          </div>
          <DiffView diff={p.diff || t("approval.noChanges")} />
        </>
      );
    case "command":
      return (
        <div className="cmd-block">
          <div className="cmd-meta">
            <span className="cmd-cwd" title={p.cwd ?? ""}>
              {shortPath(t, p.cwd)}
            </span>
            <span>·</span>
            <span>{t("approval.maxTime", { secs: p.timeout_secs })}</span>
          </div>
          <pre>{p.command}</pre>
        </div>
      );
    case "skill":
      return (
        <div className="skill-preview compact">
          <div className="row">
            <b className="mono">/{p.name}</b>
            {p.update && <span className="pill warn">{t("approval.overwrite")}</span>}
            {p.execution_mode === "subagent" && <span className="pill">Subagent</span>}
            {p.argument_hint && <code className="chip">{p.argument_hint}</code>}
          </div>
          <p className="lead">{p.description}</p>
          <Markdown text={p.body} />
        </div>
      );
    case "mcp_server":
      return (
        <div className="kv-preview">
          <div>
            <span className="muted">{t("approval.name")}</span> <b className="mono">{p.name}</b>
          </div>
          <div>
            <span className="muted">{t("approval.connection")}</span> {p.transport === "stdio" ? <code>{[p.command, ...p.args].join(" ")}</code> : <code>{p.url}</code>}
          </div>
          {Object.keys(p.headers).length > 0 && <JsonView value={p.headers} />}
          {Object.keys(p.env).length > 0 && <JsonView value={p.env} />}
          <div className="note">{t("approval.mcpNote")}</div>
        </div>
      );
    case "error":
      return <div className="error-text">{p.error}</div>;
  }
}

/** Natural-language subtitle for a pending approval (the server's reason is for logs). */
export function approvalSubtitle(t: T, name: string, reason: string): string {
  const byTool: Record<string, Key> = {
    run_command: "approval.reason.run_command",
    write_file: "approval.reason.write_file",
    save_skill: "approval.reason.save_skill",
    configure_mcp_server: "approval.reason.configure_mcp_server",
  };
  if (byTool[name]) return t(byTool[name]);
  if (/always_ask/.test(reason)) return t("approval.reason.alwaysAsk");
  if (/side effects/.test(reason)) return t("approval.reason.sideEffects");
  return t("approval.reason.other");
}

/** Approval for a side-effecting call, with a tool-specific preview. */
export function ApprovalCard({
  pending,
  onApprove,
  onDeny,
}: {
  pending: PendingApproval;
  onApprove: () => Promise<void>;
  onDeny: (reason: string) => Promise<void>;
}) {
  const { call, preview } = pending;
  const t = useT();
  return (
    <section className="widget widget-approval">
      <header className="widget-head">
        <div className="widget-titles">
          <h4>{TITLE[call.name] ? t(TITLE[call.name]) : t("approval.title.other", { tool: call.name.replace("__", " · ") })}</h4>
          <p className="widget-sub">{approvalSubtitle(t, call.name, pending.reason)}</p>
        </div>
        <span className="spacer" />
        <span className="pill warn">{t("approval.pending")}</span>
      </header>
      <div className="widget-body">
        {preview ? <Preview p={preview} /> : <JsonView value={call.arguments} />}
      </div>
      <DecisionFooter canApprove={preview?.kind !== "error"} onApprove={onApprove} onDeny={onDeny} />
    </section>
  );
}

// ----- configure_mcp_server result -----------------------------------------------------------

interface McpConfigured {
  action?: string;
  mcp_server_id?: string;
  name?: string;
  test?: { ok: boolean; error?: string; tools?: string[] } | null;
  discovered_tools?: string[];
}

/** Completed registration: offers to start a new chat with the server (this one stays as is). */
export function McpConfiguredCard({ result, onStart }: { result: ToolResult; onStart: (serverId: string) => Promise<void> }) {
  const t = useT();
  let data: McpConfigured = {};
  try {
    data = JSON.parse(result.content);
  } catch {
    return null;
  }
  if (data.action !== "suggest_new_session" || !data.mcp_server_id) return null;
  const tools = data.discovered_tools ?? [];
  return (
    <section className="widget mcp-done">
      <header className="widget-head">
        <h4>
          {t("mcpDone.title", { name: data.name ?? "" })}
        </h4>
        <span className="spacer" />
        {data.test ? data.test.ok ? <span className="pill ok">{t("mcpDone.ok")}</span> : <span className="pill danger">{t("mcpDone.failed")}</span> : null}
      </header>
      <div className="widget-body">
        {tools.length > 0 && (
          <div className="chips">
            {tools.map((t) => (
              <code key={t} className="chip">
                {t}
              </code>
            ))}
          </div>
        )}
        {data.test?.error && <div className="error-text">{data.test.error}</div>}
        <div className="wactions">
          <button className="btn primary" onClick={() => onStart(data.mcp_server_id!)}>
            {t("mcpDone.start")}
          </button>
          <span className="muted">{t("mcpDone.note")}</span>
        </div>
      </div>
    </section>
  );
}
