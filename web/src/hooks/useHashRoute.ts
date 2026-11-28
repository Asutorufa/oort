import { useEffect, useState } from "react";

export type View = "chat" | "general" | "usage" | "mcp" | "skills" | "memories" | "models" | "presets";
/** Every view but `chat` is a page of the settings workspace. */
export type SettingsView = Exclude<View, "chat">;
export const VIEWS: View[] = ["chat", "general", "usage", "mcp", "skills", "memories", "models", "presets"];

export interface Route {
  view: View;
  /** Selected item of the view (session / server / skill / provider / preset id, or "new"). */
  id: string | null;
}

/** `#/chat/<id>`, `#/general`, `#/mcp`, …; older links (`#/s/<id>`, `#/providers`) still work. */
function parse(hash: string): Route {
  const legacy = hash.match(/^#\/s\/(.+)$/);
  if (legacy) return { view: "chat", id: legacy[1] };
  const [, rawView, id] = hash.match(/^#\/([^/]+)(?:\/(.+))?$/) ?? [];
  const view = rawView === "providers" ? "models" : rawView;
  return VIEWS.includes(view as View) ? { view: view as View, id: id ? decodeURIComponent(id) : null } : { view: "chat", id: null };
}

export const href = (view: View, id?: string | null) => `#/${view}${id ? `/${encodeURIComponent(id)}` : ""}`;

export function navigate(view: View, id?: string | null) {
  window.location.hash = href(view, id).slice(1);
}

/** Like `navigate`, without a new history entry. */
export function replace(view: View, id?: string | null) {
  window.location.replace(href(view, id));
}

export function useHashRoute(): Route {
  const [route, setRoute] = useState(() => parse(window.location.hash));
  useEffect(() => {
    const onHash = () => setRoute(parse(window.location.hash));
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  return route;
}
