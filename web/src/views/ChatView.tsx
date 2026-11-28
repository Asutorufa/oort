import { animate, AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useRef, useState } from "react";
import { fadeSlow, spring } from "../lib/motion";
import { useT, type Key, type T } from "../i18n";
import { commands as commandsApi, errorText, providers as providersApi, sessions } from "../api";
import { useToast } from "../components/common/Toast";
import type { ApprovalPolicy, InputRequest, ReasoningEffort, ServerToolConfig, TurnUsage } from "../api/types";
import { supportsWebSearch } from "../api/vendorCatalog";
import { Composer } from "../components/chat/Composer";
import { Inspector } from "../components/chat/Inspector";
import { lastPlan, PlanPill } from "../components/chat/PlanTracker";
import { Thread } from "../components/chat/Thread";
import { TurnInspector } from "../components/chat/TurnInspector";
import { Welcome } from "../components/chat/Welcome";
import { Empty, ErrorBar } from "../components/common";
import { navigate } from "../hooks/useHashRoute";
import { useOnReturn } from "../hooks/useOnReturn";
import { useResource } from "../hooks/useResource";
import { useSessionStream } from "../hooks/useSessionStream";
import { formatResponse, type WidgetActions } from "../widgets/protocol";

/** Command results arrive in English from the server; shown in the UI language. */
const COMMAND_MESSAGES: Record<string, Key> = {
  "Compaction requested": "cmd.compacted",
  "Approval: run every tool automatically": "cmd.policyAll",
  "Approval: read-only tools run automatically": "cmd.policyReadonly",
  "Approval: ask every time": "cmd.policyAsk",
  Stopped: "cmd.stopped",
  "Nothing was running": "cmd.nothingRunning",
  "Export ready": "cmd.exportReady",
  "New chat started": "cmd.newChat",
};
const commandMessage = (t: T, m: string) => {
  const k = COMMAND_MESSAGES[m.replace(/^✓\s*/, "")];
  return k ? t(k) : m;
};
const commandError = (t: T, m: string) =>
  /^\/approval takes/.test(m) ? t("cmd.approvalArgs") : /needs an open conversation/.test(m) ? t("cmd.needsChat") : m;

const modelCache = new Map<string, Promise<string[]>>();
/** The provider's model ids; failures are not cached so a later visit can retry. */
function modelList(providerId: string): Promise<string[]> {
  let p = modelCache.get(providerId);
  if (!p) {
    p = providersApi.models(providerId).then((r) => r.models);
    p.catch(() => modelCache.delete(providerId));
    modelCache.set(providerId, p);
  }
  return p;
}

const STATUS: Record<string, Key | null> = { running: "status.running", needs_approval: "status.needsApproval", halted: "status.halted", idle: null };

export function ChatView({
  selected,
  visible,
  onListChanged,
}: {
  selected: string | null;
  /** False while settings cover the chat; turning true again reloads what settings may have changed. */
  visible: boolean;
  onListChanged: () => void;
}) {
  if (!selected || selected === "new") {
    return (
      <Welcome
        visible={visible}
        onCreated={(id) => {
          onListChanged();
          navigate("chat", id);
        }}
      />
    );
  }
  return <Session key={selected} id={selected} visible={visible} onListChanged={onListChanged} />;
}

function Session({ id, visible, onListChanged }: { id: string; visible: boolean; onListChanged: () => void }) {
  const { detail, live, log, conn, error: loadError, refresh, toolTimes, subagents, plan } = useSessionStream(id, () => {
    onListChanged();
    navigate("chat");
  });
  const commandList = useResource(commandsApi.list);
  const childList = useResource(() => sessions.children(id));
  const [models, setModels] = useState<string[] | "failed">([]);
  const toast = useToast();
  const t = useT();
  const [trace, setTrace] = useState<string | null>(null);
  // Children appear as subagents start; refresh their list when one is new.
  const knownChildren = Object.values(subagents).map((s) => s.subSessionId).join(",");
  const reloadChildren = childList.reload;
  useEffect(() => {
    reloadChildren();
  }, [knownChildren, detail?.raw.length, reloadChildren]);
  const [error, setError] = useState<string | null>(null);
  // Pick up the automatic retry when it fires (its events start a new loop on the server).
  const retryAt = detail?.last_error?.retry_at ?? null;
  useEffect(() => {
    if (retryAt == null) return;
    const t = window.setTimeout(refresh, Math.max(0, retryAt - Date.now()) + 800);
    return () => window.clearTimeout(t);
  }, [retryAt, refresh]);
  // Models of this chat's provider, for switching (cached per provider for the page's life).
  const providerId = detail?.settings.provider_id ?? null;
  useEffect(() => {
    if (!providerId) return;
    let live = true;
    modelList(providerId)
      .then((m) => live && setModels(m))
      .catch(() => live && setModels("failed"));
    return () => {
      live = false;
    };
  }, [providerId]);
  const reloadCommands = commandList.reload;
  useOnReturn(visible, () => {
    reloadCommands();
    refresh();
    modelCache.clear();
    if (providerId) modelList(providerId).then(setModels, () => setModels("failed"));
  });
  const [inspector, setInspector] = useState(false);
  const [inspect, setInspect] = useState<TurnUsage | null>(null);
  const [title, setTitle] = useState<string | null>(null);
  // A new title (generated or renamed) fades into the header and reaches the sidebar at once.
  const titleRef = useRef<HTMLInputElement>(null);
  const shownTitle = useRef<string | undefined>(undefined);
  const savedTitle = detail?.title;
  useEffect(() => {
    if (savedTitle === undefined) return;
    if (shownTitle.current !== undefined && shownTitle.current !== savedTitle) {
      if (titleRef.current) animate(titleRef.current, { opacity: [0.2, 1] }, fadeSlow);
      onListChanged();
    }
    shownTitle.current = savedTitle;
  }, [savedTitle, onListChanged]);

  const send = async (input: InputRequest) => {
    try {
      await sessions.input(id, input);
      setError(null);
    } catch (e) {
      setError(errorText(e));
      throw e;
    }
  };
  const actions: WidgetActions = useMemo(
    () => ({
      respond: (callId, type, payload) => send({ type: "steer", text: formatResponse(callId, type, payload) }),
      prompt: (text) => send({ type: "steer", text }),
      compact: () => send({ type: "compact" }),
      approve: (callId) => send({ type: "approval", call_id: callId, decision: "approve" }),
      deny: (callId, reason) => send({ type: "approval", call_id: callId, decision: "deny", reason }),
    }),
    [id], // eslint-disable-line react-hooks/exhaustive-deps
  );
  /** Returns whether the change was saved (errors are shown in the bar). */
  const patch = async (req: {
    title?: string;
    approval_policy?: ApprovalPolicy;
    reasoning_effort?: ReasoningEffort;
    model?: string;
    enabled_server_tools?: ServerToolConfig;
  }) => {
    try {
      await sessions.patch(id, req);
      refresh();
      onListChanged();
      return true;
    } catch (e) {
      setError(errorText(e));
      return false;
    }
  };

  const runCommand = async (name: string, args: string) => {
    try {
      const r = await commandsApi.run(name, id, args);
      toast(commandMessage(t, r.message));
      if (r.effect.download) window.location.href = r.effect.download;
      if (r.effect.navigate_session) {
        onListChanged();
        navigate("chat", r.effect.navigate_session);
      }
      refresh();
    } catch (e) {
      toast(commandError(t, errorText(e)), "error");
      throw e;
    }
  };

  /** A new chat like this one, plus the MCP server just registered. */
  const startWithMcp = async (serverId: string) => {
    if (!detail) return;
    try {
      const d = await sessions.create({
        provider_id: detail.settings.provider_id,
        model: detail.settings.model,
        mcp_server_ids: [...detail.settings.mcp_servers.map((b) => b.id), serverId],
        builtin_groups: detail.lineage.builtin_groups,
        workspace_root: detail.lineage.workspace_root ?? undefined,
        approval_policy: detail.approval_policy,
        title: t("chat.mcpSessionTitle", { title: detail.title || t("chat.newSessionTitle") }),
      });
      onListChanged();
      navigate("chat", d.id);
    } catch (e) {
      setError(errorText(e));
    }
  };

  if (!detail)
    return (
      <div className="chat-loading">
        <header className="chat-top" />
        <Empty>{loadError ?? t("common.loading")}</Empty>
      </div>
    );
  const turns = detail.usage.per_turn;
  const prevTurn = inspect ? turns[turns.findIndex((t) => t.seq === inspect.seq) - 1] : undefined;
  const failed = !!detail.last_error;
  const busy = detail.status === "running" && !failed;
  const statusKey = failed ? (detail.last_error?.retry_at ? "status.retryPending" : "status.error") : STATUS[detail.status];
  const status = statusKey ? t(statusKey) : "";
  const planSteps = plan ?? lastPlan(detail.raw);

  return (
    <div className="chat">
      <div className="chat-main">
        <header className="chat-top">
          <div className="chat-top-left">
            <input
              ref={titleRef}
              className="chat-title"
              value={title ?? detail.title}
              placeholder={t("chat.titlePlaceholder")}
              title={detail.title}
              onChange={(e) => setTitle(e.target.value)}
              onBlur={() => {
                if (title !== null && title !== detail.title) patch({ title });
                setTitle(null);
              }}
              onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
            />
            {status && (
              <span className={`status-chip ${failed ? "failed" : detail.status}`}>
                <span className="status-dot" />
                {status}
              </span>
            )}
          </div>
          <div className="chat-top-right">
            {planSteps && planSteps.length > 0 && <PlanPill steps={planSteps} />}
            {conn !== "open" && <span className="pill warn nowrap">{conn === "reconnecting" ? t("status.reconnecting") : t("status.connecting")}</span>}
            <button className={`btn ghost nowrap ${inspector ? "on" : ""}`} onClick={() => setInspector(!inspector)} title={t("chat.inspectorHint")}>
              {t("chat.inspector")}
            </button>
          </div>
        </header>
        <ErrorBar error={error} onDismiss={() => setError(null)} />
        <Thread
          detail={detail}
          live={live}
          toolTimes={toolTimes}
          actions={actions}
          subagents={subagents}
          children={childList.data ?? []}
          onTrace={(sid) => {
            setTrace(sid);
            setInspector(true);
          }}
          onAnswer={(callId, content) => send({ type: "approval", call_id: callId, decision: "answer", content })}
          onStartWithMcp={startWithMcp}
          onRetry={async () => {
            try {
              await sessions.retry(id);
              setError(null);
              refresh();
            } catch (e) {
              setError(errorText(e));
            }
          }}
          onCancelAutoRetry={async () => {
            await sessions.cancel(id).catch((e) => setError(errorText(e)));
            refresh();
          }}
        />
        <Composer
          busy={busy}
          disabled={detail.status === "halted"}
          commands={commandList.data ?? []}
          policy={detail.approval_policy}
          onPolicy={(p) => patch({ approval_policy: p })}
          reasoning={detail.reasoning_effort ?? "default"}
          effectiveReasoning={detail.effective_reasoning_effort}
          onReasoning={(r) => patch({ reasoning_effort: r })}
          model={detail.model}
          models={models === "failed" ? [] : models}
          modelsFailed={models === "failed"}
          onModel={async (m) => {
            if (await patch({ model: m })) toast(t("composer.modelSwitched", { model: m }));
          }}
          webSearchSupported={supportsWebSearch(detail.provider_vendor, detail.provider)}
          webSearch={!!detail.effective_server_tools?.web_search?.enabled}
          onWebSearch={async (enabled) => {
            const cur = detail.effective_server_tools ?? {};
            const next: ServerToolConfig = {
              ...cur,
              web_search: enabled ? { enabled: true, max_uses: cur.web_search?.max_uses ?? 5 } : null,
            };
            if (await patch({ enabled_server_tools: next })) {
              toast(enabled ? t("composer.webSearchOn") : t("composer.webSearchOff"));
            }
          }}
          onSend={(text, skills) => send({ type: "steer", text, invoked_skills: skills })}
          onInterrupt={(text, skills) => send({ type: "interrupt", text, invoked_skills: skills })}
          onCommand={runCommand}
          onStop={() => sessions.cancel(id).catch((e) => setError(errorText(e)))}
        />
      </div>
      <AnimatePresence initial={false}>
      {inspector && (
        <motion.div
          key="inspector"
          className="inspector-wrap"
          initial={{ width: 0, opacity: 0 }}
          animate={{ width: 400, opacity: 1 }}
          exit={{ width: 0, opacity: 0 }}
          transition={spring}
        >
        <Inspector
          trace={trace}
          onCloseTrace={() => setTrace(null)}
          detail={detail}
          log={log}
          onInspect={setInspect}
          onCompact={() => send({ type: "compact" }).catch(() => {})}
          onStop={() => sessions.cancel(id).catch((e) => setError(errorText(e)))}
          onClose={() => {
            setInspector(false);
            setTrace(null);
          }}
        />
        </motion.div>
      )}
      </AnimatePresence>
      {inspect && <TurnInspector turn={inspect} prev={prevTurn} onClose={() => setInspect(null)} />}
    </div>
  );
}
