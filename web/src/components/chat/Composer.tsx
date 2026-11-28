import { useMemo, useRef, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { ArrowUpIcon, BrainIcon, CubeIcon, ShieldIcon, StopIcon, XIcon } from "../../brand/Icons";
import { APPROVAL_POLICIES, REASONING_LEVELS, type ApprovalPolicy, type CommandInfo, type InvokedSkill, type ReasoningEffort } from "../../api/types";
import { popover, snappy, tap } from "../../lib/motion";
import { Select } from "../ui";
import { useT, type Key, type T } from "../../i18n";

/** Japanese descriptions for the builtin commands (the server's text is English). */
const COMMAND_TEXT: Record<string, Key> = {
  compact: "cmd.desc.compact",
  approval: "cmd.desc.approval",
  stop: "cmd.desc.stop",
  export: "cmd.desc.export",
  new: "cmd.desc.new",
};
const describe = (t: T, c: CommandInfo) => (c.kind === "command" && COMMAND_TEXT[c.name] ? t(COMMAND_TEXT[c.name]) : c.description);

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
  reasoning,
  effectiveReasoning,
  onReasoning,
  model,
  models,
  onModel,
  modelsFailed,
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
  /** The session's thinking level and what `default` currently resolves to. */
  reasoning?: ReasoningEffort;
  effectiveReasoning?: ReasoningEffort;
  onReasoning?: (r: ReasoningEffort) => void;
  model?: string;
  /** Models the user can switch to (same provider); without it the model is shown read-only. */
  models?: string[];
  onModel?: (m: string) => void;
  modelsFailed?: boolean;
  onSend: (text: string, skills: InvokedSkill[]) => Promise<void>;
  onInterrupt?: (text: string, skills: InvokedSkill[]) => Promise<void>;
  onCommand?: (name: string, args: string) => Promise<void>;
  onStop?: () => void;
  placeholder?: string;
  autoFocus?: boolean;
}) {
  const [text, setText] = useState("");
  const t = useT();
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
          {describe(t, c)}
          {c.subagent && <span className="pill tiny">Subagent</span>}
        </span>
      </li>
    );
  };

  return (
    <div className="dock-wrap">
      <div className={`dock ${disabled ? "disabled" : ""}`}>
        <AnimatePresence>
        {groups && flat.length > 0 && (
          <motion.div className="slash-menu" role="listbox" variants={popover("bottom")} initial="hidden" animate="shown" exit="exit" style={{ transformOrigin: "bottom center" }}>
            {groups.skills.length > 0 && (
              <>
                <div className="slash-group">{t("composer.skills")}</div>
                <ul>{groups.skills.map(option)}</ul>
              </>
            )}
            {groups.system.length > 0 && (
              <>
                <div className="slash-group">{t("composer.commands")}</div>
                <ul>{groups.system.map(option)}</ul>
              </>
            )}
            <div className="slash-foot">
              <kbd>↑</kbd>
              <kbd>↓</kbd> {t("composer.navigate")} <kbd>Enter</kbd> {t("composer.choose")} <kbd>Esc</kbd> {t("composer.dismiss")}
            </div>
          </motion.div>
        )}
        </AnimatePresence>
        {chips.length > 0 && (
          <div className="attached">
            {chips.map((c) => (
              <span key={c.name} className="attached-chip" title={c.description}>
                /{c.name}
                {c.subagent && <span className="pill tiny">Subagent</span>}
                <button aria-label={t("composer.removeChip", { name: c.name })} onClick={() => setChips(chips.filter((x) => x.name !== c.name))}>
                  <XIcon size={12} stroke={2} />
                </button>
              </span>
            ))}
            {chips[chips.length - 1]?.argument_hint && <span className="muted">{t("composer.argHint", { hint: chips[chips.length - 1].argument_hint ?? "" })}</span>}
          </div>
        )}
        <textarea
          ref={area}
          rows={1}
          autoFocus={autoFocus}
          value={text}
          disabled={disabled}
          placeholder={placeholder ?? (busy ? t("composer.placeholderBusy") : t("composer.placeholder"))}
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
          {reasoning && onReasoning && (
            <Select<ReasoningEffort>
              variant="pill"
              placement="top"
              ariaLabel={t("reasoning.label")}
              title={t("reasoning.pillHint")}
              icon={<BrainIcon size={13} />}
              value={reasoning}
              onChange={onReasoning}
              renderValue={(o) => {
                const eff = REASONING_LEVELS.find((l) => l.value === effectiveReasoning);
                return reasoning === "default" && eff && eff.value !== "default"
                  ? t("reasoning.pillDefault", { level: t(eff.label) })
                  : t("reasoning.pill", { level: o ? String(o.label) : "" });
              }}
              options={REASONING_LEVELS.map((l) => ({ value: l.value, label: t(l.label), description: t(l.description) }))}
            />
          )}
          {policy && onPolicy && (
            <Select<ApprovalPolicy>
              variant="pill"
              placement="top"
              ariaLabel={t("policy.label")}
              icon={<ShieldIcon size={13} />}
              value={policy}
              onChange={onPolicy}
              options={APPROVAL_POLICIES.map((p) => ({ value: p.value, label: t(p.label), description: t(p.hint) }))}
            />
          )}
          {model && onModel ? (
            <Select<string>
              variant="pill"
              placement="top"
              className="dock-model-pick"
              menuClassName="model-menu"
              ariaLabel={t("composer.model")}
              title={t("composer.modelSwitch")}
              icon={<CubeIcon size={13} />}
              value={model}
              onChange={(m) => m !== model && onModel(m)}
              renderValue={() => <span className="mono">{model}</span>}
              options={[...new Set([model, ...(models ?? [])])].map((m) => ({
                value: m,
                label: <span className="mono">{m}</span>,
                description: m === model ? (modelsFailed ? `${t("composer.modelCurrent")} · ${t("composer.modelListFailed")}` : t("composer.modelCurrent")) : undefined,
              }))}
            />
          ) : (
            model && (
              <span className="dock-model" title={t("composer.model")}>
                <CubeIcon size={13} />
                <span className="mono">{model}</span>
              </span>
            )
          )}
          <span className="spacer" />
          <AnimatePresence mode="popLayout" initial={false}>
            {busy && canSend ? (
              <motion.span key="queue" className="dock-actions" initial={{ opacity: 0, scale: 0.96 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.96 }} transition={snappy}>
                <button className="btn ghost" disabled={sending} onClick={() => submit("interrupt")} title={t("composer.interruptHint")}>
                  {t("composer.interrupt")}
                </button>
                <motion.button whileTap={tap} className="btn primary" disabled={sending} onClick={() => submit("send")} title={t("composer.queueHint")}>
                  {t("composer.queue")}
                </motion.button>
              </motion.span>
            ) : busy && onStop ? (
              <motion.button
                key="stop"
                whileTap={{ scale: 0.92 }}
                className="btn send stop"
                title={t("composer.stop")}
                aria-label={t("composer.stop")}
                onClick={onStop}
                initial={{ opacity: 0, scale: 0.6, rotate: -45 }}
                animate={{ opacity: 1, scale: 1, rotate: 0 }}
                exit={{ opacity: 0, scale: 0.6 }}
                transition={snappy}
              >
                <StopIcon size={14} />
              </motion.button>
            ) : (
              <motion.button
                key="send"
                whileTap={{ scale: 0.92 }}
                className="btn send"
                disabled={!canSend}
                onClick={() => submit("send")}
                aria-label={t("composer.send")}
                initial={{ opacity: 0, scale: 0.6 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0, scale: 0.6 }}
                transition={snappy}
              >
                <ArrowUpIcon size={16} stroke={2} />
              </motion.button>
            )}
          </AnimatePresence>
        </div>
      </div>
    </div>
  );
}
