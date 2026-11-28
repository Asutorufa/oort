import { useState } from "react";
import type { WidgetProps } from "./protocol";

interface Chip {
  label: string;
  prompt?: string;
  command?: "compact" | string;
}

/** One-click next steps: a prompt to send, or a command such as compaction. */
export function ActionChips({ spec, laterUserTexts, actions, disabled }: WidgetProps) {
  const chips = (spec.data.chips as Chip[]) ?? [];
  const [picked, setPicked] = useState<number | null>(null);
  const chosen = picked ?? chips.findIndex((c) => c.prompt && laterUserTexts.includes(c.prompt));
  const run = async (c: Chip, i: number) => {
    setPicked(i);
    try {
      if (c.command === "compact") await actions.compact();
      else await actions.prompt(c.prompt ?? c.label);
    } catch {
      setPicked(null);
    }
  };
  return (
    <div className="chips-widget">
      {spec.title && <div className="chips-title">{spec.title}</div>}
      <div className="chips">
        {chips.map((c, i) => (
          <button key={i} className={`chip-btn ${chosen === i ? "chosen" : ""}`} disabled={disabled || (chosen !== -1 && chosen !== null)} onClick={() => run(c, i)}>
            {c.command === "compact" ? "⇣ " : ""}
            {c.label}
          </button>
        ))}
      </div>
    </div>
  );
}
