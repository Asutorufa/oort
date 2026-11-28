/**
 * An assistant turn as a strictly chronological list of blocks.
 *
 * History and the live stream go through the same function: a step still streaming is first
 * turned into the message it will become (`toMessages`), so a reload cannot reorder, merge or
 * re-group anything.
 */
import type { ContentPart, PendingApproval, RawMessage, ToolCall } from "../../api/types";
import type { LiveStep } from "../../hooks/useSessionStream";

export type BlockStatus = "streaming" | "done";

export interface ThinkingBlock {
  type: "thinking";
  id: string;
  text: string;
  status: BlockStatus;
  durationMs: number | null;
  /** Browser clock, only while streaming. */
  startedAt: number | null;
}

export interface TextBlock {
  type: "text";
  id: string;
  text: string;
  status: BlockStatus;
}

/** Consecutive ordinary tool calls (nothing else between them). */
export interface ToolGroupBlock {
  type: "tool_group";
  id: string;
  calls: ToolCall[];
}

/** Calls shown as their own cards. */
export interface CardBlock {
  type: "subagent" | "ask_user" | "widget" | "mcp_configured";
  id: string;
  seq: number;
  call: ToolCall;
}

export interface ApprovalBlock {
  type: "approval";
  id: string;
  pending: PendingApproval;
}

export type TurnBlock = ThinkingBlock | TextBlock | ToolGroupBlock | CardBlock | ApprovalBlock;
export type WorkBlock = ThinkingBlock | ToolGroupBlock;

export interface SourceMessage {
  seq: number;
  role: RawMessage["role"];
  content: ContentPart[];
  thinkingMs: number | null;
  /** Set while the message is still streaming. */
  streaming?: { thinking: boolean; startedAt: number | null };
}

const CARDS: Record<string, CardBlock["type"]> = {
  spawn_subagent: "subagent",
  ask_user: "ask_user",
  render_widget: "widget",
  configure_mcp_server: "mcp_configured",
};
/** Shown elsewhere (the plan tracker in the header). */
const HIDDEN = new Set(["update_plan"]);

export const fromRaw = (m: RawMessage): SourceMessage => ({
  seq: m.seq,
  role: m.role,
  content: m.content,
  thinkingMs: m.thinking_ms ?? null,
});

export function fromLive(step: LiveStep): SourceMessage {
  const parts: ContentPart[] = step.parts ?? [
    ...(step.thinking ? [{ kind: "thinking", thinking: step.thinking } as const] : []),
    ...(step.text ? [{ kind: "text", text: step.text } as const] : []),
  ];
  return {
    seq: step.seq,
    role: "assistant",
    content: parts,
    thinkingMs: step.thinkingMs,
    streaming: step.parts ? undefined : { thinking: step.thinkingMs == null, startedAt: step.thinkingStartedAt },
  };
}

/**
 * @param shown whether a `configure_mcp_server` call has a card to show (it succeeded)
 */
export function buildBlocks(msgs: SourceMessage[], shown: (callId: string) => boolean = () => true): TurnBlock[] {
  const out: TurnBlock[] = [];
  for (const m of msgs) {
    if (m.role !== "assistant") continue;
    const count = { thinking: 0, text: 0 };
    const blockId = (kind: "thinking" | "text") => {
      const n = count[kind]++;
      return n === 0 ? `${m.seq}:${kind}` : `${m.seq}:${kind}.${n}`;
    };
    for (const p of m.content) {
      if (p.kind === "thinking") {
        // Signature-only reasoning has nothing to show.
        if (!p.thinking.trim()) continue;
        const live = !!m.streaming?.thinking;
        out.push({
          type: "thinking",
          id: blockId("thinking"),
          text: p.thinking,
          status: live ? "streaming" : "done",
          durationMs: m.thinkingMs,
          startedAt: live ? (m.streaming?.startedAt ?? null) : null,
        });
      } else if (p.kind === "text") {
        if (!p.text.trim()) continue;
        out.push({ type: "text", id: blockId("text"), text: p.text, status: m.streaming ? "streaming" : "done" });
      } else if (p.kind === "tool_call") {
        if (HIDDEN.has(p.name)) continue;
        const card = CARDS[p.name];
        if (card) {
          if (card !== "mcp_configured" || shown(p.id)) out.push({ type: card, id: p.id, seq: m.seq, call: p });
          continue;
        }
        const last = out[out.length - 1];
        if (last?.type === "tool_group") out[out.length - 1] = { ...last, calls: [...last.calls, p] };
        else out.push({ type: "tool_group", id: p.id, calls: [p] });
      }
    }
  }
  return out;
}

export type Segment = { kind: "work"; id: string; items: WorkBlock[] } | { kind: "block"; id: string; block: Exclude<TurnBlock, WorkBlock> };

/** Folds runs of reasoning and tool calls; anything else (above all text) ends a run. */
export function segments(blocks: TurnBlock[]): Segment[] {
  const out: Segment[] = [];
  for (const b of blocks) {
    if (b.type === "thinking" || b.type === "tool_group") {
      const last = out[out.length - 1];
      if (last?.kind === "work") last.items.push(b);
      else out.push({ kind: "work", id: `work:${b.id}`, items: [b] });
    } else out.push({ kind: "block", id: b.id, block: b });
  }
  return out;
}
