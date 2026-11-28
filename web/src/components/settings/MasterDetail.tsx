import { createContext, useContext, useEffect, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { PlusIcon } from "../../brand/Icons";
import { href, replace, type SettingsView } from "../../hooks/useHashRoute";

/** The right side of the settings page header, where a page puts its primary action. */
export const HeaderSlot = createContext<HTMLElement | null>(null);

export function HeaderActions({ children }: { children: ReactNode }) {
  const slot = useContext(HeaderSlot);
  return slot ? createPortal(children, slot) : null;
}

export function AddButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button type="button" className="head-action primary-action" onClick={onClick}>
      <PlusIcon size={14} />
      {label}
    </button>
  );
}

/**
 * Keeps an item open whenever there is one: with nothing (or something deleted) selected, goes to
 * `preferred`, else the first item. Replaces the history entry, so Back is not trapped.
 */
export function useAutoSelect<T extends { id: string }>(view: SettingsView, items: T[] | null, selected: string | null, preferred?: (x: T) => boolean) {
  useEffect(() => {
    if (!items?.length || selected === "new" || items.some((x) => x.id === selected)) return;
    // Settings may be closing, or the route may already have moved on: never pull it back.
    if (window.location.hash !== href(view, selected)) return;
    replace(view, (items.find((x) => preferred?.(x)) ?? items[0]).id);
  }, [view, items, selected]); // eslint-disable-line react-hooks/exhaustive-deps
}

/** One row of a settings list: icon, name and a line about it; a dot only when it is switched off. */
export function ListItem({
  selected,
  onSelect,
  icon,
  iconTitle,
  title,
  sub,
  mono,
  off,
}: {
  selected: boolean;
  onSelect: () => void;
  icon: ReactNode;
  iconTitle?: string;
  title: ReactNode;
  sub: ReactNode;
  /** `sub` is a model id, command or URL. */
  mono?: boolean;
  /** Set (to the word for it) when the item is disabled. */
  off?: string;
}) {
  return (
    <li>
      <button type="button" className={`md-item ${selected ? "selected" : ""}`} aria-current={selected || undefined} onClick={onSelect}>
        <span className="md-icon" title={iconTitle}>
          {icon}
        </span>
        <span className="md-text">
          <span className="md-title">{title}</span>
          <span className={`md-sub ${mono ? "mono" : ""}`} title={typeof sub === "string" ? sub : undefined}>
            {sub}
          </span>
        </span>
        {off && <span className="md-off" role="img" aria-label={off} title={off} />}
      </button>
    </li>
  );
}

/** Shown only when there is nothing at all. */
export function EmptyState({ icon, title, children, action }: { icon: ReactNode; title: string; children: ReactNode; action: { label: string; onClick: () => void } }) {
  return (
    <div className="md-empty">
      <span className="md-empty-icon">{icon}</span>
      <h2>{title}</h2>
      <p>{children}</p>
      <button type="button" className="head-action" onClick={action.onClick}>
        <PlusIcon size={14} />
        {action.label}
      </button>
    </div>
  );
}
