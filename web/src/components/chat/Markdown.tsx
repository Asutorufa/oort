import { memo, useState, type ComponentProps } from "react";
import ReactMarkdown from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import remarkGfm from "remark-gfm";
import { copyText } from "../common";

interface HastNode {
  type: string;
  value?: string;
  tagName?: string;
  properties?: { className?: unknown };
  children?: HastNode[];
}

const hastText = (n: HastNode | undefined): string =>
  !n ? "" : n.type === "text" ? (n.value ?? "") : (n.children ?? []).map(hastText).join("");

/** `<pre>` with the language and a copy button (the raw text comes from the source AST). */
function CodeBlock({ node, children, ...rest }: ComponentProps<"pre"> & { node?: HastNode }) {
  const [copied, setCopied] = useState(false);
  const code = node?.children?.find((c) => c.tagName === "code");
  const lang = ((code?.properties?.className as string[] | undefined) ?? [])
    .find((c) => c.startsWith("language-"))
    ?.slice("language-".length);
  const text = hastText(code).replace(/\n$/, "");
  return (
    <div className="code-block">
      <div className="code-head">
        <span className="muted">{lang ?? "text"}</span>
        <button
          className="small ghost"
          onClick={async () => {
            await copyText(text);
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1200);
          }}
        >
          {copied ? "copied" : "copy"}
        </button>
      </div>
      <pre {...rest}>{children}</pre>
    </div>
  );
}

const components = {
  pre: CodeBlock,
  a: (props: ComponentProps<"a">) => <a {...props} target="_blank" rel="noreferrer noopener" />,
};

/** GitHub-flavored Markdown with highlighted code blocks. Raw HTML is not rendered. */
export default memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="markdown">
      <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[[rehypeHighlight, { detect: true, ignoreMissing: true }]]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
});
