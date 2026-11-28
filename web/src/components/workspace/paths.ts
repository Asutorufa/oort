/** Text handling for server paths (`/` separated). */

const trim = (p: string) => (p.length > 1 ? p.replace(/\/+$/, "") || "/" : p);

/** `/home/me/code` → `~/code`. */
export function shorten(path: string, home: string | null): string {
  if (!home || home === "/") return path;
  if (path === home) return "~";
  return path.startsWith(`${home}/`) ? `~${path.slice(home.length)}` : path;
}

/** The absolute form of what was typed, for comparing with stored paths. */
export function expand(text: string, home: string | null): string {
  const p = text.trim();
  if (home && (p === "~" || p.startsWith("~/"))) return trim(home + p.slice(1));
  return trim(p);
}

export const baseName = (path: string) => trim(path).split("/").pop() || "/";

export const stripSlash = (text: string) => trim(text.trim());

/** `~/Doc` → the folder to list (`~/`) and the name being typed (`Doc`). */
export function splitForCompletion(text: string): { parent: string; partial: string } | null {
  if (text === "~") return { parent: "~/", partial: "" };
  const i = text.lastIndexOf("/");
  return i < 0 ? null : { parent: text.slice(0, i + 1), partial: text.slice(i + 1) };
}

export interface Crumb {
  label: string;
  path: string;
}

/** Clickable ancestors of an absolute path, starting at `~` inside the home folder. */
export function breadcrumbs(path: string, home: string | null): Crumb[] {
  const inHome = !!home && home !== "/" && (path === home || path.startsWith(`${home}/`));
  let at = inHome ? home! : "";
  const crumbs: Crumb[] = [inHome ? { label: "~", path: home! } : { label: "/", path: "/" }];
  for (const part of path.slice(at.length).split("/").filter(Boolean)) {
    at = `${at}/${part}`;
    crumbs.push({ label: part, path: at });
  }
  return crumbs;
}
