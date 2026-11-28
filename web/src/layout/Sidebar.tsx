import { useEffect, useState, type ReactNode } from "react";
import { errorText, sessions } from "../api";
import type { SessionSummary } from "../api/types";
import { LayersIcon, MemoryIcon, ModelIcon, MoonIcon, MoreIcon, PlusIcon, SidebarIcon, SparkIcon, SunIcon, ToolIcon } from "../brand/Icons";
import { OortMark } from "../brand/OortMark";
import { href, navigate, type View } from "../hooks/useHashRoute";
import { THEMES, useTheme } from "../theme/ThemeProvider";
import { Modal } from "../components/common";

const NAV: { view: View; label: string; icon: ReactNode }[] = [
  { view: "mcp", label: "ツール & MCP", icon: <ToolIcon /> },
  { view: "skills", label: "Skills", icon: <SparkIcon /> },
  { view: "memories", label: "メモリ", icon: <MemoryIcon /> },
  { view: "models", label: "モデル設定", icon: <ModelIcon /> },
  { view: "presets", label: "Presets", icon: <LayersIcon /> },
];

function group(ms: number): string {
  const d = new Date(ms);
  const today = new Date();
  const days = Math.floor((new Date(today.toDateString()).getTime() - new Date(d.toDateString()).getTime()) / 86400000);
  return days <= 0 ? "今日" : days === 1 ? "昨日" : days < 7 ? "過去 7 日間" : "それ以前";
}

/** Collapsible left sidebar: brand, new chat, conversations, navigation, theme. */
export function Sidebar({
  view,
  selected,
  items,
  onChanged,
}: {
  view: View;
  selected: string | null;
  items: SessionSummary[];
  onChanged: () => void;
}) {
  const { theme, mode, setTheme, setMode } = useTheme();
  const [collapsed, setCollapsed] = useState(() => localStorage.getItem("oort.sidebar") === "collapsed");
  const [menu, setMenu] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [title, setTitle] = useState("");
  const [confirm, setConfirm] = useState<SessionSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => localStorage.setItem("oort.sidebar", collapsed ? "collapsed" : "open"), [collapsed]);
  useEffect(() => {
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, []);

  const rename = async (id: string, value: string) => {
    if (renaming !== id) return;
    setRenaming(null);
    try {
      await sessions.patch(id, { title: value });
      onChanged();
    } catch (e) {
      setError(errorText(e));
    }
  };
  const remove = async (s: SessionSummary) => {
    setConfirm(null);
    try {
      await sessions.remove(s.id);
      if (s.id === selected) navigate("chat");
      onChanged();
    } catch (e) {
      setError(errorText(e));
    }
  };

  let lastGroup = "";
  return (
    <aside className={`sidebar ${collapsed ? "collapsed" : ""}`}>
      <div className="sb-top">
        <a className="brand" href={href("chat")} title="Oort">
          <OortMark size={20} />
          <span className="brand-name">Oort</span>
        </a>
        <button className="btn ghost icon collapse" onClick={() => setCollapsed(!collapsed)} aria-label={collapsed ? "サイドバーを開く" : "サイドバーを閉じる"}>
          <SidebarIcon />
        </button>
      </div>
      <a className="new-chat" href={href("chat", "new")} title="新しいチャット">
        <span className="plus">
          <PlusIcon />
        </span>
        <span className="label">新しいチャット</span>
      </a>

      <nav className="conversations" aria-label="会話">
        {items.map((s) => {
          const g = group(s.updated_at);
          const header = g !== lastGroup ? ((lastGroup = g), <div className="conv-group">{g}</div>) : null;
          return (
            <div key={s.id}>
              {header}
              <div className={`conv ${s.id === selected && view === "chat" ? "active" : ""}`} onClick={() => navigate("chat", s.id)} title={s.title || "無題の会話"}>
                {s.failed ? <span className="dot error" title="エラーで停止しました" /> : s.status === "running" ? <span className="pulse" /> : s.status === "needs_approval" ? <span className="dot warn" title="承認待ち" /> : null}
                {renaming === s.id ? (
                  <input
                    autoFocus
                    className="conv-rename"
                    value={title}
                    onClick={(e) => e.stopPropagation()}
                    onChange={(e) => setTitle(e.target.value)}
                    onBlur={(e) => rename(s.id, e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") rename(s.id, e.currentTarget.value);
                      if (e.key === "Escape") setRenaming(null);
                    }}
                  />
                ) : (
                  <span className="conv-title">{s.title || "無題の会話"}</span>
                )}
                <button
                  className="conv-more"
                  aria-label="メニュー"
                  onClick={(e) => {
                    e.stopPropagation();
                    setMenu(menu === s.id ? null : s.id);
                  }}
                >
                  <MoreIcon />
                </button>
                {menu === s.id && (
                  <div className="menu" onClick={(e) => e.stopPropagation()}>
                    <button
                      onClick={() => {
                        setMenu(null);
                        setTitle(s.title);
                        setRenaming(s.id);
                      }}
                    >
                      名前を変更
                    </button>
                    <a href={sessions.exportUrl(s.id)} download={`oort-${s.id}.json`} onClick={() => setMenu(null)}>
                      JSON でエクスポート
                    </a>
                    <button
                      className="danger"
                      onClick={() => {
                        setMenu(null);
                        setConfirm(s);
                      }}
                    >
                      削除
                    </button>
                  </div>
                )}
              </div>
            </div>
          );
        })}
        {items.length === 0 && <div className="conv-empty">まだ会話がありません</div>}
        {error && (
          <div className="conv-empty error-text" onClick={() => setError(null)}>
            {error}
          </div>
        )}
      </nav>

      <div className="sb-bottom">
        {NAV.map((n) => (
          <a key={n.view} href={href(n.view)} className={`nav ${view === n.view ? "active" : ""}`} title={n.label}>
            <span className="icon">{n.icon}</span>
            <span className="label">{n.label}</span>
          </a>
        ))}
        <div className="util-row">
          <div className="theme-seg" role="radiogroup" aria-label="テーマ">
            {THEMES.map((t) => (
              <button
                key={t.id}
                role="radio"
                aria-checked={theme === t.id}
                className={theme === t.id ? "on" : ""}
                title={t.label}
                onClick={() => setTheme(t.id)}
              >
                <span className="swatch" style={{ background: t.swatch }} />
                <span className="label">{t.label}</span>
              </button>
            ))}
          </div>
          <button className="mode-btn" onClick={() => setMode(mode === "light" ? "dark" : "light")} title={mode === "light" ? "ダークモードにする" : "ライトモードにする"} aria-label="表示モードを切り替え">
            {mode === "light" ? <MoonIcon /> : <SunIcon />}
          </button>
        </div>
      </div>

      {confirm && (
        <Modal title="会話を削除しますか？" onClose={() => setConfirm(null)}>
          <p>
            「{confirm.title || "無題の会話"}」とその履歴をすべて削除します。元に戻せません。
          </p>
          <div className="row">
            <span className="spacer" />
            <button className="btn" onClick={() => setConfirm(null)}>
              やめる
            </button>
            <button className="btn danger" onClick={() => remove(confirm)}>
              削除する
            </button>
          </div>
        </Modal>
      )}
    </aside>
  );
}
