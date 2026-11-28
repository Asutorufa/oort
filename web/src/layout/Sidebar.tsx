import { useEffect, useRef, useState, type ReactNode } from "react";
import { errorText, sessions } from "../api";
import type { SessionSummary } from "../api/types";
import { GlobeIcon, LayersIcon, MemoryIcon, ModelIcon, MoonIcon, MoreIcon, PlusIcon, SidebarIcon, SparkIcon, SunIcon, ToolIcon } from "../brand/Icons";
import { OortMark } from "../brand/OortMark";
import { href, navigate, type View } from "../hooks/useHashRoute";
import { THEMES, useTheme } from "../theme/ThemeProvider";
import { AnimatePresence, motion } from "motion/react";
import { Popover, SegmentedControl, Select } from "../components/ui";
import { LOCALES, useI18n, type Key, type Locale } from "../i18n";
import { fade, spring } from "../lib/motion";
import { Modal } from "../components/common";

const NAV: { view: View; label: Key; icon: ReactNode }[] = [
  { view: "mcp", label: "nav.tools", icon: <ToolIcon /> },
  { view: "skills", label: "nav.skills", icon: <SparkIcon /> },
  { view: "memories", label: "nav.memories", icon: <MemoryIcon /> },
  { view: "models", label: "nav.models", icon: <ModelIcon /> },
  { view: "presets", label: "nav.presets", icon: <LayersIcon /> },
];

function group(ms: number): Key {
  const d = new Date(ms);
  const today = new Date();
  const days = Math.floor((new Date(today.toDateString()).getTime() - new Date(d.toDateString()).getTime()) / 86400000);
  return days <= 0 ? "sidebar.today" : days === 1 ? "sidebar.yesterday" : days < 7 ? "sidebar.last7" : "sidebar.older";
}

/** Collapsible left sidebar: brand, new chat, conversations, navigation, theme. */
export function Sidebar({
  view,
  selected,
  items,
  onChanged,
}: {
  view: View;
  selected: string | null;
  items: SessionSummary[];
  onChanged: () => void;
}) {
  const { theme, mode, setTheme, setMode } = useTheme();
  const { t, locale, setLocale } = useI18n();
  const tr = t;
  const [collapsed, setCollapsed] = useState(() => localStorage.getItem("oort.sidebar") === "collapsed");
  const [menu, setMenu] = useState<string | null>(null);
  const moreRefs = useRef<Record<string, HTMLButtonElement | null>>({});
  const menuAnchor = useRef<HTMLButtonElement | null>(null);
  const menuItem = menu ? items.find((x) => x.id === menu) : undefined;
  const [renaming, setRenaming] = useState<string | null>(null);
  const [title, setTitle] = useState("");
  const renameFrom = useRef("");
  const [confirm, setConfirm] = useState<SessionSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => localStorage.setItem("oort.sidebar", collapsed ? "collapsed" : "open"), [collapsed]);


  const rename = async (id: string, value: string) => {
    if (renaming !== id) return;
    setRenaming(null);
    // An untouched name is not a rename: it would pin the title (or undo a newly generated one).
    if (value === renameFrom.current) return;
    try {
      await sessions.patch(id, { title: value });
      onChanged();
    } catch (e) {
      setError(errorText(e));
    }
  };
  const regenerate = async (id: string) => {
    try {
      await sessions.regenerateTitle(id);
      onChanged();
    } catch (e) {
      setError(errorText(e));
    }
  };
  const remove = async (s: SessionSummary) => {
    setConfirm(null);
    try {
      await sessions.remove(s.id);
      if (s.id === selected) navigate("chat");
      onChanged();
    } catch (e) {
      setError(errorText(e));
    }
  };

  let lastGroup = "";
  return (
    <aside className={`sidebar ${collapsed ? "collapsed" : ""}`}>
      <div className="sb-top">
        <a className="brand" href={href("chat")} title="Oort">
          <OortMark size={20} />
          <span className="brand-name">Oort</span>
        </a>
        <button className="btn ghost icon collapse" onClick={() => setCollapsed(!collapsed)} aria-label={collapsed ? t("sidebar.expand") : t("sidebar.collapse")}>
          <SidebarIcon />
        </button>
      </div>
      <a className="new-chat" href={href("chat", "new")} title={t("sidebar.newChat")}>
        <span className="plus">
          <PlusIcon />
        </span>
        <span className="label">{t("sidebar.newChat")}</span>
      </a>

      <nav className="conversations" aria-label={t("sidebar.conversations")}>
        {items.map((s) => {
          const g = group(s.updated_at);
          const header = g !== lastGroup ? ((lastGroup = g), <div className="conv-group">{t(g)}</div>) : null;
          return (
            <div key={s.id}>
              {header}
              <div className={`conv ${s.id === selected && view === "chat" ? "active" : ""}`} onClick={() => navigate("chat", s.id)} title={s.title || t("common.untitled")}>
                {s.id === selected && view === "chat" && <motion.span layoutId="sidebar-conv-active" className="nav-highlight" transition={spring} />}
                {s.failed ? <span className="dot error" title={t("sidebar.failed")} /> : s.status === "running" ? <span className="pulse" /> : s.status === "needs_approval" ? <span className="dot warn" title={t("status.needsApproval")} /> : null}
                {renaming === s.id ? (
                  <input
                    autoFocus
                    className="conv-rename"
                    value={title}
                    onClick={(e) => e.stopPropagation()}
                    onChange={(e) => setTitle(e.target.value)}
                    onBlur={(e) => rename(s.id, e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") rename(s.id, e.currentTarget.value);
                      if (e.key === "Escape") setRenaming(null);
                    }}
                  />
                ) : (
                  <span className="conv-title">
                    <AnimatePresence mode="wait" initial={false}>
                      <motion.span key={s.title} initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={fade}>
                        {s.title || t("common.untitled")}
                      </motion.span>
                    </AnimatePresence>
                  </span>
                )}
                <button
                  ref={(el) => {
                    moreRefs.current[s.id] = el;
                  }}
                  className={`conv-more ${menu === s.id ? "open" : ""}`}
                  aria-label={t("sidebar.menu")}
                  onClick={(e) => {
                    e.stopPropagation();
                    menuAnchor.current = moreRefs.current[s.id] ?? null;
                    setMenu(menu === s.id ? null : s.id);
                  }}
                >
                  <MoreIcon />
                </button>
              </div>
            </div>
          );
        })}
        {items.length === 0 && <div className="conv-empty">{t("sidebar.empty")}</div>}
        {error && (
          <div className="conv-empty error-text" onClick={() => setError(null)}>
            {error}
          </div>
        )}
      </nav>

      {/* Conversation menu in a portal: the scrolling list can't clip it. */}
      <Popover anchor={menuAnchor} open={!!menuItem} onClose={() => setMenu(null)} align="end" className="menu-pop conv-menu" role="menu">
        {menuItem && (
          <div className="menu-list">
            <button
              role="menuitem"
              className="menu-item"
              onClick={() => {
                setMenu(null);
                setTitle(menuItem.title);
                renameFrom.current = menuItem.title;
                setRenaming(menuItem.id);
              }}
            >
              {t("sidebar.rename")}
            </button>
            {menuItem.message_count > 0 && (
              <button
                role="menuitem"
                className="menu-item"
                onClick={() => {
                  setMenu(null);
                  regenerate(menuItem.id);
                }}
              >
                {t("sidebar.regenerateTitle")}
              </button>
            )}
            <a role="menuitem" className="menu-item" href={sessions.exportUrl(menuItem.id)} download={`oort-${menuItem.id}.json`} onClick={() => setMenu(null)}>
              {t("sidebar.exportJson")}
            </a>
            <button
              role="menuitem"
              className="menu-item danger-item"
              onClick={() => {
                setMenu(null);
                setConfirm(menuItem);
              }}
            >
              {t("common.delete")}
            </button>
          </div>
        )}
      </Popover>

      <div className="sb-bottom">
        {NAV.map((n) => (
          <a key={n.view} href={href(n.view)} className={`nav ${view === n.view ? "active" : ""}`} title={t(n.label)}>
            {view === n.view && <motion.span layoutId="sidebar-nav-active" className="nav-highlight" transition={spring} />}
            <span className="icon">{n.icon}</span>
            <span className="label">{t(n.label)}</span>
          </a>
        ))}
        <div className="util-row">
          <SegmentedControl
            size="sm"
            className="theme-seg"
            ariaLabel={t("sidebar.theme")}
            value={theme}
            onChange={setTheme}
            options={THEMES.map((th) => ({
              value: th.id,
              title: tr(th.label),
              label: (
                <>
                  <span className="swatch" style={{ background: th.swatch }} />
                  <span className="label">{tr(th.label)}</span>
                </>
              ),
            }))}
          />
          <Select<Locale>
            variant="ghost"
            placement="top"
            align="end"
            className="lang-btn"
            ariaLabel={t("sidebar.language")}
            title={t("sidebar.language")}
            icon={<GlobeIcon size={14} />}
            value={locale}
            onChange={setLocale}
            renderValue={() => LOCALES.find((l) => l.id === locale)?.code}
            options={LOCALES.map((l) => ({ value: l.id, label: l.native, description: l.code }))}
          />
          <button className="mode-btn" onClick={() => setMode(mode === "light" ? "dark" : "light")} title={mode === "light" ? t("sidebar.toDark") : t("sidebar.toLight")} aria-label={t("sidebar.toggleMode")}>
            {mode === "light" ? <MoonIcon /> : <SunIcon />}
          </button>
        </div>
      </div>

      {confirm && (
        <Modal title={t("sidebar.deleteTitle")} onClose={() => setConfirm(null)}>
          <p>
            {t("sidebar.deleteBody", { title: confirm.title || t("common.untitled") })}
          </p>
          <div className="row">
            <span className="spacer" />
            <button className="btn" onClick={() => setConfirm(null)}>
              {t("common.keep")}
            </button>
            <button className="btn danger" onClick={() => remove(confirm)}>
              {t("common.deleteConfirm")}
            </button>
          </div>
        </Modal>
      )}
    </aside>
  );
}
