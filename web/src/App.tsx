import { sessions } from "./api";
import { useHashRoute } from "./hooks/useHashRoute";
import { useResource } from "./hooks/useResource";
import { Sidebar } from "./layout/Sidebar";
import { ChatView } from "./views/ChatView";
import { McpView } from "./views/McpView";
import { PresetsView } from "./views/PresetsView";
import { ProvidersView } from "./views/ProvidersView";
import { SkillsView } from "./views/SkillsView";

/** Oort shell: one collapsible sidebar and the current view; the route lives in the URL hash. */
export function App() {
  const { view, id } = useHashRoute();
  const list = useResource(sessions.list, 3000);
  return (
    <div className="app">
      <Sidebar view={view} selected={view === "chat" ? id : null} items={list.data ?? []} onChanged={list.reload} />
      <main className="view">
        {view === "chat" && <ChatView selected={id} onListChanged={list.reload} />}
        {view === "mcp" && <McpView selected={id} />}
        {view === "skills" && <SkillsView selected={id} />}
        {view === "models" && <ProvidersView selected={id} />}
        {view === "presets" && <PresetsView selected={id} />}
      </main>
    </div>
  );
}
