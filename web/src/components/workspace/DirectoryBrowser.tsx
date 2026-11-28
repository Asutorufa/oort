import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { errorText } from "../../api";
import { workspaces, type DirListing, type RecentWorkspace } from "../../api/workspaces";
import { ChevronRightIcon, FolderIcon, StarIcon, XIcon } from "../../brand/Icons";
import { useT } from "../../i18n";
import { ErrorBar } from "../common";
import { CLOSE_POPOVERS } from "../ui/Popover";
import { Switch } from "../ui";
import { baseName, breadcrumbs } from "./paths";

/** Modal folder browser over the server's filesystem. */
export function DirectoryBrowser({
  initialPath,
  starred,
  container,
  onSelect,
  onClose,
}: {
  initialPath: string;
  starred: RecentWorkspace[];
  /** Where to mount: inside settings the dialog must be part of that layer. */
  container: Element;
  onSelect: (path: string) => void;
  onClose: () => void;
}) {
  const t = useT();
  const [path, setPath] = useState(initialPath);
  const [hidden, setHidden] = useState(false);
  const [listing, setListing] = useState<DirListing | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const crumbs = useRef<HTMLElement>(null);

  useEffect(() => {
    let live = true;
    setLoading(true);
    workspaces.list(path, { includeHidden: hidden }).then(
      (l) => {
        if (!live) return;
        // A typed path that does not exist: start from the home folder instead.
        if (!l.is_dir && path) return setPath("");
        setListing(l);
        setError(null);
        setLoading(false);
      },
      (e) => {
        if (!live) return;
        setError(errorText(e));
        setLoading(false);
      },
    );
    return () => {
      live = false;
    };
  }, [path, hidden]);

  const current = listing?.current_path;
  useEffect(() => {
    list.current?.scrollTo({ top: 0 });
    crumbs.current?.scrollTo({ left: crumbs.current.scrollWidth });
  }, [current]);

  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    dialog.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      } else if (e.key === "Tab") {
        // Focus stays inside the dialog.
        const items = [...(dialog.current?.querySelectorAll<HTMLElement>("button:not(:disabled)") ?? [])];
        const first = items[0];
        const last = items[items.length - 1];
        const inside = dialog.current?.contains(document.activeElement) && document.activeElement !== dialog.current;
        if (e.shiftKey && (!inside || document.activeElement === first)) {
          e.preventDefault();
          last?.focus();
        } else if (!e.shiftKey && (!inside || document.activeElement === last)) {
          e.preventDefault();
          first?.focus();
        }
      }
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener(CLOSE_POPOVERS, onClose);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener(CLOSE_POPOVERS, onClose);
      before?.focus?.({ preventScroll: true });
    };
  }, [onClose]);

  const chips = listing
    ? [
        ...(listing.home_dir ? [{ key: "home", label: t("workspace.home"), path: listing.home_dir, star: false }] : []),
        { key: "cwd", label: t("workspace.cwd"), path: listing.cwd, star: false },
        ...starred.filter((s) => s.exists).map((s) => ({ key: s.path, label: baseName(s.path), path: s.path, star: true })),
      ]
    : [];

  return createPortal(
    <div className="modal-backdrop wsp-backdrop" onMouseDown={onClose}>
      <div ref={dialog} className="modal wsp-modal" role="dialog" aria-modal="true" aria-label={t("workspace.title")} tabIndex={-1} onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h3>{t("workspace.title")}</h3>
          <button type="button" className="wsp-icon-btn" onClick={onClose} aria-label={t("common.close")} title={t("common.close")}>
            <XIcon size={14} />
          </button>
        </div>

        <div className="wsp-nav">
          <div className="wsp-chips">
            {chips.map((c) => (
              <button type="button" key={c.key} className={`wsp-chip ${c.path === current ? "on" : ""}`} title={c.path} onClick={() => setPath(c.path)}>
                {c.star && <StarIcon size={11} filled />}
                <span>{c.label}</span>
              </button>
            ))}
          </div>
          <nav ref={crumbs} className="wsp-crumbs" aria-label={t("workspace.breadcrumb")}>
            {current &&
              breadcrumbs(current, listing.home_dir).map((c, i, all) => (
                <span key={c.path} className="wsp-crumb">
                  {i > 0 && <span className="wsp-crumb-sep">›</span>}
                  <button type="button" aria-current={i === all.length - 1 ? "location" : undefined} onClick={() => setPath(c.path)}>
                    {c.label}
                  </button>
                </span>
              ))}
          </nav>
        </div>

        <div ref={list} className="wsp-dirs" aria-busy={loading}>
          <ErrorBar error={error} />
          {listing?.parent_path && (
            <button type="button" className="wsp-dir" onClick={() => setPath(listing.parent_path!)} title={t("workspace.parent")}>
              <span className="wsp-dir-main">
                <FolderIcon size={14} />
                <span className="wsp-dir-name">..</span>
                <span className="wsp-dir-note">{t("workspace.parent")}</span>
              </span>
            </button>
          )}
          {listing?.entries.map((e) => (
            <button type="button" key={e.path} className={`wsp-dir ${e.is_hidden ? "is-hidden" : ""}`} onClick={() => setPath(e.path)}>
              <span className="wsp-dir-main">
                <FolderIcon size={14} />
                <span className="wsp-dir-name">{e.name}</span>
              </span>
              <span className="wsp-dir-side">
                {e.is_git_repo && <span className="wsp-git">git</span>}
                <ChevronRightIcon size={13} />
              </span>
            </button>
          ))}
          {listing && listing.entries.length === 0 && (
            <div className="wsp-note">{t(listing.readable ? "workspace.noSubfolders" : "workspace.unreadable")}</div>
          )}
          {listing?.truncated && <div className="wsp-note">{t("workspace.truncated", { count: listing.entries.length })}</div>}
          {!listing && loading && <div className="wsp-note">{t("common.loading")}</div>}
        </div>

        <div className="wsp-footer">
          <Switch size="sm" className="wsp-hidden-switch" checked={hidden} onChange={setHidden} label={t("workspace.showHidden")} />
          <div className="wsp-actions">
            <button type="button" className="btn" onClick={onClose}>
              {t("common.cancel")}
            </button>
            <button type="button" className="btn primary" disabled={loading || !listing?.is_dir} onClick={() => current && onSelect(current)}>
              {t("workspace.select")}
            </button>
          </div>
        </div>
      </div>
    </div>,
    container,
  );
}
