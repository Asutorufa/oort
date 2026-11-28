import { useEffect, useRef, useState } from "react";
import type { Provider } from "../../api/types";

/** One pill showing "provider · model"; opens a small popover to change either. */
export function ModelPicker({
  providers,
  provider,
  model,
  onProvider,
  onModel,
}: {
  providers: Provider[];
  provider: Provider | undefined;
  model: string;
  onProvider: (id: string) => void;
  onModel: (m: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !box.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);
  const shownModel = model.trim() || provider?.default_model || "";

  return (
    <div className="picker" ref={box}>
      <button className={`picker-pill ${open ? "open" : ""}`} onClick={() => setOpen(!open)} aria-haspopup="dialog" aria-expanded={open}>
        <span className="picker-dot" />
        <span className="picker-provider">{provider?.name ?? "モデルを選択"}</span>
        {shownModel && <span className="picker-model">{shownModel}</span>}
        <svg className="chev" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
          <path d="M3 4.5 6 7.5 9 4.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>
      {open && (
        <div className="picker-pop" role="dialog" aria-label="モデルを選択">
          <div className="picker-label">プロバイダ</div>
          <ul>
            {providers.map((p) => (
              <li key={p.id}>
                <button
                  className={`picker-item ${p.id === provider?.id ? "on" : ""}`}
                  onClick={() => {
                    onProvider(p.id);
                    onModel("");
                  }}
                >
                  <span className="picker-name">{p.name}</span>
                  <span className="muted">{p.default_model || p.kind}</span>
                  {p.id === provider?.id && <span className="check-mark">✓</span>}
                </button>
              </li>
            ))}
          </ul>
          <div className="picker-label">モデル</div>
          <input
            value={model}
            placeholder={provider?.default_model ? `${provider.default_model}（既定）` : "モデル名"}
            onChange={(e) => onModel(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && setOpen(false)}
          />
        </div>
      )}
    </div>
  );
}
