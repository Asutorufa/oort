/** Oort Orbit Mark: a tilted orbit ring, the central star and one satellite. */
export function OortMark({ size = 24, className }: { size?: number; className?: string }) {
  return (
    <svg className={className} width={size} height={size} viewBox="0 0 32 32" fill="none" aria-hidden="true">
      <ellipse cx="16" cy="16" rx="13" ry="6.5" transform="rotate(-24 16 16)" stroke="var(--accent)" strokeWidth="1.6" opacity="0.55" />
      <circle cx="16" cy="16" r="4.2" fill="var(--accent)" />
      <circle cx="26.6" cy="9.6" r="2.3" fill="var(--orbit-dot)" />
    </svg>
  );
}
