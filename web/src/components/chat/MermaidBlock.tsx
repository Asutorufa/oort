import { useEffect, useId, useState } from "react";
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
  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [errOpen, setErrOpen] = useState(false);
  const [zoom, setZoom] = useState(1);
  const [fullscreen, setFullscreen] = useState(false);
  const [copied, setCopied] = useState(false);

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
          setSvg(rendered);
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

  useEffect(() => {
    if (!fullscreen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setFullscreen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [fullscreen]);

  const downloadSvg = () => {
    if (!svg) return;
    const blob = new Blob([svg], { type: "image/svg+xml;charset=utf-8" });
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
        <div className="mermaid-stage">
          <div
            className="mermaid-svg"
            style={{ transform: `scale(${zoom})`, transformOrigin: "top center" }}
            dangerouslySetInnerHTML={{ __html: svg }}
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
                    <button type="button" className="icon-btn" onClick={() => setZoom((z) => Math.max(0.4, Math.round((z - 0.15) * 100) / 100))}>
                      <ZoomOutIcon size={14} />
                    </button>
                    <button type="button" className="text-btn tabular" onClick={() => setZoom(1)}>
                      {Math.round(zoom * 100)}%
                    </button>
                    <button type="button" className="icon-btn" onClick={() => setZoom((z) => Math.min(3, Math.round((z + 0.15) * 100) / 100))}>
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
              <div className="mermaid-modal-body">
                <div
                  className="mermaid-svg"
                  style={{ transform: `scale(${zoom})`, transformOrigin: "center center" }}
                  dangerouslySetInnerHTML={{ __html: svg }}
                />
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
