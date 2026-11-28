import { useState } from "react";
import type { WidgetProps } from "./protocol";
import { WidgetCard } from "./WidgetCard";
import { DecisionFooter } from "../components/chat/Decision";

/** Unified diff with added (green) and removed (red) lines. */
export function DiffView({ diff }: { diff: string }) {
  return (
    <pre className="diff">
      {diff.split("\n").map((line, i) => {
        const cls = line.startsWith("+++") || line.startsWith("---") ? "meta" : line.startsWith("@@") ? "hunk" : line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : "";
        return (
          <span key={i} className={`dl ${cls}`}>
            {line || " "}
            {"\n"}
          </span>
        );
      })}
    </pre>
  );
}

const count = (diff: string) => {
  let add = 0;
  let del = 0;
  for (const l of diff.split("\n")) {
    if (l.startsWith("+") && !l.startsWith("+++")) add++;
    else if (l.startsWith("-") && !l.startsWith("---")) del++;
  }
  return { add, del };
};

/** The approval flow as a card: decisions go through the regular Approval input. */
export function ApprovalDiff({ id, spec, pendingApproval, result, actions, disabled }: WidgetProps) {
  const diff = String(spec.data.diff ?? "");
  const files = (spec.data.files as string[]) ?? [];
  const [open, setOpen] = useState(true);
  const { add, del } = count(diff);
  const status = result ? (result.is_error ? <span className="pill danger">却下しました</span> : <span className="pill ok">承認しました</span>) : pendingApproval ? <span className="pill warn">承認待ち</span> : null;
  return (
    <WidgetCard title={spec.title ?? "変更の確認"} kind="approval" status={status}>
      <p className="approval-summary">{String(spec.data.summary ?? "")}</p>
      <div className="diff-meta">
        {files.map((f) => (
          <code key={f} className="chip">
            {f}
          </code>
        ))}
        <span className="add-count">+{add}</span>
        <span className="del-count">−{del}</span>
        <button className="text-btn" onClick={() => setOpen(!open)}>
          {open ? "差分を隠す" : "差分を表示"}
        </button>
      </div>
      {open && <DiffView diff={diff} />}
      {pendingApproval && !result && (
        <DecisionFooter disabled={disabled} onApprove={() => actions.approve(id)} onDeny={(reason) => actions.deny(id, reason)} />
      )}
    </WidgetCard>
  );
}
