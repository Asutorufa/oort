/** `$12.34`; amounts under a cent keep four decimals so they do not read as zero. */
export function usd(n: number): string {
  const digits = n > 0 && n < 0.01 ? 4 : 2;
  return `$${n.toLocaleString("en-US", { minimumFractionDigits: digits, maximumFractionDigits: digits })}`;
}

/** `950`, `142.3k`, `12.4M` — the same in every language, so columns line up. */
export function tokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 999_950) return `${(n / 1000).toFixed(1)}k`;
  if (n < 999_950_000) return `${(n / 1_000_000).toFixed(1)}M`;
  return `${(n / 1_000_000_000).toFixed(1)}B`;
}

export const percent = (n: number) => `${n.toFixed(1)}%`;

/** A `YYYY-MM-DD` string as a local date (`new Date(str)` would read it as UTC). */
export function parseDay(date: string): Date {
  const [y, m, d] = date.split("-").map(Number);
  return new Date(y, m - 1, d);
}

/** `YYYY-MM` moved by `delta` months. */
export function shiftMonth(month: string, delta: number): string {
  const [y, m] = month.split("-").map(Number);
  const d = new Date(y, m - 1 + delta, 1);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
}
