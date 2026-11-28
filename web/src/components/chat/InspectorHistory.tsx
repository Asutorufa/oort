import { useState } from "react";
import type { ContentPart, Message, SessionDetail } from "../../api/types";
import { useT, type T } from "../../i18n";
import { Collapse, SegmentedControl } from "../ui";
import { imagesOf } from "./MessageImages";

const ROLE: Record<Message["role"], string> = { system: "sys", user: "user", assistant: "asst", tool: "tool" };

/** One block of a message, in full. */
function blockText(p: ContentPart): string {
  switch (p.kind) {
    case "text":
      return p.text;
    case "thinking":
      return p.thinking;
    case "tool_call":
      return `${p.name}  ${p.id}\n${JSON.stringify(p.arguments, null, 2)}`;
    case "tool_result":
      return `${p.name ?? ""}  ${p.tool_call_id}${p.is_error ? "  error" : ""}\n${p.content}`;
    case "image":
      return p.source.type === "attachment"
        ? [p.source.filename, p.source.width && p.source.height ? `${p.source.width}×${p.source.height}` : "", p.source.media_type, p.source.attachment_id].filter(Boolean).join("  ")
        : p.source.type;
    default:
      return JSON.stringify(p, null, 2);
  }
}

/** The one-line summary: what the message says, or which tools it is about. */
function RowSummary({ m, t }: { m: Message; t: T }) {
  const parts = m.content;
  const results = parts.filter((p) => p.kind === "tool_result");
  const text = parts.find((p) => p.kind === "text" && p.text.trim());
  const calls = parts.flatMap((p) => (p.kind === "tool_call" || p.kind === "server_tool_use" ? [p.name] : []));
  const thought = parts.some((p) => p.kind === "thinking");
  const images = imagesOf(parts);
  // The image chip already stands for all the images.
  const rest = parts.length - Math.max(images.length, 1);
  const extra = rest > 0 && !(images.length && rest === 1 && text) ? <span className="insp-extra">+{rest}</span> : null;
  const only = images.length === 1 ? images[0] : null;
  const imageChip = images.length ? (
    <span className="insp-chip">
      {only
        ? only.width && only.height
          ? t("debug.image", { size: `${only.width}×${only.height}` })
          : t("debug.images", { n: 1 })
        : t("debug.images", { n: images.length })}
    </span>
  ) : null;
  if (results.length) {
    const failed = results.some((p) => p.is_error);
    return (
      <>
        <span className="insp-tool">{results.map((p) => p.name ?? p.tool_call_id).join(", ")}</span>
        <span className={`insp-dot ${failed ? "error" : "ok"}`} role="img" aria-label={failed ? t("common.error") : "ok"} />
      </>
    );
  }
  return (
    <>
      <span className="insp-main">
        {thought && <span className="insp-chip">Think</span>}
        {imageChip}
        {text?.kind === "text" ? (
          <span className="insp-text">{text.text.slice(0, 200)}</span>
        ) : calls.length ? (
          <span className="insp-tool">{calls.join(", ")}</span>
        ) : (
          !thought && !images.length && <span className="insp-tool">{parts[0]?.kind ?? t("common.empty")}</span>
        )}
      </span>
      {extra}
    </>
  );
}

function Row({ label, labelTitle, message, dimmed }: { label: string; labelTitle?: string; message: Message; dimmed?: boolean }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  return (
    <li className={dimmed ? "dimmed" : ""}>
      <button type="button" className="insp-row" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span className="insp-seq" title={labelTitle}>
          {label}
        </span>
        <span className={`insp-role ${ROLE[message.role]}`}>{ROLE[message.role]}</span>
        <span className="insp-row-body">
          <RowSummary m={message} t={t} />
        </span>
      </button>
      <Collapse open={open}>
        <div className="insp-payload insp-detail">
          {message.content.map((p, i) => (
            <div key={i}>
              <div className="insp-kind">{p.kind}</div>
              <pre>{blockText(p)}</pre>
            </div>
          ))}
        </div>
      </Collapse>
    </li>
  );
}

/** Raw history, or only what the next request sends. */
export function HistoryTab({ detail }: { detail: SessionDetail }) {
  const t = useT();
  const [view, setView] = useState<"raw" | "active">("active");
  const { from_seq, messages } = detail.active_view;
  const hasSummary = detail.checkpoints.length > 0;
  return (
    <>
      <div className="insp-toolbar">
        <SegmentedControl
          size="sm"
          className="insp-seg"
          ariaLabel={t("debug.historyView")}
          value={view}
          onChange={setView}
          options={[
            { value: "raw", label: t("debug.rawAll", { n: detail.raw.length }) },
            { value: "active", label: t("debug.active", { n: messages.length }) },
          ]}
        />
      </div>
      <ol className="insp-scroll insp-history">
        {view === "raw"
          ? detail.raw.map((m) => <Row key={`raw-${m.seq}`} label={`#${m.seq}`} message={m} dimmed={m.seq < from_seq} />)
          : messages.map((m, i) => {
              // After a compaction the first message is the summary standing in for older ones.
              if (hasSummary && i === 0) return <Row key="summary" label="Σ" labelTitle={t("debug.summary")} message={m} />;
              const seq = from_seq + i - (hasSummary ? 1 : 0);
              return <Row key={`active-${seq}`} label={`#${seq}`} message={m} />;
            })}
        {(view === "raw" ? detail.raw : messages).length === 0 && <li className="insp-empty">{t("debug.noMessages")}</li>}
      </ol>
    </>
  );
}
