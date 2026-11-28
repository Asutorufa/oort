/**
 * Prepares model output for remark-math (outside code spans and fences):
 * - `\(…\)` → `$…$` and `\[…\]` → `$$…$$` (Markdown would otherwise eat the backslashes);
 * - a `$` that starts an amount (`$5 and $10`) is escaped so prices are not typeset as math.
 *   A `$` before a digit counts as an amount when no closing `$` follows on the line, or the
 *   text up to the next `$` ends in a space (math never ends with one: `$2x$` stays math).
 */
function prepText(t: string): string {
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
  if (!md.includes("$") && !md.includes("\\(") && !md.includes("\\[")) return md;
  return md
    .split(/(```[\s\S]*?(?:```|$)|`[^`\n]*`)/g)
    .map((part, i) => (i % 2 === 1 ? part : prepText(part)))
    .join("");
}
