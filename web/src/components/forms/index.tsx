import { useEffect, useRef, useState, type ReactNode } from "react";
import { AnimatePresence, motion } from "motion/react";
import { BUILTIN_GROUPS, type SecretSource } from "../../api/types";
import { EyeIcon, EyeOffIcon, PlusIcon, TrashIcon } from "../../brand/Icons";
import { Switch } from "../ui";
import { useT, type Key } from "../../i18n";

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
 * (or an `env:` reference). Leaving the box empty keeps the stored value. The eye button shows
 * what is being typed; the `env:` chip starts an environment-variable reference.
 */
export function SecretInput({
  current,
  source,
  value,
  onChange,
  placeholder,
  envName = "API_KEY",
}: {
  current?: string | null;
  source?: SecretSource;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  /** Suggested variable for the `env:` chip. */
  envName?: string;
}) {
  const [shown, setShown] = useState(false);
  const t = useT();
  const stored = current && source !== "none";
  return (
    <div className="secret">
      <div className="input-affix">
        <input
          type={shown ? "text" : "password"}
          autoComplete="off"
          spellCheck={false}
          value={value}
          placeholder={stored ? t("secret.keep", { current: current ?? "" }) : placeholder ?? t("secret.placeholder", { env: envName })}
          onChange={(e) => onChange(e.target.value)}
        />
        <button type="button" className="affix-btn" aria-label={shown ? t("secret.hide") : t("secret.show")} title={shown ? t("secret.hide") : t("secret.show")} onClick={() => setShown(!shown)}>
          {shown ? <EyeOffIcon /> : <EyeIcon />}
        </button>
      </div>
      <div className="secret-hints">
        <button type="button" className="hint-chip" onClick={() => onChange(`env:${envName}`)} title={t("secret.envHint")}>
          env:{envName}
        </button>
        <span className="field-hint">
          {stored ? (source === "env" ? t("secret.currentEnv", { current: current ?? "" }) : t("secret.currentStored", { current: current ?? "" })) : t("secret.envAdvice")}
        </span>
      </div>
    </div>
  );
}

type Row = { id: number; k: string; v: string };
let rowSeq = 0;

/**
 * Key/value editor. Values of existing secrets arrive masked and are sent back as is (kept).
 * With `title`, it renders its own section header with the add button on the right and an
 * empty state; rows slide in and out.
 */
export function KeyValueEditor({
  value,
  onChange,
  keyPlaceholder,
  valuePlaceholder,
  title,
  hint,
  addLabel,
  emptyText,
}: {
  value: Record<string, string>;
  onChange: (v: Record<string, string>) => void;
  keyPlaceholder?: string;
  valuePlaceholder?: string;
  title?: string;
  hint?: ReactNode;
  addLabel?: string;
  emptyText?: string;
}) {
  const t = useT();
  keyPlaceholder ??= t("kv.name");
  valuePlaceholder ??= t("kv.value");
  addLabel ??= t("common.add");
  emptyText ??= t("kv.empty");
  const toRows = (v: Record<string, string>): Row[] => Object.entries(v).map(([k, val]) => ({ id: ++rowSeq, k, v: val }));
  const [rows, setRows] = useState<Row[]>(() => toRows(value));
  // Resync only when the value comes from outside (not our own edit echoed back).
  const emitted = useRef(value);
  useEffect(() => {
    if (value !== emitted.current) setRows(toRows(value));
  }, [value]); // eslint-disable-line react-hooks/exhaustive-deps
  const commit = (next: Row[]) => {
    setRows(next);
    const out = Object.fromEntries(next.filter((r) => r.k.trim()).map((r) => [r.k.trim(), r.v]));
    emitted.current = out;
    onChange(out);
  };
  const add = () => setRows([...rows, { id: ++rowSeq, k: "", v: "" }]);
  const addButton = (
    <button type="button" className="btn ghost small add-btn" onClick={add}>
      <PlusIcon size={13} /> {addLabel}
    </button>
  );
  return (
    <div className="kv-editor">
      {title && (
        <div className="kv-head">
          <div>
            <div className="kv-title">{title}</div>
            {hint && <div className="field-hint">{hint}</div>}
          </div>
          {addButton}
        </div>
      )}
      <AnimatePresence initial={false}>
        {rows.length === 0 && title && (
          <motion.div key="empty" className="kv-empty" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0, transition: { duration: 0.08 } }}>
            {emptyText}
          </motion.div>
        )}
        {rows.map((r) => (
          <motion.div
            key={r.id}
            className="kv-row"
            layout
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={{ duration: 0.16, ease: [0.2, 0, 0, 1] }}
          >
            <input value={r.k} placeholder={keyPlaceholder} className="mono" onChange={(e) => commit(rows.map((x) => (x.id === r.id ? { ...x, k: e.target.value } : x)))} />
            <input value={r.v} placeholder={valuePlaceholder} className="mono" onChange={(e) => commit(rows.map((x) => (x.id === r.id ? { ...x, v: e.target.value } : x)))} />
            <button type="button" className="btn ghost icon" aria-label={t("kv.removeRow")} title={t("common.delete")} onClick={() => commit(rows.filter((x) => x.id !== r.id))}>
              <TrashIcon size={14} />
            </button>
          </motion.div>
        ))}
      </AnimatePresence>
      {!title && addButton}
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
  const t = useT();
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
        <span className={error ? "field-error" : "muted"}>{error ?? t("json.valid")}</span>
        <span className="spacer" />
        <button
          className="small"
          disabled={!!error}
          onClick={() => {
            const parsed = value.trim() ? JSON.parse(value) : {};
            onChange(JSON.stringify(parsed, null, 2), parsed);
          }}
        >
          {t("json.format")}
        </button>
      </div>
    </div>
  );
}

/** An on/off setting (a switch with its label). */
export function Toggle({
  checked,
  onChange,
  label,
  description,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: ReactNode;
  description?: ReactNode;
  disabled?: boolean;
}) {
  return <Switch checked={checked} onChange={onChange} label={label} description={description} disabled={disabled} />;
}

/** Builtin tool groups as toggles; the workspace path appears when Local Workspace is on. */
export function GroupToggles({
  value,
  onChange,
  workspaceRoot,
  onWorkspaceRoot,
  compact,
}: {
  value: string[];
  onChange: (v: string[]) => void;
  workspaceRoot: string;
  onWorkspaceRoot: (v: string) => void;
  compact?: boolean;
}) {
  const t = useT();
  const on = (id: string) => value.includes(id);
  return (
    <div className={`group-toggles ${compact ? "compact" : ""}`}>
      <div className="toggle-row">
        {BUILTIN_GROUPS.map((g) => (
          <button
            key={g.id}
            type="button"
            className={`toggle-chip ${on(g.id) ? "on" : ""}`}
            title={t(`groups.${g.id}.hint` as Key)}
            aria-pressed={on(g.id)}
            onClick={() => onChange(on(g.id) ? value.filter((x) => x !== g.id) : [...value, g.id])}
          >
            {t(`groups.${g.id}` as Key)}
          </button>
        ))}
      </div>
      {on("workspace") && (
        <input
          className="mono workspace-input"
          value={workspaceRoot}
          placeholder={t("groups.workspacePlaceholder")}
          onChange={(e) => onWorkspaceRoot(e.target.value)}
        />
      )}
    </div>
  );
}
