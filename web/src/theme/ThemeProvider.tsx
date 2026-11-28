import { createContext, useContext, useEffect, useState, type ReactNode } from "react";

export type ThemeName = "iris" | "lunar";
export type Mode = "light" | "dark";
export type ModePreference = Mode | "system";

export const THEMES: { id: ThemeName; label: "theme.iris" | "theme.lunar" }[] = [
  { id: "lunar", label: "theme.lunar" },
  { id: "iris", label: "theme.iris" },
];

const THEME_KEY = "oort.theme";
const MODE_KEY = "oort.mode";
const DARK_QUERY = "(prefers-color-scheme: dark)";

interface ThemeCtx {
  theme: ThemeName;
  /** The mode in effect (`system` resolved). */
  mode: Mode;
  modePreference: ModePreference;
  setTheme: (t: ThemeName) => void;
  setModePreference: (m: ModePreference) => void;
}

const Ctx = createContext<ThemeCtx | null>(null);

function initialMode(): ModePreference {
  const saved = localStorage.getItem(MODE_KEY);
  return saved === "light" || saved === "dark" ? saved : "system";
}

const systemDark = () => window.matchMedia?.(DARK_QUERY).matches ?? false;

function initialTheme(): ThemeName {
  return localStorage.getItem(THEME_KEY) === "lunar" ? "lunar" : "iris";
}

/** Theme and light/dark mode, persisted in localStorage and applied to <html>. */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<ThemeName>(initialTheme);
  const [modePreference, setModeState] = useState<ModePreference>(initialMode);
  const [dark, setDark] = useState(systemDark);
  const mode: Mode = modePreference === "system" ? (dark ? "dark" : "light") : modePreference;

  useEffect(() => {
    const query = window.matchMedia?.(DARK_QUERY);
    if (!query) return;
    const onChange = () => setDark(query.matches);
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);

  useEffect(() => {
    const el = document.documentElement;
    el.dataset.theme = theme;
    el.dataset.mode = mode;
  }, [theme, mode]);

  const value: ThemeCtx = {
    theme,
    mode,
    modePreference,
    setTheme: (t) => {
      localStorage.setItem(THEME_KEY, t);
      setThemeState(t);
    },
    setModePreference: (m) => {
      localStorage.setItem(MODE_KEY, m);
      setModeState(m);
    },
  };
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useTheme(): ThemeCtx {
  const v = useContext(Ctx);
  if (!v) throw new Error("useTheme outside ThemeProvider");
  return v;
}

/** Reads a token's current value (for charts that need concrete colors). */
export function token(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}
