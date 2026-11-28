import { useCallback, useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { CheckIcon, ChevronDown } from "../../brand/Icons";
import { Popover, type Placement } from "./Popover";

export interface Option<T extends string> {
  value: T;
  label: ReactNode;
  /** One short line under the label. */
  description?: ReactNode;
  icon?: ReactNode;
  disabled?: boolean;
}

/**
 * Listbox select in Oort's style: hairline trigger, frosted popover with check marks and
 * optional descriptions, ↑/↓/Home/End/Enter/Esc and type-to-jump. `variant="pill"` is the
 * compact form used in toolbars (the Composer opens upwards with `placement="top"`).
 */
export function Select<T extends string>({
  value,
  options,
  onChange,
  placeholder = "選択してください",
  placement = "auto",
  align = "start",
  variant = "field",
  disabled,
  ariaLabel,
  icon,
  renderValue,
  className = "",
  menuClassName = "",
  title,
}: {
  value: T | null | undefined;
  options: Option<T>[];
  onChange: (v: T) => void;
  placeholder?: string;
  placement?: Placement;
  align?: "start" | "end" | "stretch";
  variant?: "field" | "pill" | "ghost";
  disabled?: boolean;
  ariaLabel?: string;
  /** Leading icon in the trigger. */
  icon?: ReactNode;
  renderValue?: (o: Option<T> | undefined) => ReactNode;
  className?: string;
  menuClassName?: string;
  title?: string;
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const typed = useRef({ text: "", at: 0 });
  const id = useId();
  const current = options.find((o) => o.value === value);
  const close = useCallback(() => {
    setOpen(false);
    trigger.current?.focus();
  }, []);

  useEffect(() => {
    if (!open) return;
    setActive(Math.max(0, options.findIndex((o) => o.value === value)));
    requestAnimationFrame(() => list.current?.focus());
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    list.current?.querySelector(`[data-i="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const move = (from: number, dir: 1 | -1) => {
    for (let i = 1; i <= options.length; i++) {
      const j = (from + dir * i + options.length) % options.length;
      if (!options[j].disabled) return j;
    }
    return from;
  };
  const pick = (o: Option<T>) => {
    if (o.disabled) return;
    onChange(o.value);
    close();
  };
  const onListKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown") return e.preventDefault(), setActive(move(active, 1));
    if (e.key === "ArrowUp") return e.preventDefault(), setActive(move(active, -1));
    if (e.key === "Home") return e.preventDefault(), setActive(move(-1, 1));
    if (e.key === "End") return e.preventDefault(), setActive(move(options.length, -1));
    if (e.key === "Enter" || e.key === " ") return e.preventDefault(), options[active] && pick(options[active]);
    if (e.key === "Tab") return setOpen(false);
    if (e.key.length === 1) {
      const now = Date.now();
      typed.current = { text: (now - typed.current.at < 600 ? typed.current.text : "") + e.key.toLowerCase(), at: now };
      const hit = options.findIndex((o) => !o.disabled && String(typeof o.label === "string" ? o.label : o.value).toLowerCase().startsWith(typed.current.text));
      if (hit >= 0) setActive(hit);
    }
  };

  return (
    <>
      <button
        ref={trigger}
        type="button"
        className={`select-trigger ${variant} ${open ? "open" : ""} ${className}`}
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        aria-label={ariaLabel}
        title={title}
        onClick={() => setOpen(!open)}
        onKeyDown={(e) => {
          if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
            e.preventDefault();
            setOpen(true);
          }
        }}
      >
        {icon && <span className="select-icon">{icon}</span>}
        {current?.icon && !icon && <span className="select-icon">{current.icon}</span>}
        <span className={`select-value ${current ? "" : "placeholder"}`}>{renderValue ? renderValue(current) : current?.label ?? placeholder}</span>
        <span className="select-chev">
          <ChevronDown size={14} />
        </span>
      </button>
      <Popover anchor={trigger} open={open} onClose={() => setOpen(false)} placement={placement} align={align} className={`menu-pop ${menuClassName}`}>
        <div
          ref={list}
          id={id}
          role="listbox"
          tabIndex={-1}
          aria-label={ariaLabel}
          aria-activedescendant={`${id}-${active}`}
          className="menu-list"
          onKeyDown={onListKey}
        >
          {options.map((o, i) => {
            const selected = o.value === value;
            return (
              <div
                key={o.value}
                id={`${id}-${i}`}
                data-i={i}
                role="option"
                aria-selected={selected}
                aria-disabled={o.disabled}
                className={`menu-item ${i === active ? "active" : ""} ${selected ? "selected" : ""} ${o.disabled ? "disabled" : ""}`}
                onMouseEnter={() => !o.disabled && setActive(i)}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => pick(o)}
              >
                {o.icon && <span className="menu-item-icon">{o.icon}</span>}
                <span className="menu-item-text">
                  <span className="menu-item-label">{o.label}</span>
                  {o.description && <span className="menu-item-desc">{o.description}</span>}
                </span>
                <span className="menu-item-check">{selected && <CheckIcon size={14} stroke={2} />}</span>
              </div>
            );
          })}
        </div>
      </Popover>
    </>
  );
}
