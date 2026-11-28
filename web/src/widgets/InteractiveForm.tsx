import { useState } from "react";
import type { WidgetProps } from "./protocol";
import { Done, WidgetCard } from "./WidgetCard";

interface FieldSpec {
  name: string;
  label: string;
  kind: "text" | "textarea" | "number" | "select" | "checkbox" | string;
  options?: string[];
  required?: boolean;
  default?: unknown;
  placeholder?: string;
}

type Values = Record<string, unknown>;

function initial(fields: FieldSpec[]): Values {
  return Object.fromEntries(
    fields.map((f) => [f.name, f.default ?? (f.kind === "checkbox" ? false : f.kind === "select" ? (f.options?.[0] ?? "") : "")]),
  );
}

/** Dynamic form; once submitted it is locked and shows what was sent. */
export function InteractiveForm({ id, spec, response, actions, disabled }: WidgetProps) {
  const fields = (spec.data.fields as FieldSpec[]) ?? [];
  const [values, setValues] = useState<Values>(() => initial(fields));
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState<Values | null>(null);
  const submitted = (response?.payload as Values | undefined) ?? sent;
  const shown = submitted ?? values;
  const locked = !!submitted || disabled || sending;
  const missing = fields.filter((f) => f.required && (shown[f.name] === "" || shown[f.name] == null));

  const submit = async () => {
    const out: Values = Object.fromEntries(fields.map((f) => [f.name, f.kind === "number" && values[f.name] !== "" ? Number(values[f.name]) : values[f.name]]));
    setSending(true);
    try {
      setSent(out);
      await actions.respond(id, "interactive_form", out);
    } catch {
      setSent(null);
    } finally {
      setSending(false);
    }
  };

  return (
    <WidgetCard title={spec.title} kind="form" status={submitted && <Done>送信済み</Done>}>
      <form
        className={`wform ${submitted ? "locked" : ""}`}
        onSubmit={(e) => {
          e.preventDefault();
          if (!missing.length) submit();
        }}
      >
        {fields.map((f) => {
          const v = shown[f.name];
          const set = (nv: unknown) => setValues((s) => ({ ...s, [f.name]: nv }));
          return (
            <label key={f.name} className={`wfield ${f.kind === "checkbox" ? "inline" : ""}`}>
              {f.kind !== "checkbox" && (
                <span className="wlabel">
                  {f.label}
                  {f.required && <span className="req">*</span>}
                </span>
              )}
              {f.kind === "textarea" ? (
                <textarea rows={3} disabled={locked} value={String(v ?? "")} placeholder={f.placeholder} onChange={(e) => set(e.target.value)} />
              ) : f.kind === "select" ? (
                <select disabled={locked} value={String(v ?? "")} onChange={(e) => set(e.target.value)}>
                  {(f.options ?? []).map((o) => (
                    <option key={o} value={o}>
                      {o}
                    </option>
                  ))}
                </select>
              ) : f.kind === "checkbox" ? (
                <>
                  <input type="checkbox" disabled={locked} checked={!!v} onChange={(e) => set(e.target.checked)} />
                  <span>{f.label}</span>
                </>
              ) : (
                <input
                  type={f.kind === "number" ? "number" : "text"}
                  disabled={locked}
                  value={String(v ?? "")}
                  placeholder={f.placeholder}
                  onChange={(e) => set(e.target.value)}
                />
              )}
            </label>
          );
        })}
        {!submitted && (
          <div className="wactions">
            <button type="submit" className="btn primary" disabled={locked || missing.length > 0}>
              {sending ? "送信中…" : String(spec.data.submit_label ?? "送信")}
            </button>
          </div>
        )}
      </form>
    </WidgetCard>
  );
}
