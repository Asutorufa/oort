import { useEffect, useRef, useState } from "react";
import type { PlanStep, RawMessage } from "../../api/types";
import { AnimatePresence, motion } from "motion/react";
import { CheckIcon, ChevronDown } from "../../brand/Icons";
import { popover } from "../../lib/motion";
import { useT } from "../../i18n";

/** The latest `update_plan` steps in the history (the plan survives reloads this way). */
export function lastPlan(raw: RawMessage[]): PlanStep[] | null {
  for (let i = raw.length - 1; i >= 0; i--) {
    for (const p of [...raw[i].content].reverse()) {
      if (p.kind === "tool_call" && p.name === "update_plan") {
        const steps = (p.arguments as { steps?: PlanStep[] } | null)?.steps;
        if (Array.isArray(steps)) return steps;
      }
    }
  }
  return null;
}

function StepIcon({ status }: { status: PlanStep["status"] }) {
  if (status === "done") return <CheckIcon size={13} stroke={2} />;
  if (status === "in_progress") return <span className="plan-live" />;
  if (status === "skipped") return <span className="plan-skip">–</span>;
  return <span className="plan-todo" />;
}

/**
 * The agent's plan as a one-line pill in the chat header (`● 計画 1/3 · 質問する ⌄`). The full
 * checklist opens as a dropdown below the header and never covers the conversation otherwise.
 */
export function PlanPill({ steps }: { steps: PlanStep[] }) {
  const [open, setOpen] = useState(false);
  const t = useT();
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !box.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  const done = steps.filter((s) => s.status === "done" || s.status === "skipped").length;
  const current = steps.find((s) => s.status === "in_progress") ?? steps.find((s) => s.status === "pending");
  const finished = done === steps.length;
  return (
    <div className="plan" ref={box}>
      <button className={`plan-pill ${open ? "open" : ""}`} onClick={() => setOpen(!open)} aria-expanded={open} title={t("plan.show")}>
        {finished ? <CheckIcon size={12} stroke={2.2} /> : <span className="plan-live" />}
        <span className="plan-title">{t("plan.title")}</span>
        <span className="plan-count">
          {done}/{steps.length}
        </span>
        {!finished && current && <span className="plan-current">· {current.title}</span>}
        <ChevronDown size={13} />
      </button>
      <AnimatePresence>
      {open && (
        <motion.div className="plan-pop" role="dialog" aria-label={t("plan.title")} variants={popover("top")} initial="hidden" animate="shown" exit="exit" style={{ transformOrigin: "top left" }}>
          <div className="plan-pop-head">
            <span>{t("plan.title")}</span>
            <span className="plan-bar">
              <span style={{ width: `${(done / Math.max(steps.length, 1)) * 100}%` }} />
            </span>
            <span className="plan-count">
              {done}/{steps.length}
            </span>
          </div>
          <ol className="plan-steps">
            {steps.map((s) => (
              <li key={s.id} className={s.status}>
                <span className="plan-icon">
                  <StepIcon status={s.status} />
                </span>
                <span>{s.title}</span>
              </li>
            ))}
          </ol>
        </motion.div>
      )}
      </AnimatePresence>
    </div>
  );
}
