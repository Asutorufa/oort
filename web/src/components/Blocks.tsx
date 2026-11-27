import type { ContentPart, Message, Usage } from "../types";

const json = (v: unknown) => JSON.stringify(v, null, 2);

/** One content block. Thinking / tool blocks are collapsible. */
export function Block({ part }: { part: ContentPart }) {
  switch (part.kind) {
    case "text":
      return <div className="text">{part.text}</div>;
    case "thinking":
      return (
        <details className="block thinking">
          <summary>
            Thinking{part.signature ? " · signed" : ""} ({part.thinking.length} chars)
          </summary>
          <pre>{part.thinking || "(redacted)"}</pre>
        </details>
      );
    case "tool_call":
      return (
        <details className="block tool-call">
          <summary>
            ToolUse <b>{part.name}</b> <code>{part.id}</code>
          </summary>
          <pre>{json(part.arguments)}</pre>
        </details>
      );
    case "tool_result":
      return (
        <details className={`block tool-result ${part.is_error ? "error" : ""}`} open={part.is_error}>
          <summary>
            ToolResult <b>{part.name ?? ""}</b> <code>{part.tool_call_id}</code>
            {part.is_error && <span className="badge error">is_error</span>}
          </summary>
          <pre>{part.content}</pre>
        </details>
      );
    default:
      return <div className="muted">[{part.kind}]</div>;
  }
}

export function MessageView({ message, seq, usage }: { message: Message; seq?: number; usage?: Usage | null }) {
  return (
    <div className={`msg ${message.role}`}>
      <div className="msg-head">
        <span className="role">{message.role}</span>
        {seq !== undefined && <span className="muted">#{seq}</span>}
        {usage && (
          <span className="muted">
            in {usage.input_tokens} · cached {usage.cache_read_tokens ?? 0} · out {usage.output_tokens}
          </span>
        )}
      </div>
      {message.content.map((p, i) => (
        <Block key={i} part={p} />
      ))}
    </div>
  );
}
