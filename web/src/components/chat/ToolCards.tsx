import { useState } from "react";
import type { ApprovalPreview, PendingApproval, ToolCall, ToolResult } from "../../api/types";
import { DiffView } from "../../widgets/ApprovalDiff";
import { JsonView } from "../common";
import { Markdown } from "./LazyMarkdown";

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
        {answered ? <span className="pill ok">✓ 回答済み</span> : pending ? <span className="pill warn">回答待ち</span> : null}
      </header>
      <div className="widget-body">
        {answered ? (
          <div className="answer">{answered}</div>
        ) : result?.is_error ? (
          <div className="muted">回答されませんでした（{result.content}）</div>
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
                    placeholder={q.placeholder ?? "回答を入力"}
                    onChange={(e) => setText(e.target.value)}
                    onKeyDown={(e) => e.key === "Enter" && !e.nativeEvent.isComposing && send(value)}
                  />
                )}
                <button className="btn primary" disabled={!pending || sending || !value} onClick={() => send(value)}>
                  回答する
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

const TITLE: Record<string, string> = {
  write_file: "ファイルを書き換えます",
  run_command: "コマンドを実行します",
  save_skill: "Skill を保存します",
  configure_mcp_server: "MCP サーバーを登録します",
};

function Preview({ p }: { p: ApprovalPreview }) {
  switch (p.kind) {
    case "diff":
      return (
        <>
          <div className="diff-meta">
            <code className="chip">{p.path}</code>
            {p.created && <span className="pill accent">新規作成</span>}
          </div>
          <DiffView diff={p.diff || "（変更なし）"} />
        </>
      );
    case "command":
      return (
        <div className="terminal">
          <div className="term-bar">
            <span className="term-dot" />
            <span className="term-dot" />
            <span className="term-dot" />
            <span className="term-cwd">{p.cwd ?? ""}</span>
            <span className="term-timeout">最長 {p.timeout_secs}s</span>
          </div>
          <pre>
            <span className="term-prompt">$ </span>
            {p.command}
          </pre>
        </div>
      );
    case "skill":
      return (
        <div className="skill-preview compact">
          <div className="row">
            <b className="mono">/{p.name}</b>
            {p.update && <span className="pill warn">上書き</span>}
            {p.execution_mode === "subagent" && <span className="pill accent">Subagent</span>}
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
            <span className="muted">名前</span> <b className="mono">{p.name}</b>
          </div>
          <div>
            <span className="muted">接続</span> {p.transport === "stdio" ? <code>{[p.command, ...p.args].join(" ")}</code> : <code>{p.url}</code>}
          </div>
          {Object.keys(p.headers).length > 0 && <JsonView value={p.headers} />}
          {Object.keys(p.env).length > 0 && <JsonView value={p.env} />}
          <div className="note">登録しても、この会話のツールは変わりません。新しい会話から使えます。</div>
        </div>
      );
    case "error":
      return <div className="error-text">{p.error}</div>;
  }
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
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const { call, preview } = pending;
  const act = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="widget widget-approval">
      <header className="widget-head">
        <h4>{TITLE[call.name] ?? <><span className="tool-name">{call.name.replace("__", " · ")}</span> を実行してよいですか？</>}</h4>
        <span className="spacer" />
        <span className="pill warn">承認待ち</span>
      </header>
      <div className="widget-body">
        <p className="muted">{pending.reason}</p>
        {preview ? <Preview p={preview} /> : <JsonView value={call.arguments} />}
        <div className="wactions">
          <button className="btn primary" disabled={busy || preview?.kind === "error"} onClick={() => act(onApprove)}>
            承認して実行
          </button>
          <input className="reason" placeholder="キャンセルの理由（任意）" value={reason} onChange={(e) => setReason(e.target.value)} />
          <button className="btn" disabled={busy} onClick={() => act(() => onDeny(reason || "ユーザーがキャンセルしました"))}>
            キャンセル
          </button>
        </div>
      </div>
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
          MCP サーバー <span className="mono">{data.name}</span> を登録しました
        </h4>
        <span className="spacer" />
        {data.test ? data.test.ok ? <span className="pill ok">接続 OK</span> : <span className="pill danger">接続失敗</span> : null}
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
            この MCP を使って新しい会話を始める
          </button>
          <span className="muted">この会話のツールは変わりません</span>
        </div>
      </div>
    </section>
  );
}
