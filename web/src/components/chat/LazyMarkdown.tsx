import { lazy, Suspense } from "react";

// The Markdown renderer (with highlight.js grammars) is loaded on first use; text is shown
// plainly until then.
const Impl = lazy(() => import("./Markdown"));

export function Markdown({ text }: { text: string }) {
  return (
    <Suspense fallback={<div className="plain">{text}</div>}>
      <Impl text={text} />
    </Suspense>
  );
}
