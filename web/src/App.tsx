import { href, useHashRoute, type View } from "./hooks/useHashRoute";
import { ChatView } from "./views/ChatView";
import { McpView } from "./views/McpView";
import { PresetsView } from "./views/PresetsView";
import { ProvidersView } from "./views/ProvidersView";

const NAV: { view: View; label: string; icon: string }[] = [
  { view: "chat", label: "Chat", icon: "💬" },
  { view: "providers", label: "Providers", icon: "🔌" },
  { view: "mcp", label: "MCP", icon: "🧰" },
  { view: "presets", label: "Presets", icon: "📋" },
];

/** Workbench shell: a navigation rail and one view; the route lives in the URL hash. */
export function App() {
  const { view, id } = useHashRoute();
  return (
    <div className="app">
      <nav className="rail">
        <div className="logo" title="oort workbench">
          oort
        </div>
        {NAV.map((n) => (
          <a key={n.view} href={href(n.view)} className={view === n.view ? "active" : ""} title={n.label}>
            <span className="icon">{n.icon}</span>
            <span className="label">{n.label}</span>
          </a>
        ))}
      </nav>
      <div className="view">
        {view === "chat" && <ChatView selected={id} />}
        {view === "providers" && <ProvidersView selected={id} />}
        {view === "mcp" && <McpView selected={id} />}
        {view === "presets" && <PresetsView selected={id} />}
      </div>
    </div>
  );
}
