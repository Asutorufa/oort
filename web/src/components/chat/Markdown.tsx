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
  // Short untagged output ("Wrote a.txt (+1 -1).", a one-word answer) is a result line, not
  // source code: no language label, no copy bar, no highlighting.
  if (!lang && text.split("\n").length <= 3 && text.length <= 240) {
    return <div className="result-line">{text}</div>;
  }
  return (
    <div className="code-block">
      <div className="code-head">
        <span>{lang ?? ""}</span>
        <button
          className="text-btn"
          onClick={async () => {
            await copyText(text);
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1200);
          }}
        >
          {copied ? "コピーしました" : "コピー"}
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
      <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[[rehypeHighlight, { detect: false, ignoreMissing: true }]]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
});
