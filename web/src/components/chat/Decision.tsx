import { useState } from "react";

/**
 * Footer of every approval: `却下` (quiet) and `承認して実行` (primary), right-aligned. A reason
 * is optional and lives behind "理由を添えて却下", so the default row stays two buttons.
 */
export function DecisionFooter({
  disabled,
  canApprove = true,
  onApprove,
  onDeny,
}: {
  disabled?: boolean;
  canApprove?: boolean;
  onApprove: () => Promise<void>;
  onDeny: (reason: string) => Promise<void>;
}) {
  const [withReason, setWithReason] = useState(false);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const act = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
    } finally {
      setBusy(false);
    }
  };
  const deny = () => act(() => onDeny(reason.trim() || "ユーザーが却下しました"));
  const off = busy || disabled;
  if (withReason) {
    return (
      <div className="decision reason-row">
        <input
          autoFocus
          className="reason"
          placeholder="却下する理由（エージェントに伝わります）"
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.nativeEvent.isComposing) deny();
            if (e.key === "Escape") setWithReason(false);
          }}
        />
        <button className="btn ghost" disabled={off} onClick={() => setWithReason(false)}>
          戻る
        </button>
        <button className="btn" disabled={off} onClick={deny}>
          却下する
        </button>
      </div>
    );
  }
  return (
    <div className="decision">
      <button className="text-btn" disabled={off} onClick={() => setWithReason(true)}>
        理由を添えて却下
      </button>
      <span className="spacer" />
      <button className="btn" disabled={off} onClick={deny}>
        却下
      </button>
      <button className="btn primary" disabled={off || !canApprove} onClick={() => act(onApprove)}>
        承認して実行
      </button>
    </div>
  );
}
