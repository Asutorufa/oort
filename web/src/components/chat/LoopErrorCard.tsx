import { useEffect, useState } from "react";
import type { LoopError } from "../../api/types";
import { AlertIcon } from "../../brand/Icons";
import { href } from "../../hooks/useHashRoute";

interface Explained {
  title: string;
  body: string;
  /** Points at the model settings (keys, permissions). */
  settings?: boolean;
}

/** Turns a provider error into plain Japanese; the raw text stays available under "詳細". */
export function explainLoopError(message: string): Explained & { status: number | null; hintSecs: number | null; quota: string | null } {
  const status = Number(/HTTP (\d{3})/.exec(message)?.[1] ?? NaN);
  const code = Number.isNaN(status) ? null : status;
  const hint = /retry in ([\d.]+)\s*s/i.exec(message)?.[1] ?? /retryDelay"\s*:\s*"([\d.]+)s/.exec(message)?.[1];
  const q = /limit:\s*(\d+),\s*model:\s*([\w.\-/]+)/.exec(message);
  const perMinute = /per minute/i.test(message);
  const quota = q ? `上限 ${Number(q[1]).toLocaleString()}${perMinute ? " / 分" : ""} · モデル ${q[2]}` : null;
  const lower = message.toLowerCase();
  let e: Explained;
  if (code === 429 || /resource_exhausted|rate limit|quota/.test(lower))
    e = { title: "プロバイダーの利用上限に達しました", body: "短い時間に送ったリクエストやトークンが多すぎます。少し待ってから再試行してください。" };
  else if (code === 401 || code === 403)
    e = { title: "API キーが受け付けられませんでした", body: "モデル設定で API キーと権限を確認してください。", settings: true };
  else if (code === 404) e = { title: "モデルが見つかりませんでした", body: "モデル名がプロバイダーで使えるか確認してください。", settings: true };
  else if ((code !== null && code >= 500) || /overloaded|unavailable/.test(lower))
    e = { title: "プロバイダー側で一時的な障害が起きています", body: "しばらくしてから再試行してください。" };
  else if (/^llm error: (http error|stream error)/.test(lower))
    e = { title: "プロバイダーに接続できませんでした", body: "ネットワークの状態を確認して、再試行してください。" };
  else if (code === 400) e = { title: "リクエストが拒否されました", body: "詳細を確認してください。設定やモデルを変えると解決することがあります。" };
  else e = { title: "応答を生成できませんでした", body: "再試行するか、詳細を確認してください。" };
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
  if (error.retry_at != null) note = left > 0 ? `${left} 秒後に自動で再試行します（${error.attempt + 1} / ${error.max_attempts} 回目）` : "自動で再試行しています…";
  else if (exhausted) note = `自動再試行を ${error.attempt} 回行いましたが、解決しませんでした`;
  else if (left > 0) note = `あと ${left} 秒ほどで再試行できます`;

  return (
    <section className="widget loop-error" role="alert">
      <header className="widget-head">
        <span className="loop-error-icon">
          <AlertIcon size={16} />
        </span>
        <div className="widget-titles">
          <h4>{x.title}</h4>
          <p className="widget-sub">
            {x.body}
            {x.settings && (
              <>
                {" "}
                <a href={href("models")}>モデル設定を開く</a>
              </>
            )}
          </p>
        </div>
        <span className="spacer" />
        {x.status != null && <span className="pill">HTTP {x.status}</span>}
      </header>
      <div className="widget-body">
        {x.quota && <div className="loop-error-quota">{x.quota}</div>}
        {open && <pre className="loop-error-raw">{error.message}</pre>}
      </div>
      <div className="decision">
        <button className="text-btn" onClick={() => setOpen(!open)}>
          {open ? "詳細を閉じる" : "詳細を表示"}
        </button>
        {note && <span className="loop-error-note">{note}</span>}
        <span className="spacer" />
        {error.retry_at != null && (
          <button className="btn" disabled={busy} onClick={() => act(onCancelAuto)}>
            自動再試行をやめる
          </button>
        )}
        <button className="btn primary" disabled={busy} onClick={() => act(onRetry)}>
          {error.retry_at != null ? "今すぐ再試行" : "再試行"}
        </button>
      </div>
    </section>
  );
}
