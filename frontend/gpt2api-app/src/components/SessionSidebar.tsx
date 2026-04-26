import { LogOut, MessageSquarePlus, Search, Trash2 } from "lucide-react";
import type { SessionRecord } from "../types";

interface SessionSidebarProps {
  sessions: SessionRecord[];
  selectedId?: string;
  search: string;
  onSearch: (value: string) => void;
  onSelect: (session: SessionRecord) => void;
  onDelete: (session: SessionRecord) => void;
  onNew: () => void;
  onLogout: () => void;
}

export function SessionSidebar({
  sessions,
  selectedId,
  search,
  onSearch,
  onSelect,
  onDelete,
  onNew,
  onLogout,
}: SessionSidebarProps) {
  const filtered = sessions.filter((session) =>
    session.title.toLowerCase().includes(search.trim().toLowerCase()),
  );
  return (
    <aside className="sidebar">
      <div className="brand-row">
        <strong>GPT2API</strong>
        <button type="button" className="icon-button" onClick={onLogout} title="Log out">
          <LogOut size={16} />
        </button>
      </div>
      <button type="button" className={`new-chat ${selectedId ? "" : "active"}`} onClick={onNew}>
        <MessageSquarePlus size={17} />
        New image session
      </button>
      <label className="search-field">
        <Search size={15} />
        <input value={search} onChange={(event) => onSearch(event.target.value)} placeholder="Search sessions" />
      </label>
      <nav className="session-list">
        {filtered.map((session) => (
          <div key={session.id} className={`session-item ${session.id === selectedId ? "active" : ""}`}>
            <button
              type="button"
              className="session-select"
              aria-current={session.id === selectedId ? "page" : undefined}
              onClick={() => onSelect(session)}
            >
              <span>{session.title}</span>
              <small>{session.source} · {formatSessionTime(session.updated_at)}</small>
            </button>
            <button type="button" className="session-delete" onClick={() => onDelete(session)} title="Delete session">
              <Trash2 size={15} />
            </button>
          </div>
        ))}
        {filtered.length === 0 && <p className="session-empty">No sessions found</p>}
      </nav>
    </aside>
  );
}

function formatSessionTime(value: number) {
  const ms = value < 10_000_000_000 ? value * 1000 : value;
  const diff = Math.max(0, Date.now() - ms);
  const minutes = Math.floor(diff / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  return `${days}d`;
}
