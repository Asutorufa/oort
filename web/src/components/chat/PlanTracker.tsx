import { useState } from "react";
import type { PlanStep } from "../../api/types";

const ICON: Record<PlanStep["status"], string> = { done: "✓", in_progress: "●", pending: "○", skipped: "–" };

/** The agent's plan as a compact checklist with progress (sticky at the top of the thread). */
export function PlanTracker({ steps }: { steps: PlanStep[] }) {
  const [open, setOpen] = useState(true);
  const done = steps.filter((s) => s.status === "done" || s.status === "skipped").length;
  const current = steps.find((s) => s.status === "in_progress");
  return (
    <div className={`plan ${open ? "open" : ""}`}>
      <button className="plan-head" onClick={() => setOpen(!open)}>
        <span className="plan-title">計画</span>
        <span className="plan-bar">
          <span style={{ width: `${(done / Math.max(steps.length, 1)) * 100}%` }} />
        </span>
        <span className="muted">
          {done}/{steps.length}
        </span>
        {!open && current && <span className="plan-current ellipsis">{current.title}</span>}
        <span className="chev">⌄</span>
      </button>
      {open && (
        <ol className="plan-steps">
          {steps.map((s) => (
            <li key={s.id} className={s.status}>
              <span className="plan-icon">{ICON[s.status]}</span>
              <span>{s.title}</span>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}
