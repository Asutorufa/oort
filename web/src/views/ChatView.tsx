import { useEffect, useMemo, useState } from "react";
import { commands as commandsApi, errorText, sessions } from "../api";
import { useToast } from "../components/common/Toast";
import type { ApprovalPolicy, InputRequest, TurnUsage } from "../api/types";
import { Composer } from "../components/chat/Composer";
import { Inspector } from "../components/chat/Inspector";
import { lastPlan, PlanPill } from "../components/chat/PlanTracker";
import { Thread } from "../components/chat/Thread";
import { TurnInspector } from "../components/chat/TurnInspector";
import { Welcome } from "../components/chat/Welcome";
import { Empty, ErrorBar } from "../components/common";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";
import { useSessionStream } from "../hooks/useSessionStream";
import { formatResponse, type WidgetActions } from "../widgets/protocol";

/** Japanese text for the command results the server reports in English. */
const COMMAND_MESSAGES: Record<string, string> = {
  "Compaction requested": "会話の要約を始めました",
  "Approval: run every tool automatically": "承認ポリシー：すべて自動実行",
  "Approval: read-only tools run automatically": "承認ポリシー：読み取りのみ自動",
  "Approval: ask every time": "承認ポリシー：毎回確認",
  Stopped: "応答を停止しました",
  "Nothing was running": "実行中の応答はありません",
  "Export ready": "書き出しを開始しました",
  "New chat started": "新しい会話を始めました",
};
const commandMessage = (m: string) => COMMAND_MESSAGES[m.replace(/^✓\s*/, "")] ?? m;
const commandError = (m: string) =>
  /^\/approval takes/.test(m) ? "/approval の引数は auto・readonly・ask のいずれかです" : /needs an open conversation/.test(m) ? "このコマンドは会話を開いているときに使えます" : m;

const STATUS: Record<string, string> = { running: "応答中", needs_approval: "承認待ち", halted: "停止", idle: "" };

export function ChatView({ selected, onListChanged }: { selected: string | null; onListChanged: () => void }) {
  if (!selected || selected === "new") {
    return (
      <Welcome
        onCreated={(id) => {
          onListChanged();
          navigate("chat", id);
        }}
      />
    );
  }
  return <Session key={selected} id={selected} onListChanged={onListChanged} />;
}

function Session({ id, onListChanged }: { id: string; onListChanged: () => void }) {
  const { detail, live, log, conn, error: loadError, refresh, toolTimes, subagents, plan } = useSessionStream(id, () => {
    onListChanged();
    navigate("chat");
  });
  const commandList = useResource(commandsApi.list);
  const childList = useResource(() => sessions.children(id));
  const toast = useToast();
  const [trace, setTrace] = useState<string | null>(null);
  // Children appear as subagents start; refresh their list when one is new.
  const knownChildren = Object.values(subagents).map((s) => s.subSessionId).join(",");
  const reloadChildren = childList.reload;
  useEffect(() => {
    reloadChildren();
  }, [knownChildren, detail?.raw.length, reloadChildren]);
  const [error, setError] = useState<string | null>(null);
  // Pick up the automatic retry when it fires (its events start a new loop on the server).
  const retryAt = detail?.last_error?.retry_at ?? null;
  useEffect(() => {
    if (retryAt == null) return;
    const t = window.setTimeout(refresh, Math.max(0, retryAt - Date.now()) + 800);
    return () => window.clearTimeout(t);
  }, [retryAt, refresh]);
  const [inspector, setInspector] = useState(false);
  const [inspect, setInspect] = useState<TurnUsage | null>(null);
  const [title, setTitle] = useState<string | null>(null);

  const send = async (input: InputRequest) => {
    try {
      await sessions.input(id, input);
      setError(null);
    } catch (e) {
      setError(errorText(e));
      throw e;
    }
  };
  const actions: WidgetActions = useMemo(
    () => ({
      respond: (callId, type, payload) => send({ type: "steer", text: formatResponse(callId, type, payload) }),
      prompt: (text) => send({ type: "steer", text }),
      compact: () => send({ type: "compact" }),
      approve: (callId) => send({ type: "approval", call_id: callId, decision: "approve" }),
      deny: (callId, reason) => send({ type: "approval", call_id: callId, decision: "deny", reason }),
    }),
    [id], // eslint-disable-line react-hooks/exhaustive-deps
  );
  const patch = async (req: { title?: string; approval_policy?: ApprovalPolicy }) => {
    try {
      await sessions.patch(id, req);
      refresh();
      onListChanged();
    } catch (e) {
      setError(errorText(e));
    }
  };

  const runCommand = async (name: string, args: string) => {
    try {
      const r = await commandsApi.run(name, id, args);
      toast(commandMessage(r.message));
      if (r.effect.download) window.location.href = r.effect.download;
      if (r.effect.navigate_session) {
        onListChanged();
        navigate("chat", r.effect.navigate_session);
      }
      refresh();
    } catch (e) {
      toast(commandError(errorText(e)), "error");
      throw e;
    }
  };

  /** A new chat like this one, plus the MCP server just registered. */
  const startWithMcp = async (serverId: string) => {
    if (!detail) return;
    try {
      const d = await sessions.create({
        provider_id: detail.settings.provider_id,
        model: detail.settings.model,
        mcp_server_ids: [...detail.settings.mcp_servers.map((b) => b.id), serverId],
        builtin_groups: detail.lineage.builtin_groups,
        workspace_root: detail.lineage.workspace_root ?? undefined,
        approval_policy: detail.approval_policy,
        title: `${detail.title || "新しい会話"}（MCP 追加）`,
      });
      onListChanged();
      navigate("chat", d.id);
    } catch (e) {
      setError(errorText(e));
    }
  };

  if (!detail) return <Empty>{loadError ?? "読み込み中…"}</Empty>;
  const turns = detail.usage.per_turn;
  const prevTurn = inspect ? turns[turns.findIndex((t) => t.seq === inspect.seq) - 1] : undefined;
  const failed = !!detail.last_error;
  const busy = detail.status === "running" && !failed;
  const status = failed ? (detail.last_error?.retry_at ? "再試行待ち" : "エラー") : STATUS[detail.status];
  const planSteps = plan ?? lastPlan(detail.raw);

  return (
    <div className={`chat ${inspector ? "with-inspector" : ""}`}>
      <div className="chat-main">
        <header className="chat-top">
          <input
            className="chat-title"
            value={title ?? detail.title}
            placeholder="無題の会話"
            onChange={(e) => setTitle(e.target.value)}
            onBlur={() => {
              if (title !== null && title !== detail.title) patch({ title });
              setTitle(null);
            }}
            onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
          />
          {status && (
            <span className={`status-chip ${failed ? "failed" : detail.status}`}>
              <span className="status-dot" />
              {status}
            </span>
          )}
          {planSteps && planSteps.length > 0 && <PlanPill steps={planSteps} />}
          <span className="spacer" />
          <span className="model-chip" title="この会話のモデル">
            {detail.model}
          </span>
          {conn !== "open" && <span className="pill warn">{conn === "reconnecting" ? "再接続中…" : "接続中…"}</span>}
          <button className={`btn ghost ${inspector ? "on" : ""}`} onClick={() => setInspector(!inspector)} title="キャッシュ・時間・内部状態">
            透視
          </button>
        </header>
        <ErrorBar error={error} onDismiss={() => setError(null)} />
        <Thread
          detail={detail}
          live={live}
          toolTimes={toolTimes}
          actions={actions}
          subagents={subagents}
          children={childList.data ?? []}
          onTrace={(sid) => {
            setTrace(sid);
            setInspector(true);
          }}
          onAnswer={(callId, content) => send({ type: "approval", call_id: callId, decision: "answer", content })}
          onStartWithMcp={startWithMcp}
          onRetry={async () => {
            try {
              await sessions.retry(id);
              setError(null);
              refresh();
            } catch (e) {
              setError(errorText(e));
            }
          }}
          onCancelAutoRetry={async () => {
            await sessions.cancel(id).catch((e) => setError(errorText(e)));
            refresh();
          }}
        />
        <Composer
          busy={busy}
          disabled={detail.status === "halted"}
          commands={commandList.data ?? []}
          policy={detail.approval_policy}
          onPolicy={(p) => patch({ approval_policy: p })}
          onSend={(text, skills) => send({ type: "steer", text, invoked_skills: skills })}
          onInterrupt={(text, skills) => send({ type: "interrupt", text, invoked_skills: skills })}
          onCommand={runCommand}
          onStop={() => sessions.cancel(id).catch((e) => setError(errorText(e)))}
        />
      </div>
      {inspector && (
        <Inspector
          trace={trace}
          onCloseTrace={() => setTrace(null)}
          detail={detail}
          log={log}
          onInspect={setInspect}
          onCompact={() => send({ type: "compact" }).catch(() => {})}
          onStop={() => sessions.cancel(id).catch((e) => setError(errorText(e)))}
          onClose={() => {
            setInspector(false);
            setTrace(null);
          }}
        />
      )}
      {inspect && <TurnInspector turn={inspect} prev={prevTurn} onClose={() => setInspect(null)} />}
    </div>
  );
}
