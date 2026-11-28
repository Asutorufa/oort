import { animate, motion, useMotionValue, useReducedMotion, type MotionValue } from "motion/react";
import { useEffect } from "react";

export type MarkState = "idle" | "thinking" | "running";

/** The viewBox center; both rings turn around it so the mark never wobbles. */
const CENTER = { originX: "12px", originY: "12px", transformBox: "view-box" } as const;

/**
 * Turns `angle` endlessly while `spinning` (`turn`: 1 clockwise, -1 counter-clockwise). On
 * stopping it glides on to its upright position instead of jumping back.
 */
function useSpin(angle: MotionValue<number>, spinning: boolean, turn: 1 | -1, seconds: number, reduced: boolean) {
  useEffect(() => {
    if (reduced) {
      angle.set(0);
      return;
    }
    const from = angle.get() % 360;
    angle.set(from);
    if (spinning) {
      const spin = animate(angle, from + turn * 360, { duration: seconds, ease: "linear", repeat: Infinity });
      return () => spin.stop();
    }
    if (from === 0) return;
    const rest = Math.abs(from + turn * 20) < 360 ? turn * 360 : turn * 720;
    const settle = animate(angle, rest, { duration: 0.8, ease: "easeOut" });
    return () => settle.stop();
  }, [angle, spinning, turn, seconds, reduced]);
}

/**
 * The Oort mark: an outer ring (one solid and one faint half) around an inner arc, in
 * `currentColor`. While the agent works the two counter-rotate; with reduced motion they hold
 * still and only fade gently. `public/favicon.svg` uses the same geometry.
 */
export function OortMark({ size = 20, className, animated, state }: { size?: number; className?: string; animated?: boolean; state?: MarkState }) {
  const active = animated ?? (state !== undefined && state !== "idle");
  const reduced = !!useReducedMotion();
  const outer = useMotionValue(0);
  const inner = useMotionValue(0);
  useSpin(outer, active, 1, 2.6, reduced);
  useSpin(inner, active, -1, 1.8, reduced);

  return (
    <motion.svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
      animate={{ opacity: active && reduced ? [1, 0.5, 1] : 1 }}
      transition={active && reduced ? { duration: 1.8, ease: "easeInOut", repeat: Infinity } : { duration: 0.3 }}
    >
      <motion.g className="oort-outer" style={{ ...CENTER, rotate: outer }}>
        <path d="M18.364 5.636A9 9 0 0 0 5.636 18.364" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" />
        <path d="M5.636 18.364A9 9 0 0 0 18.364 5.636" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeOpacity="0.32" />
      </motion.g>
      <motion.path
        className="oort-inner"
        d="M15.182 8.818A4.5 4.5 0 0 1 8.818 15.182"
        stroke="currentColor"
        strokeWidth="2.2"
        strokeLinecap="round"
        style={{ ...CENTER, rotate: inner }}
      />
    </motion.svg>
  );
}
