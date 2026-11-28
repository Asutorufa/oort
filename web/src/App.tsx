import { AnimatePresence } from "motion/react";
import { useCallback, useEffect, useLayoutEffect, useRef } from "react";
import { sessions } from "./api";
import { CLOSE_POPOVERS } from "./components/ui/Popover";
import { navigate, useHashRoute, type SettingsView } from "./hooks/useHashRoute";
import { useResource } from "./hooks/useResource";
import { Sidebar } from "./layout/Sidebar";
import { ChatView } from "./views/ChatView";
import { SettingsWorkspace } from "./views/settings/SettingsWorkspace";

/**
 * Oort shell: the conversation sidebar and the chat, with settings as a full-viewport overlay.
 * The route lives in the URL hash. The chat stays mounted underneath settings, so the stream,
 * scroll position and unsent draft survive a visit.
 */
export function App() {
  const { view, id } = useHashRoute();
  const list = useResource(sessions.list, 3000);
  const settingsOpen = view !== "chat";

  // What to return to: the chat behind settings, and the settings page last looked at.
  const chatId = useRef<string | null>(null);
  const lastPage = useRef<SettingsView>("general");
  if (view === "chat") chatId.current = id;
  else lastPage.current = view;

  const openSettings = useCallback(() => navigate(lastPage.current), []);
  const closeSettings = useCallback(() => navigate("chat", chatId.current), []);

  // The chat is inert while covered; remember the focused control (the composer) and give it back.
  const shell = useRef<HTMLDivElement>(null);
  const focused = useRef<HTMLElement | null>(null);
  // Layout effect: runs before the settings overlay takes focus in its own effect.
  useLayoutEffect(() => {
    const el = shell.current;
    if (!el) return;
    if (settingsOpen) {
      focused.current = el.contains(document.activeElement) ? (document.activeElement as HTMLElement) : null;
      // Menus of the chat live in portals and would otherwise float above settings.
      window.dispatchEvent(new Event(CLOSE_POPOVERS));
      el.inert = true;
    } else {
      el.inert = false;
      focused.current?.focus({ preventScroll: true });
    }
  }, [settingsOpen]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.isComposing) return;
      if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key === ",") {
        e.preventDefault();
        if (settingsOpen) closeSettings();
        else openSettings();
      } else if (e.key === "Escape" && settingsOpen && !e.defaultPrevented) {
        // An open dropdown or dialog takes Esc first.
        if (!document.querySelector(".pop, .settings .modal-backdrop")) closeSettings();
      }
    };
    // Capture: runs before dropdowns and dialogs have removed themselves.
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [settingsOpen, openSettings, closeSettings]);

  return (
    <>
      <div className="app" ref={shell}>
        <Sidebar selected={chatId.current} items={list.data ?? []} onChanged={list.reload} onOpenSettings={openSettings} />
        <main className="view">
          <ChatView selected={chatId.current} visible={!settingsOpen} onListChanged={list.reload} />
        </main>
      </div>
      <AnimatePresence>{view !== "chat" && <SettingsWorkspace key="settings" view={view} id={id} onClose={closeSettings} />}</AnimatePresence>
    </>
  );
}
