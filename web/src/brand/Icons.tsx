/** Small monochrome icons (text glyphs like ⚙ turn into color emoji on some systems). */
export function ToolIcon({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden="true" style={{ verticalAlign: "-2px" }}>
      <path
        d="M9.8 2.2a3.6 3.6 0 0 0-4.5 4.6L2.2 9.9a1.3 1.3 0 0 0 0 1.9l2 2a1.3 1.3 0 0 0 1.9 0l3.1-3.1a3.6 3.6 0 0 0 4.6-4.5l-2 2-1.9-.4-.4-1.9 2.3-2.3Z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
    </svg>
  );
}
