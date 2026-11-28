import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import { popover } from "../../lib/motion";

export type Placement = "auto" | "top" | "bottom";

interface Pos {
  left: number;
  top?: number;
  bottom?: number;
  width?: number;
  minWidth: number;
  side: "top" | "bottom";
  maxHeight: number;
}

const GAP = 6;
/** Minimum distance from the viewport edges (borders and corners never touch the edge). */
const MARGIN = 16;

function place(anchor: DOMRect, contentHeight: number, placement: Placement, align: "start" | "end" | "stretch", contentWidth: number): Pos {
  const vw = document.documentElement.clientWidth || window.innerWidth;
  const below = window.innerHeight - anchor.bottom - GAP - MARGIN;
  const above = anchor.top - GAP - MARGIN;
  const side: "top" | "bottom" = placement === "top" ? "top" : placement === "bottom" ? "bottom" : below >= Math.min(contentHeight, 320) || below >= above ? "bottom" : "top";
  const width = align === "stretch" ? anchor.width : undefined;
  const w = width ?? Math.max(contentWidth, anchor.width);
  let left = align === "end" ? anchor.right - w : anchor.left;
  left = Math.max(MARGIN, Math.min(left, vw - w - MARGIN));
  return side === "bottom"
    ? { left, top: anchor.bottom + GAP, width, minWidth: anchor.width, side, maxHeight: Math.max(160, below) }
    : { left, bottom: window.innerHeight - anchor.top + GAP, width, minWidth: anchor.width, side, maxHeight: Math.max(160, above) };
}

/**
 * A floating layer anchored to an element: rendered in a portal, flips to the side with room
 * (or the side asked for), follows scrolling, closes on outside click and Esc.
 */
export function Popover({
  anchor,
  open,
  onClose,
  placement = "auto",
  align = "start",
  className = "",
  children,
  role,
  labelledBy,
}: {
  anchor: RefObject<HTMLElement | null>;
  open: boolean;
  onClose: () => void;
  placement?: Placement;
  align?: "start" | "end" | "stretch";
  className?: string;
  children: ReactNode;
  role?: string;
  labelledBy?: string;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<Pos | null>(null);

  const update = useCallback(() => {
    const a = anchor.current?.getBoundingClientRect();
    if (!a) return;
    const h = box.current?.scrollHeight ?? 240;
    const w = box.current?.offsetWidth ?? a.width;
    const next = place(a, h, placement, align, w);
    // Skip no-op updates (scroll/resize fire often; equal positions must not re-render).
    setPos((cur) => (cur && (Object.keys(next) as (keyof Pos)[]).every((k) => cur[k] === next[k]) ? cur : next));
  }, [anchor, placement, align]);

  useLayoutEffect(() => {
    if (!open) return;
    update();
    // Second pass once the content has a size.
    const raf = requestAnimationFrame(update);
    return () => cancelAnimationFrame(raf);
  }, [open, update]);

  useEffect(() => {
    if (!open) return;
    const outside = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!box.current?.contains(t) && !anchor.current?.contains(t)) onClose();
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && (e.stopPropagation(), onClose());
    window.addEventListener("mousedown", outside);
    window.addEventListener("keydown", esc, true);
    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    return () => {
      window.removeEventListener("mousedown", outside);
      window.removeEventListener("keydown", esc, true);
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", update, true);
    };
  }, [open, onClose, anchor, update]);

  return createPortal(
    <AnimatePresence>
      {open && (
        <motion.div
          ref={box}
          role={role}
          aria-labelledby={labelledBy}
          className={`pop ${className}`}
          variants={popover(pos?.side === "top" ? "bottom" : "top")}
          initial="hidden"
          animate="shown"
          exit="exit"
          style={{
            position: "fixed",
            left: pos?.left ?? -9999,
            top: pos?.top,
            bottom: pos?.bottom,
            width: pos?.width,
            minWidth: pos?.minWidth,
            maxHeight: pos?.maxHeight,
            maxWidth: `calc(100vw - ${MARGIN * 2}px)`,
            transformOrigin: `${pos?.side === "top" ? "bottom" : "top"} ${align === "end" ? "right" : "left"}`,
          }}
        >
          {children}
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}
