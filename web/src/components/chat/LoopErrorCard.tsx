import { useEffect, useState } from "react";
import type { LoopError } from "../../api/types";
import { AlertIcon } from "../../brand/Icons";
import { href } from "../../hooks/useHashRoute";
import { useI18n, type Key } from "../../i18n";

interface Explained {
  title: Key;
  body: Key;
  /** Points at the model settings (keys, permissions). */
  settings?: boolean;
}

/** Turns a provider error into a plain explanation; the raw text stays available under details. */
export function explainLoopError(message: string): Explained & { status: number | null; hintSecs: number | null; quota: { limit: number; perMinute: boolean; model: string } | null } {
  const status = Number(/HTTP (\d{3})/.exec(message)?.[1] ?? NaN);
  const code = Number.isNaN(status) ? null : status;
  const hint = /retry in ([\d.]+)\s*s/i.exec(message)?.[1] ?? /retryDelay"\s*:\s*"([\d.]+)s/.exec(message)?.[1];
  const q = /limit:\s*(\d+),\s*model:\s*([\w.\-/]+)/.exec(message);
  const quota = q ? { limit: Number(q[1]), perMinute: /per minute/i.test(message), model: q[2] } : null;
  const lower = message.toLowerCase();
  let e: Explained;
  if (code === 429 || /resource_exhausted|rate limit|quota/.test(lower)) e = { title: "loop.rateLimit", body: "loop.rateLimit.body" };
  else if (code === 401 || code === 403) e = { title: "loop.auth", body: "loop.auth.body", settings: true };
  else if (code === 404) e = { title: "loop.notFound", body: "loop.notFound.body", settings: true };
  else if ((code !== null && code >= 500) || /overloaded|unavailable/.test(lower)) e = { title: "loop.server", body: "loop.server.body" };
  else if (/^llm error: (http error|stream error)/.test(lower)) e = { title: "loop.network", body: "loop.network.body" };
  else if (code === 400) e = { title: "loop.rejected", body: "loop.rejected.body" };
  else e = { title: "loop.generic", body: "loop.generic.body" };
  return { ...e, status: code, hintSecs: hint ? Number(hint) : null, quota };
}

function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const t = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(t);
  }, [active]);
  return now;
}

/** The loop stopped with an error: what happened, when a retry makes sense, and the retry itself. */
export function LoopErrorCard({ error, onRetry, onCancelAuto }: { error: LoopError; onRetry: () => Promise<void>; onCancelAuto: () => Promise<void> }) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const { t, fmt } = useI18n();
  const x = explainLoopError(error.message);
  const waitUntil = error.retry_at ?? (x.hintSecs != null ? error.at + x.hintSecs * 1000 : null);
  const now = useNow(waitUntil != null && waitUntil > Date.now());
  const left = waitUntil != null ? Math.max(0, Math.ceil((waitUntil - now) / 1000)) : 0;
  const exhausted = error.retry_at == null && error.attempt > 0 && error.attempt >= error.max_attempts;
  const act = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
    } finally {
      setBusy(false);
    }
  };

  let note: string | null = null;
  if (error.retry_at != null) note = left > 0 ? t("loop.autoIn", { secs: left, n: error.attempt + 1, max: error.max_attempts }) : t("loop.autoNow");
  else if (exhausted) note = t("loop.exhausted", { count: error.attempt });
  else if (left > 0) note = t("loop.waitHint", { secs: left });

  return (
    <section className="widget loop-error" role="alert">
      <header className="widget-head">
        <span className="loop-error-icon">
          <AlertIcon size={16} />
        </span>
        <div className="widget-titles">
          <h4>{t(x.title)}</h4>
          <p className="widget-sub">
            {t(x.body)}
            {x.settings && (
              <>
                {" "}
                <a href={href("models")}>{t("loop.openModels")}</a>
              </>
            )}
          </p>
        </div>
        <span className="spacer" />
        {x.status != null && <span className="pill">HTTP {x.status}</span>}
      </header>
      <div className="widget-body">
        {x.quota && (
          <div className="loop-error-quota">
            {t("loop.quota", {
              limit: x.quota.perMinute ? t("loop.perMinute", { n: fmt.number(x.quota.limit) }) : fmt.number(x.quota.limit),
              model: x.quota.model,
            })}
          </div>
        )}
        {open && <pre className="loop-error-raw">{error.message}</pre>}
      </div>
      <div className="decision">
        <button className="text-btn" onClick={() => setOpen(!open)}>
          {open ? t("common.hideDetails") : t("common.showDetails")}
        </button>
        {note && <span className="loop-error-note">{note}</span>}
        <span className="spacer" />
        {error.retry_at != null && (
          <button className="btn" disabled={busy} onClick={() => act(onCancelAuto)}>
            {t("loop.stopAuto")}
          </button>
        )}
        <button className="btn primary" disabled={busy} onClick={() => act(onRetry)}>
          {error.retry_at != null ? t("loop.retryNow") : t("loop.retry")}
        </button>
      </div>
    </section>
  );
}
