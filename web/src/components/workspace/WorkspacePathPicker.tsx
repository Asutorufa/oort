import { useCallback, useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { workspaces, type DirListing, type RecentWorkspace } from "../../api/workspaces";
import { FolderIcon, FolderOpenIcon, StarIcon, XIcon } from "../../brand/Icons";
import { useT, type Key } from "../../i18n";
import { useToast } from "../common/Toast";
import { Popover } from "../ui";
import { DirectoryBrowser } from "./DirectoryBrowser";
import { baseName, expand, shorten, splitForCompletion, stripSlash } from "./paths";
import "./workspace.css";

const DEBOUNCE = 200;

interface Row {
  path: string;
  name: string;
  git: boolean;
  recent?: RecentWorkspace;
  section?: Key;
}

/**
 * A server folder: free text with validation, recent / starred folders, path completion and a
 * folder browser. Nothing depends on a native file dialog, so it works over a remote connection.
 */
export function WorkspacePathPicker({
  value,
  onChange,
  placeholder,
  ariaLabel,
  className = "",
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  ariaLabel?: string;
  className?: string;
}) {
  const t = useT();
  const toast = useToast();
  const id = useId();
  const box = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  /** Typed since the field was focused: completion rather than history. */
  const [typed, setTyped] = useState(false);
  const [active, setActive] = useState(-1);
  const [browsing, setBrowsing] = useState(false);
  const [recent, setRecent] = useState<RecentWorkspace[]>([]);
  const [home, setHome] = useState<string | null>(null);
  const [status, setStatus] = useState<{ value: string; ok: boolean } | null>(null);
  const [completion, setCompletion] = useState<{ value: string; listing: DirListing } | null>(null);

  const loadRecent = useCallback(() => {
    workspaces.recent().then(setRecent, () => {});
  }, []);
  useEffect(loadRecent, [loadRecent]);

  // Does the folder exist? Asked once the text has been still for a moment.
  useEffect(() => {
    if (!value.trim()) return setStatus(null);
    let live = true;
    const timer = setTimeout(() => {
      workspaces.list(value).then(
        (l) => {
          if (!live) return;
          setHome(l.home_dir);
          setStatus({ value, ok: l.is_dir });
        },
        () => live && setStatus(null),
      );
    }, DEBOUNCE);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [value]);

  // `~` shortening needs the home folder even while the field is empty.
  useEffect(() => {
    if (open && home === null) workspaces.list().then((l) => setHome(l.home_dir), () => {});
  }, [open, home]);

  const split = splitForCompletion(value);
  // A trailing slash asks for what is inside, even when the folder itself is in the history.
  const known = !value.endsWith("/") && recent.some((r) => r.path === expand(value, home));
  const completing = typed && value.trim() !== "" && !known && split !== null;

  useEffect(() => {
    const at = splitForCompletion(value);
    if (!open || !completing || !at) return;
    let live = true;
    const timer = setTimeout(() => {
      workspaces.list(at.parent, { prefix: at.partial, includeHidden: at.partial.startsWith(".") }).then(
        (listing) => {
          if (!live) return;
          setHome(listing.home_dir);
          setCompletion({ value, listing });
          // With nothing typed after the slash, Tab and Enter keep their usual meaning.
          setActive(at.partial && listing.entries.length > 0 ? 0 : -1);
        },
        () => live && setCompletion(null),
      );
    }, DEBOUNCE);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [value, open, completing]);

  // Candidates for an earlier text stay up while typing in the same folder, narrowed locally.
  const candidates = (() => {
    if (!completing || !split || !completion) return [];
    if (splitForCompletion(completion.value)?.parent !== split.parent) return [];
    const prefix = split.partial.toLowerCase();
    return completion.listing.entries.filter((e) => e.name.toLowerCase().startsWith(prefix));
  })();

  const starred = recent.filter((r) => r.starred);
  const others = recent.filter((r) => !r.starred);
  const historyRows = (list: RecentWorkspace[], section: Key): Row[] =>
    list.map((r, i) => ({ path: r.path, name: baseName(r.path), git: r.is_git_repo, recent: r, section: i === 0 ? section : undefined }));
  const rows: Row[] = completing
    ? candidates.map((e) => ({ path: e.path, name: e.name, git: e.is_git_repo }))
    : [...historyRows(starred, "workspace.starred"), ...historyRows(others, "workspace.recent")];
  const shown = open && rows.length > 0;
  const selected = Math.min(active, rows.length - 1);

  useEffect(() => {
    if (shown && selected >= 0) document.getElementById(`${id}-${selected}`)?.scrollIntoView({ block: "nearest" });
  }, [shown, selected, id]);

  const close = useCallback(() => {
    setOpen(false);
    setActive(-1);
  }, []);

  const show = () => {
    if (open) return;
    setOpen(true);
    setTyped(false);
    setActive(-1);
    loadRecent();
  };

  const pick = (row: Row) => {
    if (row.recent) {
      onChange(row.path);
      close();
      return;
    }
    // Completion keeps the style being typed (`~/…` stays `~/…`) and goes on to the next level.
    onChange(`${split!.parent}${row.name}/`);
    setActive(-1);
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.nativeEvent.isComposing) return;
    if (!open) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        show();
      }
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if ((e.key === "ArrowDown" || e.key === "ArrowUp") && rows.length > 0) {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      setActive(selected < 0 ? (step > 0 ? 0 : rows.length - 1) : (selected + step + rows.length) % rows.length);
    } else if ((e.key === "Enter" || (e.key === "Tab" && !e.shiftKey)) && shown && selected >= 0) {
      e.preventDefault();
      pick(rows[selected]);
    } else if (e.key === "Enter") {
      e.preventDefault();
      close();
    }
  };

  const change = async (op: Promise<void>) => {
    try {
      await op;
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
    loadRecent();
  };

  const select = (path: string) => {
    onChange(path);
    setBrowsing(false);
    workspaces.record(path).then(loadRecent, () => {});
  };

  const valid = status && status.value === value ? status.ok : null;

  return (
    <div className={`wsp ${className}`}>
      <div ref={box} className="wsp-field">
        <FolderIcon size={14} />
        <input
          ref={input}
          className="wsp-input"
          value={value}
          placeholder={placeholder}
          title={value || undefined}
          aria-label={ariaLabel ?? t("workspace.label")}
          role="combobox"
          aria-expanded={shown}
          aria-controls={`${id}-list`}
          aria-autocomplete="list"
          aria-activedescendant={shown && selected >= 0 ? `${id}-${selected}` : undefined}
          spellCheck={false}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          onFocus={show}
          onClick={show}
          onBlur={() => {
            close();
            if (value !== stripSlash(value)) onChange(stripSlash(value));
          }}
          onChange={(e) => {
            onChange(e.target.value);
            setOpen(true);
            setTyped(true);
            setActive(-1);
          }}
          onKeyDown={onKey}
        />
        <div className="wsp-end">
          {valid !== null && (
            <span
              className={`wsp-status ${valid ? "ok" : "missing"}`}
              role="img"
              aria-label={t(valid ? "workspace.found" : "workspace.notFound")}
              title={t(valid ? "workspace.found" : "workspace.notFound")}
            >
              <span />
            </span>
          )}
          <button
            type="button"
            className="wsp-icon-btn"
            aria-label={t("workspace.browse")}
            title={t("workspace.browse")}
            onClick={() => {
              close();
              loadRecent();
              setBrowsing(true);
            }}
          >
            <FolderOpenIcon size={14} />
          </button>
        </div>
      </div>

      <Popover anchor={box} open={shown} onClose={close} align="stretch" className="wsp-pop">
        {/* Pointer presses must not take the focus (and with it the dropdown) from the input. */}
        <div id={`${id}-list`} className="wsp-list" role="listbox" onMouseDown={(e) => e.preventDefault()}>
          {rows.map((row, i) => (
            <div key={row.path} role="presentation">
              {row.section && <div className="wsp-section">{t(row.section)}</div>}
              <div
                id={`${id}-${i}`}
                role="option"
                aria-selected={i === selected}
                className={`wsp-row ${i === selected ? "active" : ""} ${row.recent && !row.recent.exists ? "missing" : ""}`}
                title={row.recent && !row.recent.exists ? `${row.path} — ${t("workspace.notFound")}` : row.path}
                onMouseEnter={() => setActive(i)}
                onClick={() => pick(row)}
              >
                <FolderIcon size={14} />
                <span className="wsp-row-text">
                  <span className="wsp-row-name">{row.name}</span>
                  <span className="wsp-row-path">{shorten(row.path, home)}</span>
                </span>
                {row.git && <span className="wsp-git">git</span>}
                {row.recent && (
                  <span className="wsp-row-actions" onClick={(e) => e.stopPropagation()}>
                    <button
                      type="button"
                      tabIndex={-1}
                      className={`wsp-icon-btn ${row.recent.starred ? "starred" : ""}`}
                      aria-label={t(row.recent.starred ? "workspace.unstar" : "workspace.star")}
                      title={t(row.recent.starred ? "workspace.unstar" : "workspace.star")}
                      onClick={() => change(workspaces.star(row.path, !row.recent!.starred))}
                    >
                      <StarIcon size={13} filled={row.recent.starred} />
                    </button>
                    <button
                      type="button"
                      tabIndex={-1}
                      className="wsp-icon-btn"
                      aria-label={t("workspace.remove")}
                      title={t("workspace.remove")}
                      onClick={() => change(workspaces.remove(row.path))}
                    >
                      <XIcon size={13} />
                    </button>
                  </span>
                )}
              </div>
            </div>
          ))}
        </div>
      </Popover>

      {browsing && (
        <DirectoryBrowser
          initialPath={value.trim()}
          starred={starred}
          container={box.current?.closest(".settings") ?? document.body}
          onSelect={select}
          onClose={() => setBrowsing(false)}
        />
      )}
    </div>
  );
}
