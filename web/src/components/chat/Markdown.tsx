import { createContext, memo, useContext, useState, type ComponentProps } from "react";
import ReactMarkdown from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import rehypeKatex from "rehype-katex";
import remarkMath from "remark-math";
import "katex/dist/katex.min.css";
import remarkGfm from "remark-gfm";
import { copyText } from "../common";
import { prepareMath } from "./mathPrep";
import { MermaidBlock } from "./MermaidBlock";
import { useT } from "../../i18n";

const StreamingCtx = createContext<{ streaming: boolean; unclosedMermaidCode: string | null }>({
  streaming: false,
  unclosedMermaidCode: null,
});

/** Detects if the markdown ends with an unclosed ```mermaid fenced code block while streaming. */
function findUnclosedMermaid(src: string): string | null {
  const lines = src.split("\n");
  let openLang: string | null = null;
  let buf: string[] = [];
  for (const line of lines) {
    const trimmed = line.trimStart();
    if (openLang === null) {
      const m = /^```([a-zA-Z0-9_-]*)/.exec(trimmed);
      if (m) {
        openLang = m[1].toLowerCase();
        buf = [];
      }
    } else if (/^```\s*$/.test(trimmed)) {
      openLang = null;
      buf = [];
    } else {
      buf.push(line);
    }
  }
  return openLang === "mermaid" ? buf.join("\n").replace(/\n$/, "") : null;
}

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
  const t = useT();
  const { streaming, unclosedMermaidCode } = useContext(StreamingCtx);
  const code = node?.children?.find((c) => c.tagName === "code");
  const lang = ((code?.properties?.className as string[] | undefined) ?? [])
    .find((c) => c.startsWith("language-"))
    ?.slice("language-".length);
  const text = hastText(code).replace(/\n$/, "");
  if (lang?.toLowerCase() === "mermaid") {
    const isUnclosed = streaming && unclosedMermaidCode !== null && text === unclosedMermaidCode;
    return <MermaidBlock code={text} streaming={isUnclosed} />;
  }
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
          {copied ? t("common.copied") : t("common.copy")}
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

/** GitHub-flavored Markdown with math (KaTeX), Mermaid diagrams, and highlighted code blocks. */
export default memo(function Markdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  const unclosedMermaidCode = streaming ? findUnclosedMermaid(text) : null;
  return (
    <StreamingCtx.Provider value={{ streaming, unclosedMermaidCode }}>
      <div className="markdown">
        <ReactMarkdown
          remarkPlugins={[remarkGfm, remarkMath]}
          rehypePlugins={[
            [rehypeKatex, { throwOnError: false, strict: "ignore" }],
            [rehypeHighlight, { detect: false, ignoreMissing: true }],
          ]}
          components={components}
        >
          {prepareMath(text)}
        </ReactMarkdown>
      </div>
    </StreamingCtx.Provider>
  );
});
