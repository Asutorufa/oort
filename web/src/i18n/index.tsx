import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { en, type Dict, type Key, type Message } from "./en";
import { ja } from "./ja";
import { ko } from "./ko";

/**
 * Lightweight, type-safe i18n.
 *
 * - `en` is the source of truth; `ja` and `ko` are typed as `Dict`, so a missing, extra or
 *   misspelled key fails `tsc`.
 * - Messages interpolate `{{name}}`; a message may be `{ one, other }` and is chosen with
 *   `Intl.PluralRules` from the `count` parameter (Japanese and Korean only need `other`).
 * - Only the UI changes language: prompts, history and anything sent to the model are untouched.
 */

export type Locale = "en" | "ja" | "ko";
export const LOCALES: { id: Locale; code: string; native: string }[] = [
  { id: "en", code: "EN", native: "English" },
  { id: "ja", code: "JA", native: "日本語" },
  { id: "ko", code: "KO", native: "한국어" },
];

const DICTS: Record<Locale, Dict> = { en, ja, ko };

/** The active locale, for the few places outside React state (kept in sync by the provider). */
let active: Locale = "en";
export const currentLocale = () => active;
const KEY = "oort.locale";

export type Params = Record<string, string | number>;
export type T = (key: Key, params?: Params) => string;

function detect(): Locale {
  const saved = localStorage.getItem(KEY);
  if (saved === "en" || saved === "ja" || saved === "ko") return saved;
  for (const l of navigator.languages ?? [navigator.language]) {
    const base = l.toLowerCase().split("-")[0];
    if (base === "ja" || base === "ko" || base === "en") return base;
  }
  return "en";
}

function render(locale: Locale, msg: Message, params?: Params): string {
  let text: string;
  if (typeof msg === "string") text = msg;
  else {
    const n = Number(params?.count ?? 0);
    const cat = new Intl.PluralRules(locale).select(n) as keyof typeof msg;
    text = (msg[cat] ?? msg.other) as string;
  }
  if (!params) return text;
  return text.replace(/\{\{(\w+)\}\}/g, (m, k: string) => (k in params ? String(params[k]) : m));
}

export function translate(locale: Locale, key: Key, params?: Params): string {
  return render(locale, DICTS[locale][key] ?? en[key], params);
}

interface I18n {
  locale: Locale;
  setLocale: (l: Locale) => void;
  t: T;
  /** Locale-aware formatting. */
  fmt: {
    number: (n: number, opts?: Intl.NumberFormatOptions) => string;
    compact: (n: number) => string;
    percent: (ratio: number, digits?: number) => string;
    date: (ms: number, opts?: Intl.DateTimeFormatOptions) => string;
    time: (ms: number) => string;
    relative: (ms: number) => string;
    ms: (ms: number | null | undefined) => string;
  };
}

const Ctx = createContext<I18n | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(detect);
  active = locale;
  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);
  const setLocale = useCallback((l: Locale) => {
    localStorage.setItem(KEY, l);
    setLocaleState(l);
  }, []);
  const value = useMemo<I18n>(() => {
    const nf = new Intl.NumberFormat(locale);
    const rtf = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
    const t: T = (key, params) => translate(locale, key, params);
    return {
      locale,
      setLocale,
      t,
      fmt: {
        number: (n, opts) => (opts ? new Intl.NumberFormat(locale, opts).format(n) : nf.format(n)),
        compact: (n) => new Intl.NumberFormat(locale, { notation: "compact", maximumFractionDigits: 1 }).format(n),
        percent: (r, digits = 1) => new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: digits }).format(r),
        date: (ms, opts) => new Intl.DateTimeFormat(locale, opts ?? { dateStyle: "medium", timeStyle: "short" }).format(ms),
        time: (ms) => new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(ms),
        relative: (ms) => {
          const s = Math.round((ms - Date.now()) / 1000);
          const a = Math.abs(s);
          if (a < 60) return rtf.format(s, "second");
          if (a < 3600) return rtf.format(Math.round(s / 60), "minute");
          if (a < 86400) return rtf.format(Math.round(s / 3600), "hour");
          return rtf.format(Math.round(s / 86400), "day");
        },
        ms: (ms) => {
          if (ms == null) return "–";
          if (ms < 1000) return `${nf.format(Math.round(ms))} ms`;
          return `${new Intl.NumberFormat(locale, { maximumFractionDigits: ms < 10_000 ? 2 : 1 }).format(ms / 1000)} s`;
        },
      },
    };
  }, [locale, setLocale]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useI18n(): I18n {
  const v = useContext(Ctx);
  if (!v) throw new Error("useI18n outside I18nProvider");
  return v;
}

/** Shorthand for components that only translate. */
export const useT = (): T => useI18n().t;

export type { Key };
