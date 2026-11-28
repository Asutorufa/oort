/**
 * Oort mark: a thin tilted orbit around a small core, one body on the orbit and a faint grain of
 * the outer cloud. Monochrome (`currentColor`) so it follows the text color in every theme and
 * stays legible at 18–20px.
 */
export function OortMark({ size = 20, className }: { size?: number; className?: string }) {
  return (
    <svg className={className} width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <ellipse cx="12" cy="12" rx="9.6" ry="4.4" transform="rotate(-28 12 12)" stroke="currentColor" strokeWidth="1.4" />
      <circle cx="12" cy="12" r="2.3" fill="currentColor" />
      <circle cx="19.6" cy="6.7" r="1.6" fill="currentColor" />
      <circle cx="5.2" cy="19.6" r="0.9" fill="currentColor" opacity="0.45" />
      <circle cx="20.6" cy="16.4" r="0.7" fill="currentColor" opacity="0.35" />
    </svg>
  );
}
