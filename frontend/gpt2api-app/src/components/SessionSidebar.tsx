import { LogOut, MessageSquarePlus, Search } from "lucide-react";
import type { SessionRecord } from "../types";

interface SessionSidebarProps {
  sessions: SessionRecord[];
  selectedId?: string;
  search: string;
  onSearch: (value: string) => void;
  onSelect: (session: SessionRecord) => void;
  onNew: () => void;
  onLogout: () => void;
}

export function SessionSidebar({
  sessions,
  selectedId,
  search,
  onSearch,
  onSelect,
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
      <button type="button" className="new-chat" onClick={onNew}>
        <MessageSquarePlus size={17} />
        New chat
      </button>
      <label className="search-field">
        <Search size={15} />
        <input value={search} onChange={(event) => onSearch(event.target.value)} placeholder="Search sessions" />
      </label>
      <nav className="session-list">
        {filtered.map((session) => (
          <button
            type="button"
            key={session.id}
            className={session.id === selectedId ? "active" : ""}
            onClick={() => onSelect(session)}
          >
            <span>{session.title}</span>
            <small>{session.source}</small>
          </button>
        ))}
      </nav>
    </aside>
  );
}
