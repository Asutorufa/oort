/**
 * Models often write single symbols as inline LaTeX (`$\rightarrow$`). Without a math renderer
 * those show up raw, so a `$…$` (or `\(…\)`) that holds only symbol commands, spaces and plain
 * characters becomes the Unicode symbol. Code spans and fences are left alone, and anything with
 * a command outside this table (real formulas) is kept as written.
 */
const SYMBOLS: Record<string, string> = {
  rightarrow: "→", to: "→", leftarrow: "←", gets: "←", leftrightarrow: "↔", Rightarrow: "⇒", Leftarrow: "⇐",
  Leftrightarrow: "⇔", implies: "⟹", iff: "⟺", longrightarrow: "⟶", mapsto: "↦", uparrow: "↑", downarrow: "↓",
  times: "×", div: "÷", pm: "±", mp: "∓", cdot: "·", cdots: "⋯", ldots: "…", dots: "…", circ: "∘", bullet: "•",
  le: "≤", leq: "≤", ge: "≥", geq: "≥", ne: "≠", neq: "≠", approx: "≈", sim: "∼", simeq: "≃", equiv: "≡", propto: "∝",
  infty: "∞", partial: "∂", nabla: "∇", sum: "∑", prod: "∏", int: "∫", sqrt: "√", degree: "°",
  in: "∈", notin: "∉", subset: "⊂", subseteq: "⊆", supset: "⊃", cup: "∪", cap: "∩", emptyset: "∅", forall: "∀", exists: "∃",
  neg: "¬", land: "∧", lor: "∨", wedge: "∧", vee: "∨", checkmark: "✓", star: "⋆",
  alpha: "α", beta: "β", gamma: "γ", delta: "δ", epsilon: "ε", theta: "θ", lambda: "λ", mu: "μ", pi: "π", sigma: "σ",
  tau: "τ", phi: "φ", omega: "ω", Delta: "Δ", Sigma: "Σ", Omega: "Ω", Pi: "Π",
};

const MATH = /\$([^$\n]{1,80})\$|\\\(([^\n]{1,80}?)\\\)/g;

function convert(expr: string): string | null {
  let unknown = false;
  const out = expr.replace(/\\([a-zA-Z]+)/g, (_m, name: string) => {
    const s = SYMBOLS[name];
    if (!s) unknown = true;
    return s ?? "";
  });
  if (unknown || !/\\/.test(expr)) return null;
  // Only simple content may remain (no braces, sub/superscripts or other markup).
  return /[{}^_\\]/.test(out) ? null : out.trim();
}

/** Applies the substitution outside fenced and inline code. */
export function latexSymbolsToUnicode(md: string): string {
  if (!md.includes("\\")) return md;
  return md
    .split(/(```[\s\S]*?(?:```|$)|`[^`\n]*`)/g)
    .map((part, i) => (i % 2 === 1 ? part : part.replace(MATH, (m, a?: string, b?: string) => convert(a ?? b ?? "") ?? m)))
    .join("");
}
