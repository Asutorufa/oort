import { motion } from "motion/react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { analytics, BUDGET_CHANGED, type BudgetState } from "../../api/analytics";
import { HeaderSlot } from "../../components/settings/MasterDetail";
import { ArrowLeftIcon, ChartIcon, LayersIcon, MemoryIcon, ModelIcon, PaletteIcon, SparkIcon, ToolIcon } from "../../brand/Icons";
import { href, type SettingsView } from "../../hooks/useHashRoute";
import { useT, type Key } from "../../i18n";
import { McpView } from "../McpView";
import { MemoriesView } from "../MemoriesView";
import { PresetsView } from "../PresetsView";
import { ProvidersView } from "../ProvidersView";
import { SkillsView } from "../SkillsView";
import { UsageView } from "../usage/UsageView";
import { GeneralSettings } from "./GeneralSettings";

interface Page {
  view: SettingsView;
  label: Key;
  description: Key;
  icon: ReactNode;
}

const GROUPS: { title: Key; pages: Page[] }[] = [
  {
    title: "settings.groupApp",
    pages: [
      { view: "general", label: "settings.general", description: "settings.general.desc", icon: <PaletteIcon size={16} /> },
      { view: "usage", label: "usage.title", description: "usage.desc", icon: <ChartIcon size={16} /> },
    ],
  },
  {
    title: "settings.groupAgent",
    pages: [
      { view: "models", label: "nav.models", description: "settings.models.desc", icon: <ModelIcon size={16} /> },
      { view: "presets", label: "nav.presets", description: "settings.presets.desc", icon: <LayersIcon size={16} /> },
      { view: "mcp", label: "nav.tools", description: "settings.mcp.desc", icon: <ToolIcon size={16} /> },
      { view: "skills", label: "nav.skills", description: "settings.skills.desc", icon: <SparkIcon size={16} /> },
      { view: "memories", label: "nav.memories", description: "settings.memories.desc", icon: <MemoryIcon size={16} /> },
    ],
  },
];

/** Where this month stands against the budget (`null` until known); only shown as a quiet dot. */
function useBudgetState(): BudgetState | null {
  const [state, setState] = useState<BudgetState | null>(null);
  useEffect(() => {
    // A failed check just leaves the dot as it was.
    const check = () => void analytics.budget().then((b) => setState(b.status), () => {});
    check();
    window.addEventListener(BUDGET_CHANGED, check);
    const timer = window.setInterval(check, 60_000);
    return () => {
      window.removeEventListener(BUDGET_CHANGED, check);
      window.clearInterval(timer);
    };
  }, []);
  return state;
}

/** Full-viewport settings: a navigation rail and one page. Covers the (still mounted) chat. */
export function SettingsWorkspace({ view, id, onClose }: { view: SettingsView; id: string | null; onClose: () => void }) {
  const t = useT();
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => root.current?.focus({ preventScroll: true }), []);
  const [slot, setSlot] = useState<HTMLDivElement | null>(null);
  const budget = useBudgetState();
  const page = GROUPS.flatMap((g) => g.pages).find((p) => p.view === view)!;

  return (
    <motion.div
      ref={root}
      className="settings"
      role="dialog"
      aria-modal="true"
      aria-label={t("settings.title")}
      tabIndex={-1}
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.15 }}
    >
      <aside className="settings-rail">
        <button className="settings-back" onClick={onClose}>
          <ArrowLeftIcon size={15} />
          {t("settings.back")}
        </button>
        {GROUPS.map((g) => (
          <nav key={g.title} aria-label={t(g.title)}>
            <div className="settings-group">{t(g.title)}</div>
            {g.pages.map((p) => (
              <a key={p.view} href={href(p.view)} className={`settings-nav ${p.view === view ? "active" : ""}`} aria-current={p.view === view ? "page" : undefined}>
                {p.icon}
                <span>{t(p.label)}</span>
                {p.view === "usage" && (budget === "warning" || budget === "exceeded") && (
                  <i className={`u-alert is-${budget}`} role="img" aria-label={t(`usage.alert.${budget}`)} title={t(`usage.alert.${budget}`)} />
                )}
              </a>
            ))}
          </nav>
        ))}
      </aside>
      <div className="settings-content">
        {/* List + detail pages are wider; the tool playground widest. */}
        <div className={`settings-container ${view === "general" ? "" : view === "usage" ? "usage-page" : view === "mcp" ? "widest" : "wide"}`}>
          <header className="settings-head">
            <div>
              <h1>{t(page.label)}</h1>
              <p>{t(page.description)}</p>
            </div>
            <div className="settings-actions" ref={setSlot} />
          </header>
          <div className="settings-body">
            <HeaderSlot.Provider value={slot}>
              {view === "general" && <GeneralSettings />}
              {view === "usage" && <UsageView />}
              {view === "models" && <ProvidersView selected={id} />}
              {view === "presets" && <PresetsView selected={id} />}
              {view === "mcp" && <McpView selected={id} />}
              {view === "skills" && <SkillsView selected={id} />}
              {view === "memories" && <MemoriesView selected={id} />}
            </HeaderSlot.Provider>
          </div>
        </div>
      </div>
    </motion.div>
  );
}
