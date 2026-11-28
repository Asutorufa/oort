/**
 * Prepares model output for remark-math (outside code spans and fences):
 * - `\(…\)` → `$…$` and `\[…\]` → `$$…$$` (Markdown would otherwise eat the backslashes);
 * - a `$` that starts an amount (`$5 and $10`) is escaped so prices are not typeset as math.
 *   A `$` before a digit counts as an amount when no closing `$` follows on the line, or the
 *   text up to the next `$` ends in a space (math never ends with one: `$2x$` stays math).
 */
/**
 * Fixes CommonMark's strict flanking-delimiter rules for CJK emphasis:
 * - Closing `**` preceded by punctuation and immediately followed by CJK/alphanumeric
 *   (e.g. `**结论。**白天...` or `**（注）**16:58`) fails to close in CommonMark because
 *   a right-flanking run after punctuation must be followed by whitespace or punctuation.
 * - Opening `**` preceded by CJK/alphanumeric and immediately followed by opening punctuation
 *   (e.g. `这是**「重点」**`) fails to open for the symmetric reason.
 * Inserting a Unicode hair space (`\u200A`, category `Zs`) satisfies CommonMark whitespace
 * without introducing a visible gap.
 */
function fixCjkEmphasis(t: string): string {
  if (!t.includes("*")) return t;
  return t.replace(
    /(\*{2,3})((?:[^*\n]|\*(?!\*))+?)(\1)/gu,
    (match, delim: string, inner: string, close: string, offset: number, all: string) => {
      if (/^\s|\s$/.test(inner)) return match;
      const before = offset > 0 ? all[offset - 1] : "";
      const after = offset + match.length < all.length ? all[offset + match.length] : "";
      const innerStartsPunct = /^\p{P}/u.test(inner);
      const innerEndsPunct = /\p{P}$/u.test(inner);
      const beforeWord = before !== "" && !/[\s\p{P}*]/u.test(before);
      const afterWord = after !== "" && !/[\s\p{P}*]/u.test(after);
      const pre = beforeWord && innerStartsPunct ? "\u200A" : "";
      const post = innerEndsPunct && afterWord ? "\u200A" : "";
      return `${pre}${delim}${inner}${close}${post}`;
    },
  );
}

function prepText(t: string): string {
  t = fixCjkEmphasis(t);
  if (!t.includes("$") && !t.includes("\\(") && !t.includes("\\[")) return t;
  // remark-math only displays `$$` fences that stand on their own lines.
  t = t.replace(/\\\[([\s\S]+?)\\\]/g, (_m, x: string) => `\n$$\n${x.trim()}\n$$\n`);
  t = t.replace(/\\\(([^\n]+?)\\\)/g, (_m, x: string) => `$${x.trim()}$`);
  return t.replace(/(^|[^\\$])\$(?=\d)/gm, (m: string, pre: string, offset: number, all: string) => {
    const start = offset + m.length;
    const end = all.indexOf("$", start);
    const nl = all.indexOf("\n", start);
    const unclosed = end < 0 || (nl >= 0 && nl < end);
    return unclosed || /\s$/.test(all.slice(start, end)) ? `${pre}\\$` : m;
  });
}

export function prepareMath(md: string): string {
  if (!md.includes("$") && !md.includes("\\(") && !md.includes("\\[") && !md.includes("*")) return md;
  return md
    .split(/(```[\s\S]*?(?:```|$)|`[^`\n]*`)/g)
    .map((part, i) => (i % 2 === 1 ? part : prepText(part)))
    .join("");
}
