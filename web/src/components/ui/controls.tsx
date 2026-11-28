import { AnimatePresence, motion } from "motion/react";
import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { collapse, snappy, spring } from "../../lib/motion";
import { Popover } from "./Popover";
import { useT } from "../../i18n";

// ----- SegmentedControl ----------------------------------------------------------------------

export interface Segment<T extends string> {
  value: T;
  label: ReactNode;
  title?: string;
  disabled?: boolean;
}

/** Mutually exclusive choice; the selected pill slides between segments (`layoutId`). */
export function SegmentedControl<T extends string>({
  value,
  options,
  onChange,
  ariaLabel,
  disabled,
  size = "md",
  full,
  className = "",
}: {
  value: T;
  options: Segment<T>[];
  onChange: (v: T) => void;
  ariaLabel?: string;
  disabled?: boolean;
  size?: "sm" | "md";
  /** Stretch segments to the container width. */
  full?: boolean;
  className?: string;
}) {
  const id = useId();
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const onKey = (e: KeyboardEvent, i: number) => {
    const dir = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
    if (!dir) return;
    e.preventDefault();
    for (let k = 1; k <= options.length; k++) {
      const j = (i + dir * k + options.length) % options.length;
      if (!options[j].disabled) {
        onChange(options[j].value);
        refs.current[j]?.focus();
        return;
      }
    }
  };
  return (
    <div role="radiogroup" aria-label={ariaLabel} className={`segmented ${size} ${full ? "full" : ""} ${disabled ? "disabled" : ""} ${className}`}>
      {options.map((o, i) => {
        const on = o.value === value;
        return (
          <button
            key={o.value}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={on}
            tabIndex={on ? 0 : -1}
            title={o.title}
            disabled={disabled || o.disabled}
            className={on ? "on" : ""}
            onClick={() => onChange(o.value)}
            onKeyDown={(e) => onKey(e, i)}
          >
            {on && <motion.span layoutId={`${id}-pill`} className="segmented-pill" transition={spring} />}
            <span className="segmented-label">{o.label}</span>
          </button>
        );
      })}
    </div>
  );
}

// ----- Switch --------------------------------------------------------------------------------

/** On/off setting with a spring knob. The label (and description) are part of the hit area. */
export function Switch({
  checked,
  onChange,
  label,
  description,
  disabled,
  size = "md",
  className = "",
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: ReactNode;
  description?: ReactNode;
  disabled?: boolean;
  size?: "sm" | "md";
  className?: string;
}) {
  const id = useId();
  const sw = (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-labelledby={label ? `${id}-l` : undefined}
      disabled={disabled}
      className={`switch ${size} ${checked ? "on" : ""}`}
      onClick={() => onChange(!checked)}
    >
      <motion.span className="switch-knob" layout transition={snappy} />
    </button>
  );
  if (!label) return sw;
  return (
    <div className={`switch-row ${disabled ? "disabled" : ""} ${className}`} onClick={(e) => e.target === e.currentTarget && !disabled && onChange(!checked)}>
      <span className="switch-text" onClick={() => !disabled && onChange(!checked)}>
        <span id={`${id}-l`} className="switch-label">
          {label}
        </span>
        {description && <span className="switch-desc">{description}</span>}
      </span>
      {sw}
    </div>
  );
}

// ----- Checkbox ------------------------------------------------------------------------------

/** Secondary option; the check mark draws itself. */
export function Checkbox({
  checked,
  onChange,
  label,
  disabled,
  className = "",
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: ReactNode;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      disabled={disabled}
      className={`checkbox ${checked ? "on" : ""} ${className}`}
      onClick={() => onChange(!checked)}
    >
      <span className="checkbox-box">
        <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
          <motion.path
            d="M3.5 8.5 6.5 11.5 12.5 4.5"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            initial={false}
            animate={{ pathLength: checked ? 1 : 0, opacity: checked ? 1 : 0 }}
            transition={{ duration: 0.18, ease: [0.2, 0, 0, 1] }}
          />
        </svg>
      </span>
      {label && <span className="checkbox-label">{label}</span>}
    </button>
  );
}

// ----- Collapse ------------------------------------------------------------------------------

/** Height + opacity accordion body. */
export function Collapse({ open, children, className = "" }: { open: boolean; children: ReactNode; className?: string }) {
  return (
    <AnimatePresence initial={false}>
      {open && (
        <motion.div className={`collapse ${className}`} variants={collapse} initial="closed" animate="open" exit="closed" style={{ overflow: "hidden" }}>
          {children}
        </motion.div>
      )}
    </AnimatePresence>
  );
}

// ----- Combobox ------------------------------------------------------------------------------

/** Free text with a filtered suggestion list (e.g. model ids); typing anything stays allowed. */
export function Combobox({
  value,
  onChange,
  options,
  placeholder,
  className = "",
  emptyText,
}: {
  value: string;
  onChange: (v: string) => void;
  options: string[];
  placeholder?: string;
  className?: string;
  emptyText?: string;
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const t = useT();
  const wrap = useRef<HTMLDivElement>(null);
  const q = value.trim().toLowerCase();
  const exact = options.some((o) => o.toLowerCase() === q);
  const shown = (q && !exact ? options.filter((o) => o.toLowerCase().includes(q)) : options).slice(0, 200);
  const pick = (v: string) => {
    onChange(v);
    setOpen(false);
  };
  return (
    <div ref={wrap} className={`combobox ${className}`}>
      <input
        value={value}
        placeholder={placeholder}
        role="combobox"
        aria-expanded={open}
        aria-autocomplete="list"
        onFocus={() => options.length > 0 && setOpen(true)}
        onClick={() => options.length > 0 && setOpen(true)}
        onChange={(e) => {
          onChange(e.target.value);
          setActive(0);
          setOpen(options.length > 0);
        }}
        onKeyDown={(e) => {
          if (!open || shown.length === 0) return;
          if (e.key === "ArrowDown") return e.preventDefault(), setActive((active + 1) % shown.length);
          if (e.key === "ArrowUp") return e.preventDefault(), setActive((active - 1 + shown.length) % shown.length);
          if (e.key === "Enter" && !e.nativeEvent.isComposing) return e.preventDefault(), pick(shown[active]);
          if (e.key === "Escape") return e.preventDefault(), setOpen(false);
        }}
      />
      <Popover anchor={wrap} open={open && options.length > 0} onClose={() => setOpen(false)} align="stretch" className="menu-pop">
        <div className="menu-list" role="listbox">
          {shown.length === 0 && <div className="menu-empty">{emptyText ?? t("combobox.empty")}</div>}
          {shown.map((o, i) => (
            <div
              key={o}
              role="option"
              aria-selected={o === value}
              className={`menu-item mono ${i === active ? "active" : ""} ${o === value ? "selected" : ""}`}
              onMouseEnter={() => setActive(i)}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => pick(o)}
            >
              <span className="menu-item-text">
                <span className="menu-item-label">{o}</span>
              </span>
            </div>
          ))}
        </div>
      </Popover>
    </div>
  );
}
