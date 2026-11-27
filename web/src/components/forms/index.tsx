import { useEffect, useRef, useState, type ReactNode } from "react";
import type { SecretSource } from "../../api/types";

export function Field({ label, hint, children }: { label: string; hint?: ReactNode; children: ReactNode }) {
  return (
    // A div, not a <label>: fields also wrap buttons and checkbox groups, which a label would
    // activate on any click.
    <div className="field">
      <span className="field-label">{label}</span>
      {children}
      {hint && <span className="field-hint">{hint}</span>}
    </div>
  );
}

/**
 * Secret input. The stored value is never sent to the browser; `current` is its masked form
 * (or an `env:` reference). Leaving the box empty keeps the stored value.
 */
export function SecretInput({
  current,
  source,
  value,
  onChange,
  placeholder,
}: {
  current?: string | null;
  source?: SecretSource;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  const stored = current && source !== "none";
  return (
    <div className="secret">
      <input
        type="password"
        autoComplete="off"
        value={value}
        placeholder={stored ? `${current} (leave empty to keep)` : placeholder ?? "sk-… or env:VAR_NAME"}
        onChange={(e) => onChange(e.target.value)}
      />
      {stored && <span className="muted">current: {source === "env" ? current : `${current} (stored)`}</span>}
    </div>
  );
}

type Pairs = [string, string][];

/** Key/value editor. Values of existing secrets arrive masked and are sent back as is (kept). */
export function KeyValueEditor({
  value,
  onChange,
  keyPlaceholder = "name",
  valuePlaceholder = "value or env:VAR",
}: {
  value: Record<string, string>;
  onChange: (v: Record<string, string>) => void;
  keyPlaceholder?: string;
  valuePlaceholder?: string;
}) {
  const [rows, setRows] = useState<Pairs>(() => Object.entries(value));
  // Resync only when the value comes from outside (not our own edit echoed back).
  const emitted = useRef(value);
  useEffect(() => {
    if (value !== emitted.current) setRows(Object.entries(value));
  }, [value]);
  const commit = (next: Pairs) => {
    setRows(next);
    const out = Object.fromEntries(next.filter(([k]) => k.trim()).map(([k, v]) => [k.trim(), v]));
    emitted.current = out;
    onChange(out);
  };
  return (
    <div className="kv-editor">
      {rows.map(([k, v], i) => (
        <div className="row" key={i}>
          <input value={k} placeholder={keyPlaceholder} onChange={(e) => commit(rows.map((r, j) => (j === i ? [e.target.value, r[1]] : r)))} />
          <input value={v} placeholder={valuePlaceholder} onChange={(e) => commit(rows.map((r, j) => (j === i ? [r[0], e.target.value] : r)))} />
          <button className="ghost" title="remove" onClick={() => commit(rows.filter((_, j) => j !== i))}>
            ✕
          </button>
        </div>
      ))}
      <button className="small" onClick={() => setRows([...rows, ["", ""]])}>
        + add
      </button>
    </div>
  );
}

/** One string per line (stdio args). */
export function LinesEditor({ value, onChange, placeholder }: { value: string[]; onChange: (v: string[]) => void; placeholder?: string }) {
  const [text, setText] = useState(value.join("\n"));
  const emitted = useRef(value);
  useEffect(() => {
    if (value !== emitted.current) setText(value.join("\n"));
  }, [value]);
  return (
    <textarea
      className="mono"
      rows={Math.max(2, Math.min(8, text.split("\n").length))}
      value={text}
      placeholder={placeholder}
      onChange={(e) => {
        setText(e.target.value);
        const out = e.target.value.split("\n").filter((l) => l.length > 0);
        emitted.current = out;
        onChange(out);
      }}
    />
  );
}

/** JSON textarea with validation and a format button. `onChange` receives parsed values only. */
export function JsonEditor({
  value,
  onChange,
  rows = 8,
}: {
  value: string;
  onChange: (text: string, parsed: unknown | undefined) => void;
  rows?: number;
}) {
  let error: string | null = null;
  try {
    if (value.trim()) JSON.parse(value);
  } catch (e) {
    error = (e as Error).message;
  }
  return (
    <div className="json-editor">
      <textarea
        className={`mono ${error ? "invalid" : ""}`}
        rows={rows}
        spellCheck={false}
        value={value}
        onChange={(e) => {
          let parsed: unknown;
          try {
            parsed = e.target.value.trim() ? JSON.parse(e.target.value) : {};
          } catch {
            parsed = undefined;
          }
          onChange(e.target.value, parsed);
        }}
      />
      <div className="row">
        <span className={error ? "field-error" : "muted"}>{error ?? "valid JSON"}</span>
        <span className="spacer" />
        <button
          className="small"
          disabled={!!error}
          onClick={() => {
            const parsed = value.trim() ? JSON.parse(value) : {};
            onChange(JSON.stringify(parsed, null, 2), parsed);
          }}
        >
          Format
        </button>
      </div>
    </div>
  );
}

export function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: ReactNode }) {
  return (
    <label className="check">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      {label}
    </label>
  );
}
