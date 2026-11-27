import { useEffect, useState } from "react";
import { errorText, sessions } from "../api";
import type { SessionSummary, TurnUsage } from "../api/types";
import { ChatHeader } from "../components/chat/ChatHeader";
import { ApprovalCard, Composer } from "../components/chat/Controls";
import { DebugPanel } from "../components/chat/DebugPanel";
import { NewSessionDialog } from "../components/chat/NewSession";
import { SessionList } from "../components/chat/SessionList";
import { Transcript } from "../components/chat/Transcript";
import { TurnInspector } from "../components/chat/TurnInspector";
import { Empty, ErrorBar, Modal } from "../components/common";
import { navigate } from "../hooks/useHashRoute";
import { useResource } from "../hooks/useResource";
import { useSessionStream } from "../hooks/useSessionStream";

export function ChatView({ selected }: { selected: string | null }) {
  const list = useResource(sessions.list, 3000);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(selected === "new");
  const [confirmDelete, setConfirmDelete] = useState<SessionSummary | { id: string; title: string } | null>(null);
  const [inspect, setInspect] = useState<TurnUsage | null>(null);
  const sessionId = selected === "new" ? null : selected;
  const { detail, live, log, conn, error: loadError, refresh, toolTimes } = useSessionStream(sessionId, () => {
    list.reload();
    navigate("chat");
  });

  // Keep the list's status badges in step with the open session.
  const status = detail?.status;
  const reloadList = list.reload;
  useEffect(() => {
    if (status) reloadList();
  }, [status, reloadList]);

  const remove = async (id: string) => {
    setConfirmDelete(null);
    try {
      await sessions.remove(id);
      if (id === sessionId) navigate("chat");
      list.reload();
    } catch (e) {
      setError(errorText(e));
    }
  };
  const turns = detail?.usage.per_turn ?? [];
  const prevTurn = inspect ? turns[turns.findIndex((t) => t.seq === inspect.seq) - 1] : undefined;

  return (
    <div className="chat-layout">
      <aside className="pane left">
        <div className="pane-head">
          <h3>Sessions</h3>
          <button className="primary small" onClick={() => setCreating(true)}>
            + New
          </button>
        </div>
        <SessionList
          items={list.data ?? []}
          selected={sessionId}
          onSelect={(id) => navigate("chat", id)}
          onChanged={() => {
            list.reload();
            refresh();
          }}
          onDelete={setConfirmDelete}
          onError={setError}
        />
      </aside>

      <main className="chat-main">
        <ErrorBar error={error ?? (sessionId ? loadError : null) ?? list.error} onDismiss={() => setError(null)} />
        {detail ? (
          <>
            <ChatHeader
              detail={detail}
              conn={conn}
              onChanged={() => {
                refresh();
                list.reload();
              }}
              onDelete={() => setConfirmDelete({ id: detail.id, title: detail.title })}
              onError={setError}
            />
            <Transcript detail={detail} live={live} toolTimes={toolTimes} onInspect={setInspect} />
            <ApprovalCard detail={detail} onError={setError} />
            <Composer detail={detail} onError={setError} />
          </>
        ) : (
          <Empty>
            {sessionId ? (
              "Loading…"
            ) : (
              <>
                Select a session or <button onClick={() => setCreating(true)}>start a new one</button>.
              </>
            )}
          </Empty>
        )}
      </main>

      <aside className="pane right">{detail && <DebugPanel detail={detail} log={log} onInspect={setInspect} />}</aside>

      {creating && (
        <NewSessionDialog
          onClose={() => setCreating(false)}
          onCreated={(id) => {
            setCreating(false);
            list.reload();
            navigate("chat", id);
          }}
        />
      )}
      {inspect && <TurnInspector turn={inspect} prev={prevTurn} onClose={() => setInspect(null)} />}
      {confirmDelete && (
        <Modal title="Delete session?" onClose={() => setConfirmDelete(null)}>
          <p>
            <b>{confirmDelete.title || confirmDelete.id}</b> and its whole history will be deleted. This cannot be undone (export it first if
            you need a trace).
          </p>
          <div className="row">
            <span className="spacer" />
            <button onClick={() => setConfirmDelete(null)}>Cancel</button>
            <button className="danger" onClick={() => remove(confirmDelete.id)}>
              Delete
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
