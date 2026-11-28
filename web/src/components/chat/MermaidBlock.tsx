import { type PointerEvent as ReactPointerEvent, type WheelEvent as ReactWheelEvent, useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import {
  AlertIcon,
  ChevronDown,
  DownloadIcon,
  MaximizeIcon,
  Spinner,
  XIcon,
  ZoomInIcon,
  ZoomOutIcon,
} from "../../brand/Icons";
import { useT } from "../../i18n";
import { fade, spring, tap } from "../../lib/motion";
import { token, useTheme } from "../../theme/ThemeProvider";
import { copyText } from "../common";
import { Collapse, SegmentedControl } from "../ui";

type MermaidApi = typeof import("mermaid").default;
let mermaidPromise: Promise<MermaidApi> | null = null;
function loadMermaid(): Promise<MermaidApi> {
  if (!mermaidPromise) {
    mermaidPromise = import("mermaid").then((m) => m.default);
  }
  return mermaidPromise;
}

interface RenderedSvg {
  html: string;
  width: number;
  height: number;
}

function normalizeMermaidSvg(raw: string): RenderedSvg {
  // Use the HTML5 parser (<template>) rather than strict `image/svg+xml` DOMParser,
  // because Mermaid's <foreignObject> labels contain HTML5 tags like `<br>` that
  // would cause strict XML parsing to truncate the rest of the diagram.
  const tpl = document.createElement("template");
  tpl.innerHTML = raw.trim();
  const svg = tpl.content.querySelector("svg");
  if (!svg) return { html: raw, width: 600, height: 400 };

  let width = 0;
  let height = 0;
  const vb = svg.getAttribute("viewBox");
  if (vb) {
    const parts = vb
      .trim()
      .split(/[\s,]+/)
      .map(Number);
    if (parts.length === 4 && parts[2] > 0 && parts[3] > 0) {
      width = parts[2];
      height = parts[3];
    }
  }
  if (!width || !height) {
    const wAttr = parseFloat(svg.getAttribute("width") || "");
    const hAttr = parseFloat(svg.getAttribute("height") || "");
    width = Number.isFinite(wAttr) && wAttr > 0 ? wAttr : 600;
    height = Number.isFinite(hAttr) && hAttr > 0 ? hAttr : 400;
    if (!vb) svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
  }

  // Strip Mermaid's `width="100%"` + `style="max-width: ..."` quirk so the SVG fills its
  // explicitly sized wrapper instead of collapsing to ~300px inside flex containers.
  svg.setAttribute("width", "100%");
  svg.setAttribute("height", "100%");
  svg.style.removeProperty("max-width");
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");

  return {
    html: svg.outerHTML,
    width: Math.ceil(width),
    height: Math.ceil(height),
  };
}

function parseRgb(color: string): [number, number, number, number] | null {
  const m = color.match(
    /rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)(?:\s*,\s*([\d.]+))?\s*\)/i,
  );
  if (!m) return null;
  return [
    Number(m[1]),
    Number(m[2]),
    Number(m[3]),
    m[4] === undefined ? 1 : Number(m[4]),
  ];
}

function relativeLuminance(r: number, g: number, b: number): number {
  const toLinear = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * toLinear(r) + 0.7152 * toLinear(g) + 0.0722 * toLinear(b);
}

function contrastRatio(l1: number, l2: number): number {
  const hi = Math.max(l1, l2);
  const lo = Math.min(l1, l2);
  return (hi + 0.05) / (lo + 0.05);
}

/**
 * Ensures text inside custom-styled Mermaid nodes (e.g. `style A fill:#e8f4fd` generated
 * by LLMs assuming a light canvas) always maintains readable WCAG AA contrast in both
 * light and dark themes.
 */
function fixSvgContrast(container: HTMLElement | null) {
  if (!container) return;
  const groups = container.querySelectorAll<SVGGElement>(
    "g.node, g.cluster, g.stateGroup, g.statediagram-state",
  );
  for (const g of groups) {
    // Pick the primary background shape of this node (skip shapes nested inside labels/clusters).
    let bgLum: number | null = null;
    const shapes = g.querySelectorAll<SVGElement>("rect, polygon, circle, ellipse, path");
    for (const shape of shapes) {
      if (shape.closest("g.label, g.cluster-label")) continue;
      if (g.classList.contains("cluster") && shape.closest("g.node")) continue;
      const rgb = parseRgb(getComputedStyle(shape).fill);
      if (rgb && rgb[3] > 0.35) {
        bgLum = relativeLuminance(rgb[0], rgb[1], rgb[2]);
        break;
      }
    }
    if (bgLum === null) continue;

    const readable = bgLum > 0.42 ? "#18181b" : "#f4f4f5";
    const labelRoot = g.classList.contains("cluster")
      ? g.querySelector<Element>(":scope > g.cluster-label")
      : g;
    if (!labelRoot) continue;

    const htmlLabels = labelRoot.querySelectorAll<HTMLElement>(
      "foreignObject span, foreignObject p, foreignObject div",
    );
    for (const el of htmlLabels) {
      const rgb = parseRgb(getComputedStyle(el).color);
      if (!rgb) continue;
      const textLum = relativeLuminance(rgb[0], rgb[1], rgb[2]);
      if (contrastRatio(bgLum, textLum) < 4.5) {
        el.style.setProperty("color", readable, "important");
      }
    }

    const svgTexts = labelRoot.querySelectorAll<SVGElement>("text, tspan");
    for (const el of svgTexts) {
      const rgb = parseRgb(getComputedStyle(el).fill);
      if (!rgb) continue;
      const textLum = relativeLuminance(rgb[0], rgb[1], rgb[2]);
      if (contrastRatio(bgLum, textLum) < 4.5) {
        el.style.setProperty("fill", readable, "important");
      }
    }
  }
}

function buildThemeVariables(dark: boolean) {
  const surface = token("--bg-surface") || (dark ? "#1A1A1D" : "#FAFAFB");
  const elevated = token("--bg-elevated") || (dark ? "#1E1E24" : "#FFFFFF");
  const border = token("--border-default") || (dark ? "#3F3F46" : "#D4D4D8");
  const text = token("--text-primary") || (dark ? "#F4F4F5" : "#18181B");
  const textSec = token("--text-secondary") || (dark ? "#B4B4BB" : "#52525B");
  const muted = token("--text-muted") || (dark ? "#7A7A82" : "#8E8E93");
  const accent = token("--accent") || (dark ? "#7C7CE6" : "#5B5BD6");

  return {
    background: "transparent",
    darkMode: dark,
    fontFamily: "var(--font-sans, Inter, system-ui, sans-serif)",
    fontSize: "13px",
    primaryColor: surface,
    primaryTextColor: text,
    primaryBorderColor: border,
    secondaryColor: elevated,
    secondaryTextColor: textSec,
    secondaryBorderColor: border,
    tertiaryColor: elevated,
    tertiaryTextColor: text,
    tertiaryBorderColor: border,
    lineColor: muted,
    textColor: text,
    mainBkg: surface,
    nodeBorder: border,
    clusterBkg: elevated,
    clusterBorder: border,
    edgeLabelBackground: surface,
    actorBkg: surface,
    actorBorder: border,
    actorTextColor: text,
    actorLineColor: muted,
    signalColor: text,
    signalTextColor: text,
    noteBkgColor: elevated,
    noteBorderColor: border,
    noteTextColor: text,
    activeTaskBkgColor: accent,
    activeTaskBorderColor: accent,
    sectionBkgColor: surface,
    altSectionBkgColor: elevated,
  };
}

export function MermaidBlock({ code, streaming }: { code: string; streaming?: boolean }) {
  const t = useT();
  const { theme, mode } = useTheme();
  const reactId = useId().replace(/[^a-zA-Z0-9]/g, "");
  const [view, setView] = useState<"diagram" | "source">("diagram");
  const [svg, setSvg] = useState<RenderedSvg | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [errOpen, setErrOpen] = useState(false);
  const [zoom, setZoom] = useState(1);
  const [modalZoom, setModalZoom] = useState(1);
  const [fullscreen, setFullscreen] = useState(false);
  const [copied, setCopied] = useState(false);
  const inlineRef = useRef<HTMLDivElement | null>(null);
  const modalRef = useRef<HTMLDivElement | null>(null);
  const stageRef = useRef<HTMLDivElement | null>(null);
  const modalBodyRef = useRef<HTMLDivElement | null>(null);
  const dragRef = useRef<{
    el: HTMLDivElement;
    pointerId: number;
    startX: number;
    startY: number;
    startLeft: number;
    startTop: number;
  } | null>(null);

  const syncPannable = useCallback((el: HTMLDivElement | null) => {
    if (!el) return;
    const canPan = el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1;
    el.classList.toggle("is-pannable", canPan);
  }, []);

  const onPanPointerDown = useCallback((e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const el = e.currentTarget;
    const canPan = el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1;
    if (!canPan) return;
    e.preventDefault();
    el.setPointerCapture(e.pointerId);
    el.classList.add("is-dragging");
    dragRef.current = {
      el,
      pointerId: e.pointerId,
      startX: e.clientX,
      startY: e.clientY,
      startLeft: el.scrollLeft,
      startTop: el.scrollTop,
    };
  }, []);

  const onPanPointerMove = useCallback((e: ReactPointerEvent<HTMLDivElement>) => {
    const d = dragRef.current;
    if (!d || d.el !== e.currentTarget || d.pointerId !== e.pointerId) return;
    d.el.scrollLeft = d.startLeft - (e.clientX - d.startX);
    d.el.scrollTop = d.startTop - (e.clientY - d.startY);
  }, []);

  const onPanPointerEnd = useCallback((e: ReactPointerEvent<HTMLDivElement>) => {
    const d = dragRef.current;
    if (!d || d.el !== e.currentTarget || d.pointerId !== e.pointerId) return;
    if (d.el.hasPointerCapture(e.pointerId)) {
      d.el.releasePointerCapture(e.pointerId);
    }
    d.el.classList.remove("is-dragging");
    dragRef.current = null;
  }, []);

  const onModalWheel = useCallback((e: ReactWheelEvent<HTMLDivElement>) => {
    if (!e.ctrlKey && !e.metaKey) return;
    e.preventDefault();
    const step = e.deltaY < 0 ? 0.15 : -0.15;
    setModalZoom((z) => Math.max(0.4, Math.min(3, Math.round((z + step) * 100) / 100)));
  }, []);

  useEffect(() => {
    // Never invoke mermaid.render() while the fenced block is still streaming/unclosed.
    if (streaming) return;
    let cancelled = false;
    const renderId = `mmd_${reactId}_${Date.now()}`;

    (async () => {
      try {
        const mermaid = await loadMermaid();
        if (cancelled) return;
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          suppressErrorRendering: true,
          theme: "base",
          themeVariables: buildThemeVariables(mode === "dark"),
        });
        await mermaid.parse(code);
        const { svg: rendered } = await mermaid.render(renderId, code);
        if (!cancelled) {
          setSvg(normalizeMermaidSvg(rendered));
          setError(null);
        }
      } catch (e) {
        // Clean up any stray error container mermaid may have injected.
        document.getElementById(`d${renderId}`)?.remove();
        document.getElementById(renderId)?.remove();
        if (!cancelled) {
          setSvg(null);
          setError(e instanceof Error ? e.message : String(e));
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [code, streaming, theme, mode, reactId]);

  useLayoutEffect(() => {
    if (svg && view === "diagram" && !streaming && !error) {
      fixSvgContrast(inlineRef.current);
      syncPannable(stageRef.current);
    }
  }, [svg, view, streaming, error, theme, mode, zoom, syncPannable]);

  useLayoutEffect(() => {
    if (fullscreen && svg) {
      fixSvgContrast(modalRef.current);
      syncPannable(modalBodyRef.current);
    }
  }, [fullscreen, svg, theme, mode, modalZoom, syncPannable]);

  useEffect(() => {
    if (!fullscreen) return;
    setModalZoom(1);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setFullscreen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [fullscreen]);

  const downloadSvg = () => {
    if (!svg) return;
    const liveSvg =
      modalRef.current?.querySelector("svg") ?? inlineRef.current?.querySelector("svg");
    let out = svg.html;
    if (liveSvg) {
      const clone = liveSvg.cloneNode(true) as SVGSVGElement;
      clone.setAttribute("width", String(svg.width));
      clone.setAttribute("height", String(svg.height));
      out = new XMLSerializer().serializeToString(clone);
    }
    const blob = new Blob([out], { type: "image/svg+xml;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "diagram.svg";
    a.click();
    URL.revokeObjectURL(url);
  };

  const copySource = async () => {
    await copyText(code);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1200);
  };

  const showSource = streaming || !!error || view === "source";

  // Compute a balanced default scale in the fullscreen modal so compact diagrams fill
  // the viewport comfortably instead of appearing tiny.
  const modalFitScale = svg
    ? (() => {
        const availW = Math.max(320, Math.min(1100, window.innerWidth * 0.94) - 64);
        const availH = Math.max(240, Math.min(820, window.innerHeight * 0.84) - 110);
        const fit = Math.min(availW / svg.width, availH / svg.height);
        return Math.max(0.5, Math.min(1.8, fit));
      })()
    : 1;

  return (
    <div className="mermaid-card">
      <div className="mermaid-head">
        <div className="mermaid-head-left">
          <span className="mermaid-lang">mermaid</span>
          {streaming && (
            <span className="mermaid-status">
              <Spinner size={12} />
              <span>{t("mermaid.drawing")}</span>
            </span>
          )}
          {!streaming && !error && (
            <SegmentedControl<"diagram" | "source">
              ariaLabel="Mermaid view"
              value={view}
              onChange={setView}
              options={[
                { value: "diagram", label: t("mermaid.diagram") },
                { value: "source", label: t("mermaid.source") },
              ]}
            />
          )}
        </div>
        <div className="mermaid-head-actions">
          {!showSource && svg && (
            <div className="mermaid-zoom">
              <button
                type="button"
                className="icon-btn"
                title={t("mermaid.zoomOut")}
                onClick={() => setZoom((z) => Math.max(0.4, Math.round((z - 0.15) * 100) / 100))}
              >
                <ZoomOutIcon size={13} />
              </button>
              <button
                type="button"
                className="text-btn tabular"
                title={t("mermaid.resetZoom")}
                onClick={() => setZoom(1)}
              >
                {Math.round(zoom * 100)}%
              </button>
              <button
                type="button"
                className="icon-btn"
                title={t("mermaid.zoomIn")}
                onClick={() => setZoom((z) => Math.min(2.5, Math.round((z + 0.15) * 100) / 100))}
              >
                <ZoomInIcon size={13} />
              </button>
              <button
                type="button"
                className="icon-btn"
                title={t("mermaid.fullscreen")}
                onClick={() => setFullscreen(true)}
              >
                <MaximizeIcon size={13} />
              </button>
            </div>
          )}
          {svg && !streaming && (
            <button type="button" className="icon-btn" title={t("mermaid.downloadSvg")} onClick={downloadSvg}>
              <DownloadIcon size={13} />
            </button>
          )}
          <button type="button" className="text-btn" onClick={copySource}>
            {copied ? t("common.copied") : t("common.copy")}
          </button>
        </div>
      </div>

      {error && !streaming && (
        <div className="mermaid-error-bar">
          <button type="button" className="mermaid-error-toggle" onClick={() => setErrOpen((o) => !o)}>
            <AlertIcon size={13} />
            <span>{t("mermaid.error")}</span>
            <motion.span animate={{ rotate: errOpen ? 180 : 0 }} transition={spring} className="chev">
              <ChevronDown size={12} />
            </motion.span>
          </button>
          <Collapse open={errOpen}>
            <pre className="mermaid-error-detail">{error}</pre>
          </Collapse>
        </div>
      )}

      {showSource ? (
        <pre className="mermaid-source">
          <code>{code}</code>
        </pre>
      ) : svg ? (
        <div
          ref={stageRef}
          className="mermaid-stage"
          onPointerDown={onPanPointerDown}
          onPointerMove={onPanPointerMove}
          onPointerUp={onPanPointerEnd}
          onPointerCancel={onPanPointerEnd}
        >
          <div
            ref={inlineRef}
            className="mermaid-svg"
            style={{
              width: `${Math.round(svg.width * zoom)}px`,
              maxWidth: zoom <= 1 ? "100%" : "none",
              aspectRatio: `${svg.width} / ${svg.height}`,
            }}
            dangerouslySetInnerHTML={{ __html: svg.html }}
          />
        </div>
      ) : (
        <div className="mermaid-loading">
          <Spinner size={14} />
          <span>{t("mermaid.drawing")}</span>
        </div>
      )}

      <AnimatePresence>
        {fullscreen && svg && (
          <motion.div
            className="mermaid-modal-backdrop"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={fade}
            onClick={() => setFullscreen(false)}
          >
            <motion.div
              className="mermaid-modal"
              initial={{ opacity: 0, scale: 0.96, y: 8 }}
              animate={{ opacity: 1, scale: 1, y: 0 }}
              exit={{ opacity: 0, scale: 0.97, y: 4 }}
              transition={spring}
              onClick={(e) => e.stopPropagation()}
            >
              <div className="mermaid-modal-head">
                <span className="mermaid-lang">mermaid</span>
                <div className="mermaid-head-actions">
                  <div className="mermaid-zoom">
                    <button type="button" className="icon-btn" onClick={() => setModalZoom((z) => Math.max(0.4, Math.round((z - 0.15) * 100) / 100))}>
                      <ZoomOutIcon size={14} />
                    </button>
                    <button type="button" className="text-btn tabular" onClick={() => setModalZoom(1)}>
                      {Math.round(modalZoom * 100)}%
                    </button>
                    <button type="button" className="icon-btn" onClick={() => setModalZoom((z) => Math.min(3, Math.round((z + 0.15) * 100) / 100))}>
                      <ZoomInIcon size={14} />
                    </button>
                  </div>
                  <motion.button whileTap={tap} type="button" className="btn ghost small" onClick={downloadSvg}>
                    <DownloadIcon size={13} /> {t("mermaid.downloadSvg")}
                  </motion.button>
                  <button type="button" className="icon-btn" aria-label={t("common.close")} onClick={() => setFullscreen(false)}>
                    <XIcon size={15} />
                  </button>
                </div>
              </div>
              <div
                ref={modalBodyRef}
                className="mermaid-modal-body"
                onPointerDown={onPanPointerDown}
                onPointerMove={onPanPointerMove}
                onPointerUp={onPanPointerEnd}
                onPointerCancel={onPanPointerEnd}
                onWheel={onModalWheel}
              >
                <div
                  ref={modalRef}
                  className="mermaid-svg"
                  style={{
                    width: `${Math.round(svg.width * modalFitScale * modalZoom)}px`,
                    height: `${Math.round(svg.height * modalFitScale * modalZoom)}px`,
                  }}
                  dangerouslySetInnerHTML={{ __html: svg.html }}
                />
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
