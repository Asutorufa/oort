import { createContext, useContext, useEffect, useState, type ReactNode } from "react";

export type ThemeName = "iris" | "lunar";
export type Mode = "light" | "dark";

export const THEMES: { id: ThemeName; label: string; swatch: string }[] = [
  { id: "iris", label: "星雲暮紫", swatch: "#5B5BD6" },
  { id: "lunar", label: "月光極簡", swatch: "#52525B" },
];

const THEME_KEY = "oort.theme";
const MODE_KEY = "oort.mode";

interface ThemeCtx {
  theme: ThemeName;
  mode: Mode;
  setTheme: (t: ThemeName) => void;
  setMode: (m: Mode) => void;
}

const Ctx = createContext<ThemeCtx | null>(null);

function initialMode(): Mode {
  const saved = localStorage.getItem(MODE_KEY);
  if (saved === "light" || saved === "dark") return saved;
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function initialTheme(): ThemeName {
  return localStorage.getItem(THEME_KEY) === "lunar" ? "lunar" : "iris";
}

/** Theme and light/dark mode, persisted in localStorage and applied to <html>. */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<ThemeName>(initialTheme);
  const [mode, setModeState] = useState<Mode>(initialMode);

  useEffect(() => {
    const el = document.documentElement;
    el.dataset.theme = theme;
    el.dataset.mode = mode;
  }, [theme, mode]);

  const value: ThemeCtx = {
    theme,
    mode,
    setTheme: (t) => {
      localStorage.setItem(THEME_KEY, t);
      setThemeState(t);
    },
    setMode: (m) => {
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
