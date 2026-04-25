import { RefreshCw, Save, Shield, SlidersHorizontal, X } from "lucide-react";
import { useEffect, useState } from "react";
import {
  adminCancelTask,
  adminKeys,
  adminQueue,
  adminSessions,
  patchAdminKey,
  patchAdminQueue,
} from "../api";
import type { AdminQueueResponse, ProductKey, SessionRecord } from "../types";

interface AdminPanelProps {
  apiKey: string;
  role: string;
}

export function AdminPanel({ apiKey, role }: AdminPanelProps) {
  const [tab, setTab] = useState<"sessions" | "queue" | "keys">("sessions");
  const [sessions, setSessions] = useState<SessionRecord[]>([]);
  const [queue, setQueue] = useState<AdminQueueResponse | null>(null);
  const [keys, setKeys] = useState<ProductKey[]>([]);
  const [query, setQuery] = useState("");
  const [concurrency, setConcurrency] = useState(1);
  const [error, setError] = useState("");

  useEffect(() => {
    if (role === "admin") void reload();
  }, [role, tab]);

  if (role !== "admin") {
    return (
      <aside className="right-panel">
        <section className="quiet-panel">
          <Shield size={18} />
          <p>Admin tools are hidden for this key.</p>
        </section>
      </aside>
    );
  }

  async function reload() {
    setError("");
    try {
      if (tab === "sessions") {
        const params = new URLSearchParams({ limit: "80" });
        if (query.trim()) params.set("q", query.trim());
        setSessions((await adminSessions(apiKey, params)).items);
      }
      if (tab === "queue") {
        const value = await adminQueue(apiKey);
        setQueue(value);
        setConcurrency(value.config.global_image_concurrency);
      }
      if (tab === "keys") {
        setKeys(await adminKeys(apiKey));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function saveQueue() {
    setError("");
    try {
      await patchAdminQueue(apiKey, concurrency);
      await reload();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function updateKey(key: ProductKey, patch: Partial<ProductKey>) {
    setError("");
    try {
      await patchAdminKey(apiKey, key.id, patch);
      await reload();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <aside className="right-panel">
      <div className="panel-tabs">
        <button className={tab === "sessions" ? "active" : ""} onClick={() => setTab("sessions")}>All sessions</button>
        <button className={tab === "queue" ? "active" : ""} onClick={() => setTab("queue")}>Queue</button>
        <button className={tab === "keys" ? "active" : ""} onClick={() => setTab("keys")}>Keys</button>
        <button className="icon-button" onClick={() => void reload()} title="Refresh"><RefreshCw size={15} /></button>
      </div>
      {error && <p className="error-line">{error}</p>}
      {tab === "sessions" && (
        <section className="admin-section">
          <input className="plain-input" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search all sessions" />
          <button className="secondary-button" onClick={() => void reload()}>Search</button>
          <div className="admin-list">
            {sessions.map((session) => (
              <div key={session.id} className="admin-row">
                <strong>{session.title}</strong>
                <span>{session.key_id} · {session.source}</span>
              </div>
            ))}
          </div>
        </section>
      )}
      {tab === "queue" && (
        <section className="admin-section">
          <label className="setting-row">
            <span><SlidersHorizontal size={15} /> Global concurrency</span>
            <input type="number" min={1} max={16} value={concurrency} onChange={(event) => setConcurrency(Number(event.target.value))} />
          </label>
          <button className="secondary-button" onClick={() => void saveQueue()}><Save size={15} /> Save</button>
          <div className="queue-columns">
            <QueueList title="Running" tasks={queue?.queue.running || []} onCancel={(id) => void adminCancelTask(apiKey, id).then(reload)} />
            <QueueList title="Queued" tasks={queue?.queue.queued || []} onCancel={(id) => void adminCancelTask(apiKey, id).then(reload)} />
          </div>
        </section>
      )}
      {tab === "keys" && (
        <section className="admin-section">
          <div className="admin-list">
            {keys.map((item) => (
              <div key={item.id} className="admin-row key-row">
                <strong>{item.name}</strong>
                <span>{item.id} · {item.role} · {item.quota_used_calls}/{item.quota_total_calls}</span>
                <button onClick={() => void updateKey(item, { role: item.role === "admin" ? "user" : "admin" })}>
                  {item.role === "admin" ? "Make user" : "Make admin"}
                </button>
              </div>
            ))}
          </div>
        </section>
      )}
    </aside>
  );
}

function QueueList({
  title,
  tasks,
  onCancel,
}: {
  title: string;
  tasks: { id: string; prompt: string; key_id: string }[];
  onCancel: (id: string) => void;
}) {
  return (
    <div>
      <h4>{title}</h4>
      {tasks.map((task) => (
        <div className="admin-row" key={task.id}>
          <strong>{task.prompt}</strong>
          <span>{task.key_id}</span>
          <button onClick={() => onCancel(task.id)}><X size={14} /> Cancel</button>
        </div>
      ))}
    </div>
  );
}
