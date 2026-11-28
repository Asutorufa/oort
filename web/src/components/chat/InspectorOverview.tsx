import type { SessionDetail, TurnUsage } from "../../api/types";
import { useI18n } from "../../i18n";

/** Run state, per-call tokens and cache, and past compactions. */
export function Overview({ detail, onInspect }: { detail: SessionDetail; onInspect: (t: TurnUsage) => void }) {
  const { t, fmt } = useI18n();
  const { usage } = detail;
  const mcp = detail.settings.mcp_servers.map((s) => s.prefix).join(", ");
  return (
    <>
      <section>
        <h4>{t("debug.state")}</h4>
        <dl className="insp-state">
          <div>
            <dt>{t("debug.phase")}</dt>
            <dd title={detail.phase.phase === "halted" ? detail.phase.reason : undefined}>
              {detail.phase.phase}
              {detail.phase.phase === "halted" && ` (${detail.phase.reason})`}
            </dd>
          </div>
          <div>
            <dt>{t("debug.turnsCalls")}</dt>
            <dd>
              {fmt.number(detail.turn)} / {fmt.number(usage.ledger.llm_calls)}
            </dd>
          </div>
          <div>
            <dt>{t("debug.tools")}</dt>
            <dd title={detail.tools.map((x) => x.name).join("\n")}>
              {fmt.number(detail.tools.length)}
              {mcp && ` · MCP: ${mcp}`}
            </dd>
          </div>
          <div>
            <dt>{t("debug.queued")}</dt>
            <dd>{fmt.number(detail.pending_steers)}</dd>
          </div>
        </dl>
      </section>

      <section>
        <div className="insp-section-head">
          <h4>{t("debug.tokens")}</h4>
          <span title={t("debug.inOut")}>
            {fmt.number(usage.ledger.total_input_tokens)} / {fmt.number(usage.ledger.total_output_tokens)}
          </span>
        </div>
        {usage.per_turn.length === 0 ? (
          <div className="insp-empty">{t("debug.noCalls")}</div>
        ) : (
          <table className="insp-table">
            <thead>
              <tr>
                <th>{t("debug.col.turn")}</th>
                <th>{t("debug.col.prompt")}</th>
                <th>{t("debug.col.cached")}</th>
                <th>{t("debug.col.hit")}</th>
                <th>{t("debug.col.out")}</th>
                <th>{t("debug.col.ttft")}</th>
                <th>{t("debug.col.total")}</th>
              </tr>
            </thead>
            <tbody>
              {usage.per_turn.map((x) => (
                <tr key={x.seq} tabIndex={0} onClick={() => onInspect(x)} onKeyDown={(e) => e.key === "Enter" && onInspect(x)} title={t("debug.inspectTurn")}>
                  <td>{x.turn}</td>
                  <td>{fmt.number(x.prompt_tokens)}</td>
                  <td>{fmt.number(x.cache_read_tokens)}</td>
                  <td className={x.cache_hit_rate > 0.5 ? "high" : "low"}>{fmt.percent(x.cache_hit_rate)}</td>
                  <td>{fmt.number(x.output_tokens)}</td>
                  <td>{fmt.ms(x.ttft_ms)}</td>
                  <td>{fmt.ms(x.latency_ms)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      {detail.checkpoints.length > 0 && (
        <section>
          <h4>{t("debug.summary")}</h4>
          <ul className="insp-compactions">
            {detail.checkpoints.map((c) => (
              <li key={c.epoch}>
                <details>
                  <summary>{t("debug.epoch", { epoch: c.epoch, last: c.up_to_seq - 1 })}</summary>
                  <pre className="insp-payload">{c.summary.content.map((p) => (p.kind === "text" ? p.text : "")).join("")}</pre>
                </details>
              </li>
            ))}
          </ul>
        </section>
      )}
    </>
  );
}
