import type { TurnUsage } from "../../api/types";
import { Badge, fmtMs, Modal, pct } from "../common";

function Hash({ label, now, prev }: { label: string; now: string; prev?: string }) {
  const changed = prev !== undefined && prev !== now;
  return (
    <tr>
      <td>{label}</td>
      <td>
        <code>{now}</code>{" "}
        {prev === undefined ? (
          <span className="muted">first call</span>
        ) : changed ? (
          <Badge tone={label === "messages" ? "muted" : "error"} title="differs from the previous call">
            changed
          </Badge>
        ) : (
          <Badge tone="ok">same as previous</Badge>
        )}
      </td>
    </tr>
  );
}

/** Per-call debug info: latency, tokens, cache and what the request looked like. */
export function TurnInspector({ turn, prev, onClose }: { turn: TurnUsage; prev?: TurnUsage; onClose: () => void }) {
  const req = turn.request;
  const prefixBroken =
    req && prev?.request && (req.hash.system !== prev.request.hash.system || req.hash.tools !== prev.request.hash.tools);
  return (
    <Modal title={`Turn ${turn.turn} · message #${turn.seq}`} onClose={onClose}>
      <div className="stats">
        <div className="stat">
          <span className="muted">TTFT</span>
          <b>{fmtMs(turn.ttft_ms)}</b>
        </div>
        <div className="stat">
          <span className="muted">total</span>
          <b>{fmtMs(turn.latency_ms)}</b>
        </div>
        <div className="stat">
          <span className="muted">cache hit</span>
          <b className={turn.cache_hit_rate > 0.5 ? "good" : "bad"}>{pct(turn.cache_hit_rate)}</b>
        </div>
      </div>
      <table className="kv">
        <tbody>
          <tr>
            <td>prompt tokens</td>
            <td>{turn.prompt_tokens}</td>
          </tr>
          <tr>
            <td>input (as reported)</td>
            <td>{turn.input_tokens}</td>
          </tr>
          <tr>
            <td>cache read / write</td>
            <td>
              {turn.cache_read_tokens} / {turn.cache_creation_tokens}
            </td>
          </tr>
          <tr>
            <td>output tokens</td>
            <td>{turn.output_tokens}</td>
          </tr>
        </tbody>
      </table>
      <h4>Request</h4>
      {req ? (
        <>
          {prefixBroken && (
            <div className="warning">The system prompt or tool catalog changed since the previous call: the cached prefix cannot be reused.</div>
          )}
          <table className="kv">
            <tbody>
              <tr>
                <td>model</td>
                <td>{req.model ?? "(client default)"}</td>
              </tr>
              <tr>
                <td>messages / tools</td>
                <td>
                  {req.messages} / {req.tools}
                </td>
              </tr>
              <tr>
                <td>size</td>
                <td>{(req.bytes / 1024).toFixed(1)} KiB</td>
              </tr>
              <tr>
                <td>max_tokens</td>
                <td>{req.max_tokens ?? "—"}</td>
              </tr>
              <tr>
                <td>cache breakpoints</td>
                <td>
                  {req.breakpoints.map((b, i) => (
                    <code key={i} className="chip">
                      {b.type === "after_message" ? `after #${b.index}` : "tools+system"}
                    </code>
                  ))}
                </td>
              </tr>
              <Hash label="system" now={req.hash.system} prev={prev?.request?.hash.system} />
              <Hash label="tools" now={req.hash.tools} prev={prev?.request?.hash.tools} />
              <Hash label="messages" now={req.hash.messages} prev={prev?.request?.hash.messages} />
            </tbody>
          </table>
        </>
      ) : (
        <div className="muted">No request snapshot for this call (recorded before metrics existed).</div>
      )}
    </Modal>
  );
}
