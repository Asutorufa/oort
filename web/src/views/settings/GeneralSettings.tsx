import type { ReactNode } from "react";
import { MonitorIcon, MoonIcon, SunIcon } from "../../brand/Icons";
import { SegmentedControl } from "../../components/ui";
import { LOCALES, useI18n, type Key, type Locale } from "../../i18n";
import { THEMES, useTheme, type ModePreference } from "../../theme/ThemeProvider";

const MODES: { value: ModePreference; label: Key; icon: ReactNode }[] = [
  { value: "light", label: "settings.mode.light", icon: <SunIcon size={14} /> },
  { value: "dark", label: "settings.mode.dark", icon: <MoonIcon size={14} /> },
  { value: "system", label: "settings.mode.system", icon: <MonitorIcon size={14} /> },
];

function Row({ title, hint, children }: { title: string; hint: string; children: ReactNode }) {
  return (
    <div className="setting-row">
      <div className="setting-text">
        <div className="setting-title">{title}</div>
        <div className="setting-hint">{hint}</div>
      </div>
      {children}
    </div>
  );
}

/** Language, light/dark and accent theme. */
export function GeneralSettings() {
  const { t, locale, setLocale } = useI18n();
  const { theme, modePreference, setTheme, setModePreference } = useTheme();
  return (
    <div className="setting-rows">
      <Row title={t("settings.language")} hint={t("settings.language.desc")}>
        <SegmentedControl<Locale>
          ariaLabel={t("settings.language")}
          value={locale}
          onChange={setLocale}
          options={["ja", "en", "ko"].map((id) => {
            const l = LOCALES.find((x) => x.id === id)!;
            return { value: l.id, label: <span lang={l.id}>{l.native}</span> };
          })}
        />
      </Row>
      <Row title={t("settings.mode")} hint={t("settings.mode.desc")}>
        <SegmentedControl<ModePreference>
          ariaLabel={t("settings.mode")}
          value={modePreference}
          onChange={setModePreference}
          options={MODES.map((m) => ({
            value: m.value,
            label: (
              <>
                {m.icon}
                {t(m.label)}
              </>
            ),
          }))}
        />
      </Row>
      <Row title={t("settings.accent")} hint={t("settings.accent.desc")}>
        <div className="accent-cards" role="radiogroup" aria-label={t("settings.accent")}>
          {THEMES.map((th) => (
            <button key={th.id} type="button" role="radio" aria-checked={theme === th.id} className={`accent-card ${theme === th.id ? "on" : ""}`} onClick={() => setTheme(th.id)}>
              <span className={`accent-swatch ${th.id}`} />
              {t(th.label)}
            </button>
          ))}
        </div>
      </Row>
    </div>
  );
}
