import { useEffect, type ReactNode } from "react";

export type Tone = "ok" | "warn" | "error" | "info" | "muted";

export function Badge({ tone = "muted", children, title }: { tone?: Tone; children: ReactNode; title?: string }) {
  return (
    <span className={`badge ${tone}`} title={title}>
      {children}
    </span>
  );
}

const STATUS_TONE: Record<string, Tone> = {
  idle: "ok",
  running: "info",
  needs_approval: "warn",
  halted: "error",
};

export function StatusBadge({ status }: { status: string }) {
  return <Badge tone={STATUS_TONE[status] ?? "muted"}>{status.replace("_", " ")}</Badge>;
}

export function Dot({ tone, title }: { tone: Tone; title?: string }) {
  return <span className={`dot ${tone}`} title={title} />;
}

export function ErrorBar({ error, onDismiss }: { error: string | null; onDismiss?: () => void }) {
  if (!error) return null;
  return (
    <div className="error-bar" role="alert">
      <span>{error}</span>
      {onDismiss && (
        <button className="ghost" onClick={onDismiss} aria-label="dismiss">
          ✕
        </button>
      )}
    </div>
  );
}

export function Modal({ title, onClose, children }: { title: ReactNode; onClose: () => void; children: ReactNode }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="modal" role="dialog" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h3>{title}</h3>
          <button className="ghost" onClick={onClose} aria-label="close">
            ✕
          </button>
        </div>
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <div className="empty muted">{children}</div>;
}

export const fmtMs = (ms: number | null | undefined) =>
  ms == null ? "—" : ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(ms < 10000 ? 2 : 1)} s`;
export const pct = (r: number) => `${(r * 100).toFixed(1)}%`;
export const fmtTime = (ms: number) => new Date(ms).toLocaleString();

/** Small JSON block. */
export function JsonView({ value, className }: { value: unknown; className?: string }) {
  return <pre className={`json ${className ?? ""}`}>{JSON.stringify(value, null, 2)}</pre>;
}

/** Copies `text`; the label flips to "copied" briefly. */
export async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
}
