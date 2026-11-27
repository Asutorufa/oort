import { useCallback, useEffect, useState } from "react";
import { api } from "./api";
import { ApprovalCard, Composer } from "./components/Controls";
import { DebugPanel } from "./components/DebugPanel";
import { NewSessionForm, SessionList } from "./components/Sessions";
import { Transcript } from "./components/Transcript";
import { useSession } from "./hooks/useSession";
import type { SessionSummary } from "./types";

/** The selected session lives in the URL hash (`#/s/<id>`) so a reload restores it. */
function useHashSession(): [string | null, (id: string) => void] {
  const read = () => window.location.hash.match(/^#\/s\/(.+)$/)?.[1] ?? null;
  const [id, setId] = useState(read);
  useEffect(() => {
    const onHash = () => setId(read());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  return [id, (next) => (window.location.hash = `/s/${next}`)];
}

export function App() {
  const [selected, select] = useHashSession();
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  const { detail, live, log, connected, error: loadError } = useSession(selected);

  const reloadList = useCallback(() => api.listSessions().then(setSessions).catch((e) => setError(String(e))), []);
  useEffect(() => {
    reloadList();
    const t = window.setInterval(reloadList, 3000);
    return () => window.clearInterval(t);
  }, [reloadList]);

  return (
    <div className="layout">
      <aside className="left">
        <h2>oort</h2>
        <NewSessionForm onCreated={(id) => { select(id); reloadList(); }} onError={setError} />
        <SessionList sessions={sessions} selected={selected} onSelect={select} />
      </aside>
      <main>
        {(error || loadError) && (
          <div className="error-bar" onClick={() => setError(null)}>{error ?? loadError} (click to dismiss)</div>
        )}
        {detail ? (
          <>
            <Transcript detail={detail} live={live} />
            <ApprovalCard detail={detail} onError={setError} />
            <Composer detail={detail} onError={setError} />
          </>
        ) : (
          <div className="empty muted">{selected ? "Loading…" : "Select or create a session."}</div>
        )}
      </main>
      <aside className="right">{detail && <DebugPanel detail={detail} log={log} connected={connected} />}</aside>
    </div>
  );
}
