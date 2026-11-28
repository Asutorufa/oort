import type { ReactNode } from "react";

/** Shared frame of every widget. */
export function WidgetCard({ title, kind, status, children }: { title?: string; kind: string; status?: ReactNode; children: ReactNode }) {
  return (
    <section className={`widget widget-${kind}`}>
      {(title || status) && (
        <header className="widget-head">
          {title && <h4>{title}</h4>}
          <span className="spacer" />
          {status}
        </header>
      )}
      <div className="widget-body">{children}</div>
    </section>
  );
}

export function Done({ children }: { children: ReactNode }) {
  return <span className="pill ok">✓ {children}</span>;
}
