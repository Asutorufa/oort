import { useEffect, useRef } from "react";
import type { Live } from "../hooks/useSession";
import type { SessionDetail } from "../types";
import { MessageView } from "./Blocks";

/** Raw history with compaction markers, followed by the live (uncommitted) turn. */
export function Transcript({ detail, live }: { detail: SessionDetail; live: Live }) {
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => end.current?.scrollIntoView({ block: "end" }), [detail.raw.length, live.text.length]);
  const cutAt = new Map(detail.checkpoints.map((c) => [c.up_to_seq, c.epoch]));

  return (
    <div className="transcript">
      {detail.system_prompt && (
        <details className="block system">
          <summary>System prompt · {detail.tools.length} frozen tools</summary>
          <pre>{detail.system_prompt}</pre>
          <pre className="muted">{detail.tools.map((t) => t.name).join("\n")}</pre>
        </details>
      )}
      {detail.raw.map((m) => (
        <div key={m.seq}>
          {cutAt.has(m.seq) && <div className="epoch-marker">── epoch {cutAt.get(m.seq)} starts here (messages before #{m.seq} are summarized) ──</div>}
          <MessageView message={m} seq={m.seq} usage={m.usage} />
        </div>
      ))}
      {(live.text || live.thinking || live.runningTools.length > 0) && (
        <div className="msg assistant live">
          <div className="msg-head">
            <span className="role">assistant</span>
            <span className="badge">streaming</span>
          </div>
          {live.thinking && (
            <details className="block thinking" open>
              <summary>Thinking…</summary>
              <pre>{live.thinking}</pre>
            </details>
          )}
          {live.text && <div className="text">{live.text}▍</div>}
          {live.runningTools.map((t, i) => (
            <div key={i} className="muted">
              ⏳ running {t}
            </div>
          ))}
        </div>
      )}
      <div ref={end} />
    </div>
  );
}
