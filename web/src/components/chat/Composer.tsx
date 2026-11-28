import { useMemo, useRef, useState } from "react";
import { ArrowUpIcon, StopIcon, XIcon } from "../../brand/Icons";
import { APPROVAL_POLICIES, type ApprovalPolicy, type CommandInfo, type InvokedSkill } from "../../api/types";

/** Japanese descriptions for the builtin commands (the server's text is English). */
const COMMAND_TEXT: Record<string, string> = {
  compact: "会話履歴を要約してコンテキストを整理",
  approval: "ツール実行の承認ポリシーを切り替え",
  stop: "実行中の応答をすぐに停止",
  export: "この会話を JSON で書き出し",
  new: "新しい会話を開始（プリセットを指定可）",
};
const describe = (c: CommandInfo) => (c.kind === "command" ? (COMMAND_TEXT[c.name] ?? c.description) : c.description);

/** Commands that run as soon as they are picked (no argument to type). */
const INSTANT = new Set(["compact", "stop", "export"]);

/**
 * Floating input dock.
 *
 * `/` opens a palette with two groups — skills and system commands. Picking a skill attaches it
 * as a removable chip (sent with the next message); typing `/skill args` and Enter sends it
 * directly. System commands run immediately (no model call). While the agent works, the two
 * ways to talk are explicit: queue (Steer) and interrupt.
 */
export function Composer({
  busy,
  disabled,
  commands,
  policy,
  onPolicy,
  onSend,
  onInterrupt,
  onCommand,
  onStop,
  placeholder,
  autoFocus,
}: {
  busy: boolean;
  disabled?: boolean;
  commands: CommandInfo[];
  policy?: ApprovalPolicy;
  onPolicy?: (p: ApprovalPolicy) => void;
  onSend: (text: string, skills: InvokedSkill[]) => Promise<void>;
  onInterrupt?: (text: string, skills: InvokedSkill[]) => Promise<void>;
  onCommand?: (name: string, args: string) => Promise<void>;
  onStop?: () => void;
  placeholder?: string;
  autoFocus?: boolean;
}) {
  const [text, setText] = useState("");
  const [chips, setChips] = useState<CommandInfo[]>([]);
  const [sel, setSel] = useState(0);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const area = useRef<HTMLTextAreaElement>(null);

  const query = /^\/([a-z0-9-]*)$/.exec(text)?.[1];
  const groups = useMemo(() => {
    if (query === undefined || dismissed === text) return null;
    const match = (c: CommandInfo) => c.name.includes(query) && !chips.some((x) => x.name === c.name);
    const rank = (c: CommandInfo) => (c.name.startsWith(query) ? 0 : 1);
    const pick = (kind: CommandInfo["kind"]) => commands.filter((c) => c.kind === kind && match(c)).sort((a, b) => rank(a) - rank(b));
    return { skills: pick("skill").slice(0, 8), system: pick("command") };
  }, [query, commands, chips, dismissed, text]);
  const flat = groups ? [...groups.skills, ...groups.system] : [];

  const resize = () => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 220)}px`;
  };

  const runCommand = async (name: string, args: string) => {
    if (!onCommand) return;
    setSending(true);
    try {
      await onCommand(name, args);
      setText("");
    } catch {
      // The caller shows the error.
    } finally {
      setSending(false);
    }
  };

  const choose = (c: CommandInfo) => {
    setSel(0);
    if (c.kind === "skill") {
      setChips([...chips, c]);
      setText("");
    } else if (INSTANT.has(c.name) && !c.argument_hint) {
      runCommand(c.name, "");
    } else {
      setText(`/${c.name} `);
    }
    area.current?.focus();
  };

  const submit = async (how: "send" | "interrupt") => {
    const t = text.trim();
    // `/command args` typed in full runs the command.
    const typed = /^\/([a-z0-9-]+)(?:\s+(.*))?$/s.exec(t);
    const cmd = typed && commands.find((c) => c.kind === "command" && c.name === typed[1]);
    if (cmd) return runCommand(cmd.name, typed[2]?.trim() ?? "");
    if ((!t && chips.length === 0) || sending) return;
    const skills = chips.map((c) => ({ name: c.name }));
    setSending(true);
    try {
      await (how === "interrupt" && onInterrupt ? onInterrupt(t, skills) : onSend(t, skills));
      setText("");
      setChips([]);
      requestAnimationFrame(resize);
    } catch {
      // The caller shows the error; the text stays so nothing is lost.
    } finally {
      setSending(false);
      area.current?.focus();
    }
  };

  const canSend = (text.trim() !== "" || chips.length > 0) && !sending && !disabled;
  const option = (c: CommandInfo) => {
    const i = flat.indexOf(c);
    return (
      <li key={`${c.kind}:${c.name}`} className={i === sel ? "active" : ""} onMouseEnter={() => setSel(i)} onMouseDown={(e) => (e.preventDefault(), choose(c))}>
        <span className="slash-name">/{c.name}</span>
        <span className="slash-hint">{c.argument_hint ?? ""}</span>
        <span className="slash-desc">
          {describe(c)}
          {c.subagent && <span className="pill tiny">Subagent</span>}
        </span>
      </li>
    );
  };

  return (
    <div className="dock-wrap">
      <div className={`dock ${disabled ? "disabled" : ""}`}>
        {groups && flat.length > 0 && (
          <div className="slash-menu" role="listbox">
            {groups.skills.length > 0 && (
              <>
                <div className="slash-group">スキル</div>
                <ul>{groups.skills.map(option)}</ul>
              </>
            )}
            {groups.system.length > 0 && (
              <>
                <div className="slash-group">システムコマンド</div>
                <ul>{groups.system.map(option)}</ul>
              </>
            )}
            <div className="slash-foot">
              <kbd>↑</kbd>
              <kbd>↓</kbd> 選択 <kbd>Enter</kbd> 決定 <kbd>Esc</kbd> 閉じる
            </div>
          </div>
        )}
        {chips.length > 0 && (
          <div className="attached">
            {chips.map((c) => (
              <span key={c.name} className="attached-chip" title={c.description}>
                /{c.name}
                {c.subagent && <span className="pill tiny">Subagent</span>}
                <button aria-label={`${c.name} を外す`} onClick={() => setChips(chips.filter((x) => x.name !== c.name))}>
                  <XIcon size={12} stroke={2} />
                </button>
              </span>
            ))}
            {chips[chips.length - 1]?.argument_hint && <span className="muted">本文に {chips[chips.length - 1].argument_hint} を書いてください</span>}
          </div>
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
            resize();
          }}
          onKeyDown={(e) => {
            if (flat.length) {
              if (e.key === "ArrowDown") return e.preventDefault(), setSel((sel + 1) % flat.length);
              if (e.key === "ArrowUp") return e.preventDefault(), setSel((sel - 1 + flat.length) % flat.length);
              if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing)) return e.preventDefault(), choose(flat[sel]);
              if (e.key === "Escape") return e.preventDefault(), setDismissed(text);
            }
            if (e.key === "Backspace" && text === "" && chips.length) setChips(chips.slice(0, -1));
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
              {!canSend && onStop && (
                <button className="btn send stop" title="停止" aria-label="停止" onClick={onStop}>
                  <StopIcon size={14} />
                </button>
              )}
              {canSend && (
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
            <button className="btn send" disabled={!canSend} onClick={() => submit("send")} aria-label="送信">
              <ArrowUpIcon size={16} stroke={2} />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
