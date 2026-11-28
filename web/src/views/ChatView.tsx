import { useMemo, useState } from "react";
import { errorText, sessions, skills as skillsApi } from "../api";
import type { ApprovalPolicy, InputRequest, TurnUsage } from "../api/types";
import { Composer } from "../components/chat/Composer";
import { Inspector } from "../components/chat/Inspector";
import { Thread } from "../components/chat/Thread";
import { TurnInspector } from "../components/chat/TurnInspector";
import { Welcome } from "../components/chat/Welcome";
import { Empty, ErrorBar } from "../components/common";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";
import { useSessionStream } from "../hooks/useSessionStream";
import { formatResponse, type WidgetActions } from "../widgets/protocol";

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
  const { detail, live, log, conn, error: loadError, refresh, toolTimes } = useSessionStream(id, () => {
    onListChanged();
    navigate("chat");
  });
  const skillList = useResource(skillsApi.list);
  const [error, setError] = useState<string | null>(null);
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

  if (!detail) return <Empty>{loadError ?? "読み込み中…"}</Empty>;
  const turns = detail.usage.per_turn;
  const prevTurn = inspect ? turns[turns.findIndex((t) => t.seq === inspect.seq) - 1] : undefined;
  const busy = detail.status === "running";
  const status = STATUS[detail.status];

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
          {status && <span className={`pill ${detail.status === "needs_approval" ? "warn" : detail.status === "halted" ? "danger" : "accent"}`}>{status}</span>}
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
        <Thread detail={detail} live={live} toolTimes={toolTimes} actions={actions} />
        <Composer
          busy={busy}
          disabled={detail.status === "halted"}
          skills={skillList.data ?? []}
          policy={detail.approval_policy}
          onPolicy={(p) => patch({ approval_policy: p })}
          onSend={(text) => send({ type: "steer", text })}
          onInterrupt={(text) => send({ type: "interrupt", text })}
          onStop={() => sessions.cancel(id).catch((e) => setError(errorText(e)))}
        />
      </div>
      {inspector && (
        <Inspector
          detail={detail}
          log={log}
          onInspect={setInspect}
          onCompact={() => send({ type: "compact" }).catch(() => {})}
          onStop={() => sessions.cancel(id).catch((e) => setError(errorText(e)))}
          onClose={() => setInspector(false)}
        />
      )}
      {inspect && <TurnInspector turn={inspect} prev={prevTurn} onClose={() => setInspect(null)} />}
    </div>
  );
}
