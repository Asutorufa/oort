import { useMemo, useRef, useState } from "react";
import { APPROVAL_POLICIES, type ApprovalPolicy, type Skill } from "../../api/types";

interface Command {
  name: string;
  hint: string;
}

const COMMANDS: Command[] = [{ name: "compact", hint: "会話を要約してコンテキストを空ける" }];

/**
 * Floating input dock. While the agent works, the two ways to talk are explicit:
 * "キューに追記" (Steer, applied at the next safe point) and "中断して送信" (Interrupt).
 */
export function Composer({
  busy,
  disabled,
  skills,
  policy,
  onPolicy,
  onSend,
  onInterrupt,
  onStop,
  placeholder,
  autoFocus,
}: {
  busy: boolean;
  disabled?: boolean;
  skills: Skill[];
  policy?: ApprovalPolicy;
  onPolicy?: (p: ApprovalPolicy) => void;
  onSend: (text: string) => Promise<void>;
  onInterrupt?: (text: string) => Promise<void>;
  onStop?: () => void;
  placeholder?: string;
  autoFocus?: boolean;
}) {
  const [text, setText] = useState("");
  const [sel, setSel] = useState(0);
  const [sending, setSending] = useState(false);
  const area = useRef<HTMLTextAreaElement>(null);

  // `/` completion while the first word is being typed.
  const slash = /^\/([a-z0-9-]*)$/.exec(text);
  const options = useMemo(() => {
    if (!slash) return [];
    const q = slash[1];
    return [
      ...COMMANDS.filter((c) => c.name.startsWith(q)).map((c) => ({ name: c.name, hint: c.hint, kind: "command" as const })),
      ...skills.filter((s) => s.name.includes(q)).map((s) => ({ name: s.name, hint: s.description, kind: "skill" as const })),
    ].slice(0, 8);
  }, [slash?.[1], skills]); // eslint-disable-line react-hooks/exhaustive-deps

  const submit = async (how: "send" | "interrupt") => {
    const t = text.trim();
    if (!t || sending) return;
    setSending(true);
    try {
      await (how === "interrupt" && onInterrupt ? onInterrupt(t) : onSend(t));
      setText("");
      if (area.current) area.current.style.height = "auto";
    } catch {
      // The caller shows the error; the text stays so nothing is lost.
    } finally {
      setSending(false);
      area.current?.focus();
    }
  };

  const pick = (name: string) => {
    setText(`/${name} `);
    setSel(0);
    area.current?.focus();
  };

  return (
    <div className="dock-wrap">
      <div className={`dock ${disabled ? "disabled" : ""}`}>
        {options.length > 0 && (
          <ul className="slash-menu" role="listbox">
            {options.map((o, i) => (
              <li key={o.name} className={i === sel ? "active" : ""} onMouseDown={(e) => (e.preventDefault(), pick(o.name))}>
                <span className="slash-name">/{o.name}</span>
                <span className="slash-kind">{o.kind === "skill" ? "スキル" : "コマンド"}</span>
                <span className="muted ellipsis">{o.hint}</span>
              </li>
            ))}
          </ul>
        )}
        <textarea
          ref={area}
          rows={1}
          autoFocus={autoFocus}
          value={text}
          disabled={disabled}
          placeholder={placeholder ?? (busy ? "続けて入力するとキューに追記されます…" : "メッセージを入力 — / でスキル・コマンド")}
          onChange={(e) => {
            setText(e.target.value);
            setSel(0);
            const el = e.currentTarget;
            el.style.height = "auto";
            el.style.height = `${Math.min(el.scrollHeight, 220)}px`;
          }}
          onKeyDown={(e) => {
            if (options.length) {
              if (e.key === "ArrowDown") return e.preventDefault(), setSel((sel + 1) % options.length);
              if (e.key === "ArrowUp") return e.preventDefault(), setSel((sel - 1 + options.length) % options.length);
              if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) return e.preventDefault(), pick(options[sel].name);
            }
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              submit("send");
            }
          }}
        />
        <div className="dock-bar">
          {policy && onPolicy && (
            <select className="policy" value={policy} title="ツール実行の承認ポリシー" onChange={(e) => onPolicy(e.target.value as ApprovalPolicy)}>
              {APPROVAL_POLICIES.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.value === "auto_all" ? "すべて自動実行" : p.value === "auto_readonly" ? "読み取りのみ自動" : "毎回確認"}
                </option>
              ))}
            </select>
          )}
          <span className="spacer" />
          {busy ? (
            <>
              {!text.trim() && onStop && (
                <button className="btn icon" title="停止" onClick={onStop}>
                  ■
                </button>
              )}
              {text.trim() && (
                <>
                  <button className="btn ghost" disabled={sending} onClick={() => submit("interrupt")} title="いまの応答を止めて、このメッセージを送ります">
                    中断して送信
                  </button>
                  <button className="btn primary" disabled={sending} onClick={() => submit("send")} title="応答の区切りでこのメッセージを追加します">
                    キューに追記
                  </button>
                </>
              )}
            </>
          ) : (
            <button className="btn primary send" disabled={!text.trim() || sending || disabled} onClick={() => submit("send")} aria-label="送信">
              ↑
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
