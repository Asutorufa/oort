import type { TurnUsage } from "../../api/types";
import { Badge, Modal } from "../common";
import { useI18n, useT } from "../../i18n";

function Hash({ label, now, prev }: { label: string; now: string; prev?: string }) {
  const t = useT();
  const changed = prev !== undefined && prev !== now;
  return (
    <tr>
      <td>{label}</td>
      <td>
        <code>{now}</code>{" "}
        {prev === undefined ? (
          <span className="muted">{t("turn.firstCall")}</span>
        ) : changed ? (
          <Badge tone={label === "messages" ? "muted" : "error"} title={t("turn.changedHint")}>
            {t("turn.changed")}
          </Badge>
        ) : (
          <Badge tone="ok">{t("turn.same")}</Badge>
        )}
      </td>
    </tr>
  );
}

/** Per-call debug info: latency, tokens, cache and what the request looked like. */
export function TurnInspector({ turn, prev, onClose }: { turn: TurnUsage; prev?: TurnUsage; onClose: () => void }) {
  const req = turn.request;
  const { t, fmt } = useI18n();
  const prefixBroken =
    req && prev?.request && (req.hash.system !== prev.request.hash.system || req.hash.tools !== prev.request.hash.tools);
  return (
    <Modal title={t("turn.title", { turn: turn.turn, seq: turn.seq })} onClose={onClose}>
      <div className="stats">
        <div className="stat">
          <span className="muted">{t("insp.ttft")}</span>
          <b>{fmt.ms(turn.ttft_ms)}</b>
        </div>
        <div className="stat">
          <span className="muted">{t("insp.total")}</span>
          <b>{fmt.ms(turn.latency_ms)}</b>
        </div>
        <div className="stat">
          <span className="muted">{t("turn.cacheHit")}</span>
          <b className={turn.cache_hit_rate > 0.5 ? "good" : "bad"}>{fmt.percent(turn.cache_hit_rate)}</b>
        </div>
      </div>
      <table className="kv">
        <tbody>
          <tr>
            <td>{t("turn.promptTokens")}</td>
            <td>{fmt.number(turn.prompt_tokens)}</td>
          </tr>
          <tr>
            <td>{t("turn.inputReported")}</td>
            <td>{fmt.number(turn.input_tokens)}</td>
          </tr>
          <tr>
            <td>{t("turn.cacheRW")}</td>
            <td>
              {fmt.number(turn.cache_read_tokens)} / {fmt.number(turn.cache_creation_tokens)}
            </td>
          </tr>
          <tr>
            <td>{t("turn.outputTokens")}</td>
            <td>{fmt.number(turn.output_tokens)}</td>
          </tr>
        </tbody>
      </table>
      <h4>{t("turn.request")}</h4>
      {req ? (
        <>
          {prefixBroken && (
            <div className="warning">{t("turn.prefixBroken")}</div>
          )}
          <table className="kv">
            <tbody>
              <tr>
                <td>{t("turn.model")}</td>
                <td>{req.model ?? t("turn.clientDefault")}</td>
              </tr>
              <tr>
                <td>{t("turn.messagesTools")}</td>
                <td>
                  {req.messages} / {req.tools}
                </td>
              </tr>
              <tr>
                <td>{t("turn.size")}</td>
                <td>{fmt.number(req.bytes / 1024, { maximumFractionDigits: 1 })} KiB</td>
              </tr>
              <tr>
                <td>max_tokens</td>
                <td>{req.max_tokens ?? "—"}</td>
              </tr>
              <tr>
                <td>{t("turn.breakpoints")}</td>
                <td>
                  {req.breakpoints.map((b, i) => (
                    <code key={i} className="chip">
                      {b.type === "after_message" ? t("turn.after", { index: b.index ?? 0 }) : "tools+system"}
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
        <div className="muted">{t("turn.noSnapshot")}</div>
      )}
    </Modal>
  );
}
