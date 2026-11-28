import type { ReactNode } from "react";

/**
 * One line-icon family for the whole UI (24px grid, round caps, `currentColor`). Text glyphs such
 * as ⚙ or ✦ render as color emoji on some systems and never match each other, so none are used.
 */
function Svg({ size = 15, stroke = 1.6, children }: { size?: number; stroke?: number; children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={stroke}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className="icon-svg"
    >
      {children}
    </svg>
  );
}

type P = { size?: number; stroke?: number };

export const ToolIcon = (p: P) => (
  <Svg {...p}>
    <path d="M14.7 6.3a4 4 0 0 0-5.4 5.1L3.6 17.1a1.6 1.6 0 0 0 0 2.3l1 1a1.6 1.6 0 0 0 2.3 0l5.7-5.7a4 4 0 0 0 5.1-5.4l-2.6 2.6-2.3-.5-.5-2.3Z" />
  </Svg>
);
export const SparkIcon = (p: P) => (
  <Svg {...p}>
    <path d="M12 3.5 13.8 9a1.5 1.5 0 0 0 1 1l5.7 1.9-5.7 1.9a1.5 1.5 0 0 0-1 1L12 20.5l-1.8-5.7a1.5 1.5 0 0 0-1-1L3.5 12l5.7-1.9a1.5 1.5 0 0 0 1-1Z" />
  </Svg>
);
export const MemoryIcon = (p: P) => (
  <Svg {...p}>
    <rect x="4" y="3.5" width="16" height="17" rx="2.5" />
    <path d="M8 8h8M8 12h8M8 16h5" />
  </Svg>
);
export const ModelIcon = (p: P) => (
  <Svg {...p}>
    <path d="M4 7h10M18 7h2M4 17h4M12 17h8" />
    <circle cx="16" cy="7" r="2" />
    <circle cx="10" cy="17" r="2" />
  </Svg>
);
export const LayersIcon = (p: P) => (
  <Svg {...p}>
    <path d="m12 3.5 8.5 4.5-8.5 4.5L3.5 8Z" />
    <path d="m3.5 12.5 8.5 4.5 8.5-4.5" />
  </Svg>
);
export const ChartIcon = (p: P) => (
  <Svg {...p}>
    <path d="M3.5 3.5v15a2 2 0 0 0 2 2h15" />
    <path d="M8 16.5v-3M13 16.5v-11M18 16.5v-8" />
  </Svg>
);
export const PlusIcon = (p: P) => (
  <Svg {...p}>
    <path d="M12 5v14M5 12h14" />
  </Svg>
);
export const ChevronDown = (p: P) => (
  <Svg {...p}>
    <path d="m6 9 6 6 6-6" />
  </Svg>
);
export const CheckIcon = (p: P) => (
  <Svg {...p}>
    <path d="m5 12.5 4.5 4.5L19 7.5" />
  </Svg>
);
export const XIcon = (p: P) => (
  <Svg {...p}>
    <path d="M6 6l12 12M18 6 6 18" />
  </Svg>
);
export const SunIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="4" />
    <path d="M12 2.5v2M12 19.5v2M4.6 4.6l1.4 1.4M18 18l1.4 1.4M2.5 12h2M19.5 12h2M4.6 19.4 6 18M18 6l1.4-1.4" />
  </Svg>
);
export const MoonIcon = (p: P) => (
  <Svg {...p}>
    <path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5Z" />
  </Svg>
);
export const SettingsIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1.04 1.56V21a2 2 0 1 1-4 0v-.09A1.7 1.7 0 0 0 9 19.35a1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.7 1.7 0 0 0 .34-1.87 1.7 1.7 0 0 0-1.56-1.04H3a2 2 0 1 1 0-4h.09A1.7 1.7 0 0 0 4.65 9a1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.7 1.7 0 0 0 1.87.34H9a1.7 1.7 0 0 0 1-1.56V3a2 2 0 1 1 4 0v.09a1.7 1.7 0 0 0 1.04 1.56 1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.7 1.7 0 0 0-.34 1.87V9a1.7 1.7 0 0 0 1.56 1H21a2 2 0 1 1 0 4h-.09A1.7 1.7 0 0 0 19.4 15Z" />
  </Svg>
);
export const ArrowLeftIcon = (p: P) => (
  <Svg {...p}>
    <path d="M19 12H5M11 6l-6 6 6 6" />
  </Svg>
);
export const MonitorIcon = (p: P) => (
  <Svg {...p}>
    <rect x="3.5" y="4.5" width="17" height="12" rx="2" />
    <path d="M8.5 20h7M12 16.5V20" />
  </Svg>
);
export const PaletteIcon = (p: P) => (
  <Svg {...p}>
    <path d="M12 3.5a8.5 8.5 0 1 0 0 17c1.2 0 1.8-.8 1.8-1.7 0-1.3-1.1-1.5-1.1-2.6 0-.8.7-1.4 1.6-1.4h2a4.2 4.2 0 0 0 4.2-4.2c0-4-3.8-7.1-8.5-7.1Z" />
    <path d="M7.5 12h.01M9.5 8h.01M14.5 8h.01" />
  </Svg>
);
export const SidebarIcon = (p: P) => (
  <Svg {...p}>
    <rect x="3.5" y="4.5" width="17" height="15" rx="2.5" />
    <path d="M9.5 4.5v15" />
  </Svg>
);
export const MoreIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="6" cy="12" r=".8" fill="currentColor" />
    <circle cx="12" cy="12" r=".8" fill="currentColor" />
    <circle cx="18" cy="12" r=".8" fill="currentColor" />
  </Svg>
);
export const AlertIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M12 8v4.5M12 16h.01" />
  </Svg>
);
export const StopIcon = (p: P) => (
  <Svg {...p}>
    <rect x="7" y="7" width="10" height="10" rx="1.5" fill="currentColor" stroke="none" />
  </Svg>
);
export const ArrowUpIcon = (p: P) => (
  <Svg {...p}>
    <path d="M12 19V5M6 11l6-6 6 6" />
  </Svg>
);

export const EyeIcon = (p: P) => (
  <Svg {...p}>
    <path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12Z" />
    <circle cx="12" cy="12" r="3" />
  </Svg>
);
export const EyeOffIcon = (p: P) => (
  <Svg {...p}>
    <path d="M10.6 5.6A9.6 9.6 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a16 16 0 0 1-2.6 3.4M6.6 6.6C3.9 8.3 2.5 12 2.5 12S6 18.5 12 18.5a9 9 0 0 0 5.4-1.7" />
    <path d="M9.9 9.9a3 3 0 0 0 4.2 4.2M3.5 3.5l17 17" />
  </Svg>
);
export const TrashIcon = (p: P) => (
  <Svg {...p}>
    <path d="M4.5 7h15M10 11v6M14 11v6M6 7l1 12.5a1.5 1.5 0 0 0 1.5 1.5h7a1.5 1.5 0 0 0 1.5-1.5L18 7M9 7V4.5h6V7" />
  </Svg>
);
export const BrainIcon = (p: P) => (
  <Svg {...p}>
    <path d="M9 4.5a3 3 0 0 0-3 3v.2A3.2 3.2 0 0 0 4.5 13a3.3 3.3 0 0 0 2 5.5A3 3 0 0 0 12 19.5v-12a3 3 0 0 0-3-3ZM15 4.5a3 3 0 0 1 3 3v.2a3.2 3.2 0 0 1 1.5 5.3 3.3 3.3 0 0 1-2 5.5A3 3 0 0 1 12 19.5" />
  </Svg>
);
export const ShieldIcon = (p: P) => (
  <Svg {...p}>
    <path d="M12 3.5 19 6v5.5c0 4.3-3 7.6-7 9-4-1.4-7-4.7-7-9V6Z" />
    <path d="m9 12 2.2 2.2L15.5 10" />
  </Svg>
);
export const RefreshIcon = (p: P) => (
  <Svg {...p}>
    <path d="M19.5 12a7.5 7.5 0 0 1-13 5.1M4.5 12a7.5 7.5 0 0 1 13-5.1" />
    <path d="M17.5 3.5v3.4h-3.4M6.5 20.5v-3.4h3.4" />
  </Svg>
);
export const CubeIcon = (p: P) => (
  <Svg {...p}>
    <path d="m12 3 8 4.5v9L12 21l-8-4.5v-9Z" />
    <path d="m4 7.5 8 4.5 8-4.5M12 12v9" />
  </Svg>
);
export const GlobeIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M3.5 12h17M12 3.5c2.4 2.3 3.6 5.1 3.6 8.5s-1.2 6.2-3.6 8.5c-2.4-2.3-3.6-5.1-3.6-8.5S9.6 5.8 12 3.5Z" />
  </Svg>
);
export const SearchIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="11" cy="11" r="6.5" />
    <path d="m16 16 4 4" />
  </Svg>
);
export const ExternalLinkIcon = (p: P) => (
  <Svg {...p}>
    <path d="M14 4.5h5.5V10M10 14 19.5 4.5M19.5 14v4a1.5 1.5 0 0 1-1.5 1.5H6A1.5 1.5 0 0 1 4.5 18V6A1.5 1.5 0 0 1 6 4.5h4" />
  </Svg>
);
export const MaximizeIcon = (p: P) => (
  <Svg {...p}>
    <path d="M15 4.5h4.5V9M9 19.5H4.5V15M19.5 4.5 14 10M4.5 19.5 10 14" />
  </Svg>
);
export const DownloadIcon = (p: P) => (
  <Svg {...p}>
    <path d="M12 4.5v11M7.5 11.5 12 16l4.5-4.5M4.5 19.5h15" />
  </Svg>
);
export const ZoomInIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="11" cy="11" r="6.5" />
    <path d="m16 16 4 4M8.5 11h5M11 8.5v5" />
  </Svg>
);
export const ZoomOutIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="11" cy="11" r="6.5" />
    <path d="m16 16 4 4M8.5 11h5" />
  </Svg>
);
/** Generic marks for provider kinds and vendor presets (not trademark logos). */
export const KindIcon = ({ kind, size = 18 }: { kind: string; size?: number }) => {
  switch (kind) {
    case "openai":
      return (
        <Svg size={size}>
          <circle cx="12" cy="12" r="8" />
          <path d="M12 4v16M4 12h16" opacity=".55" />
        </Svg>
      );
    case "anthropic":
      return (
        <Svg size={size}>
          <path d="M5 19.5 12 4.5l7 15M8 13.5h8" />
        </Svg>
      );
    case "gemini":
      return (
        <Svg size={size}>
          <path d="M12 3c.8 4.6 4.4 8.2 9 9-4.6.8-8.2 4.4-9 9-.8-4.6-4.4-8.2-9-9 4.6-.8 8.2-4.4 9-9Z" />
        </Svg>
      );
    case "deepseek":
      return (
        <Svg size={size}>
          <path d="M4.5 13.5c1.5-4.5 5-7 9.5-7 3 0 5.5 1.8 5.5 4.8 0 3.5-3.2 6.2-7.5 6.2-3 0-5.5-1.5-7.5-4Z" />
          <circle cx="14.5" cy="10.5" r="1" fill="currentColor" stroke="none" />
        </Svg>
      );
    case "openrouter":
      return (
        <Svg size={size}>
          <circle cx="5.5" cy="12" r="2" />
          <circle cx="18.5" cy="6.5" r="2" />
          <circle cx="18.5" cy="17.5" r="2" />
          <path d="M7.5 11.2 16.5 7.3M7.5 12.8l9 3.9" />
        </Svg>
      );
    case "qwen":
      return (
        <Svg size={size}>
          <polygon points="12 3.5 19.5 7.8 19.5 16.2 12 20.5 4.5 16.2 4.5 7.8" />
          <path d="M12 12 19.5 7.8M12 12v8.5M12 12 4.5 7.8" opacity=".55" />
        </Svg>
      );
    case "ollama":
      return (
        <Svg size={size}>
          <rect x="4.5" y="6.5" width="15" height="11" rx="3" />
          <path d="M9 6.5V4M15 6.5V4M9.5 11.5h.01M14.5 11.5h.01" />
        </Svg>
      );
    case "custom":
    case "openai_compatible":
      return (
        <Svg size={size}>
          <rect x="3.5" y="5" width="17" height="11" rx="2" />
          <path d="M8 20h8M12 16v4M7.5 9.5l2 1.5-2 1.5M12 12.5h4" />
        </Svg>
      );
    default:
      return (
        <Svg size={size}>
          <rect x="4" y="4" width="16" height="16" rx="4" strokeDasharray="3 3" />
        </Svg>
      );
  }
};

/** Indeterminate progress ring. */
export function Spinner({ size = 14 }: { size?: number }) {
  return (
    <svg className="spinner" width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <circle cx="8" cy="8" r="6" stroke="currentColor" strokeOpacity=".18" strokeWidth="1.8" />
      <path d="M14 8a6 6 0 0 0-6-6" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
    </svg>
  );
}
