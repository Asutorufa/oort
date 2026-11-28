/** The server-side folder browser and the recent / starred workspace folders. */

import { request } from "./client";

export interface DirEntry {
  name: string;
  path: string;
  is_git_repo: boolean;
  is_hidden: boolean;
}

export interface DirListing {
  current_path: string;
  parent_path: string | null;
  home_dir: string | null;
  cwd: string;
  exists: boolean;
  is_dir: boolean;
  readable: boolean;
  truncated: boolean;
  entries: DirEntry[];
}

export interface RecentWorkspace {
  path: string;
  starred: boolean;
  use_count: number;
  last_used_at: string;
  exists: boolean;
  is_git_repo: boolean;
}

export const workspaces = {
  list: (path = "", opts: { prefix?: string; includeHidden?: boolean } = {}) => {
    const q = new URLSearchParams({ path });
    if (opts.prefix) q.set("prefix", opts.prefix);
    if (opts.includeHidden) q.set("include_hidden", "true");
    return request<DirListing>("GET", `/fs/list?${q}`);
  },
  recent: () => request<RecentWorkspace[]>("GET", "/workspaces/recent"),
  record: (path: string) => request<RecentWorkspace>("POST", "/workspaces/recent", { path }),
  star: (path: string, starred: boolean) => request<void>("PATCH", "/workspaces/recent/star", { path, starred }),
  remove: (path: string) => request<void>("DELETE", "/workspaces/recent", { path }),
};
