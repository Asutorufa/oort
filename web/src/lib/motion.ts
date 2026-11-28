import type { Transition, Variants } from "motion/react";

/**
 * Shared motion vocabulary: quick, light, never in the way. Nothing here lasts longer than
 * ~220ms; springs are stiff and well damped so they settle without wobble.
 */
export const spring: Transition = { type: "spring", stiffness: 450, damping: 34, mass: 0.8 };
/** For small knobs and pills that should feel snappy. */
export const snappy: Transition = { type: "spring", stiffness: 600, damping: 38, mass: 0.6 };
export const fade: Transition = { duration: 0.16, ease: [0.2, 0, 0, 1] };
export const fadeSlow: Transition = { duration: 0.22, ease: [0.2, 0, 0, 1] };

/** Popovers, menus, the slash palette: fade + slight scale + a few pixels of travel. */
export const popover = (from: "top" | "bottom" = "top"): Variants => ({
  hidden: { opacity: 0, scale: 0.97, y: from === "top" ? -4 : 4 },
  shown: { opacity: 1, scale: 1, y: 0, transition: fade },
  exit: { opacity: 0, scale: 0.98, y: from === "top" ? -2 : 2, transition: { duration: 0.12 } },
});

/** Accordion bodies: height and opacity together. */
export const collapse: Variants = {
  closed: { height: 0, opacity: 0, transition: { height: { duration: 0.18, ease: [0.2, 0, 0, 1] }, opacity: { duration: 0.1 } } },
  open: { height: "auto", opacity: 1, transition: { height: { duration: 0.2, ease: [0.2, 0, 0, 1] }, opacity: { duration: 0.16, delay: 0.04 } } },
};

/** Messages, cards and toasts entering the page. */
export const enter: Variants = {
  hidden: { opacity: 0, y: 6 },
  shown: { opacity: 1, y: 0, transition: fadeSlow },
};

/** Press feedback for primary buttons and chips. */
export const tap = { scale: 0.98 } as const;
