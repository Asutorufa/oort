import { useEffect, useState } from "react";

export type View = "chat" | "providers" | "mcp" | "presets";
export const VIEWS: View[] = ["chat", "providers", "mcp", "presets"];

export interface Route {
  view: View;
  /** Selected item of the view (session / provider / server / preset id, or "new"). */
  id: string | null;
}

/** `#/chat/<id>`, `#/providers`, … (the pre-workbench `#/s/<id>` still works). */
function parse(hash: string): Route {
  const legacy = hash.match(/^#\/s\/(.+)$/);
  if (legacy) return { view: "chat", id: legacy[1] };
  const [, view, id] = hash.match(/^#\/([^/]+)(?:\/(.+))?$/) ?? [];
  return VIEWS.includes(view as View) ? { view: view as View, id: id ?? null } : { view: "chat", id: null };
}

export const href = (view: View, id?: string | null) => `#/${view}${id ? `/${id}` : ""}`;

export function navigate(view: View, id?: string | null) {
  window.location.hash = href(view, id).slice(1);
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
