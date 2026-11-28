import { useState } from "react";
import type { ConnState, LogEntry } from "../../hooks/useSessionStream";
import { useT } from "../../i18n";
import { Collapse } from "../ui";

const clock = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23" });

function Line({ entry }: { entry: LogEntry }) {
  const [open, setOpen] = useState(false);
  const expandable = entry.data !== undefined || entry.text !== "";
  return (
    <li>
      <button type="button" className="insp-log-row" aria-expanded={open} disabled={!expandable} onClick={() => setOpen(!open)}>
        <span className="insp-log-seq">{entry.seq ?? "·"}</span>
        <span className="insp-log-time">{clock.format(entry.at)}</span>
        <span className={`insp-log-type ${entry.error ? "error" : ""}`}>{entry.type}</span>
        <span className="insp-log-text">{entry.text}</span>
      </button>
      <Collapse open={open}>
        <pre className="insp-payload insp-detail">{entry.data === undefined ? entry.text : JSON.stringify(entry.data, null, 2)}</pre>
      </Collapse>
    </li>
  );
}

/** Events received over SSE since the chat was opened, newest first. */
export function LogTab({ log, conn }: { log: LogEntry[]; conn: ConnState }) {
  const t = useT();
  return (
    <ol className="insp-scroll insp-log">
      {conn !== "open" && <li className="insp-empty">{conn === "reconnecting" ? t("status.reconnecting") : t("status.connecting")}</li>}
      {conn === "open" && log.length === 0 && <li className="insp-empty">{t("debug.noEvents")}</li>}
      {log
        .slice(-80)
        .reverse()
        .map((e) => (
          <Line key={e.id} entry={e} />
        ))}
    </ol>
  );
}
