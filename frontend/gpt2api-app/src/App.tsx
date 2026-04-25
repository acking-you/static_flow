import { Bell, Image as ImageIcon, KeyRound, Loader2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import {
  cancelTask,
  createSession,
  fetchArtifactBlob,
  fetchSharedArtifactBlob,
  getSession,
  getShare,
  getTask,
  listSessions,
  openSessionEventStream,
  submitEditMessage,
  submitImageMessage,
  submitTextMessage,
  updateNotification,
  verifyKey,
} from "./api";
import { AdminPanel } from "./components/AdminPanel";
import { Composer, ComposerMode } from "./components/Composer";
import { PendingImageCard } from "./components/PendingImageCard";
import { SessionSidebar } from "./components/SessionSidebar";
import type {
  ImageArtifactRecord,
  ImageTaskRecord,
  ProductKey,
  QueueSnapshot,
  SessionDetail,
  SessionRecord,
  ShareResponse,
  TaskEventRecord,
} from "./types";

const STORAGE_KEY = "gpt2api.product.key";

export default function App() {
  const shareToken = shareTokenFromPath();
  const [apiKey, setApiKey] = useState(() => localStorage.getItem(STORAGE_KEY) || "");
  const [draftKey, setDraftKey] = useState(apiKey);
  const [keyInfo, setKeyInfo] = useState<ProductKey | null>(null);
  const [sessions, setSessions] = useState<SessionRecord[]>([]);
  const [selectedId, setSelectedId] = useState<string>("");
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [search, setSearch] = useState("");
  const [mode, setMode] = useState<ComposerMode>("image");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [events, setEvents] = useState<Record<string, TaskEventRecord[]>>({});
  const [queues, setQueues] = useState<Record<string, QueueSnapshot>>({});
  const [artifactUrls, setArtifactUrls] = useState<Record<string, string>>({});
  const [share, setShare] = useState<ShareResponse | null>(null);
  const [shareUrls, setShareUrls] = useState<Record<string, string>>({});

  useEffect(() => {
    if (shareToken) {
      void loadShare(shareToken);
      return;
    }
    if (apiKey) void bootstrap(apiKey);
  }, []);

  useEffect(() => {
    if (!apiKey || !selectedId || shareToken) return;
    let cancelled = false;
    void refreshDetail(selectedId);
    void connectEvents();
    async function connectEvents() {
      try {
        const stream = await openSessionEventStream(selectedId, apiKey);
        const reader = stream.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        while (!cancelled) {
          const { value, done } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          const parts = buffer.split(/\n\n|\r\n\r\n/);
          buffer = parts.pop() || "";
          for (const part of parts) handleSse(part);
        }
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      }
    }
    return () => {
      cancelled = true;
    };
  }, [apiKey, selectedId, shareToken]);

  useEffect(() => {
    if (!apiKey || !detail) return;
    const pending = detail.artifacts.filter((artifact) => !artifactUrls[artifact.id]);
    for (const artifact of pending) {
      void fetchArtifactBlob(apiKey, artifact.id).then((blob) => {
        setArtifactUrls((current) => ({ ...current, [artifact.id]: URL.createObjectURL(blob) }));
      });
    }
    for (const task of detail.tasks.filter((task) => task.status === "queued" || task.status === "running")) {
      void getTask(apiKey, task.id).then((value) => {
        setQueues((current) => ({ ...current, [task.id]: value.queue }));
      });
    }
  }, [apiKey, detail]);

  async function bootstrap(key: string) {
    setBusy(true);
    setError("");
    try {
      const verified = await verifyKey(key);
      setKeyInfo(verified);
      localStorage.setItem(STORAGE_KEY, key);
      setApiKey(key);
      const sessionItems = (await listSessions(key)).items;
      setSessions(sessionItems);
      if (sessionItems[0]) setSelectedId(sessionItems[0].id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function refreshSessions() {
    const items = (await listSessions(apiKey)).items;
    setSessions(items);
    return items;
  }

  async function refreshDetail(sessionId: string) {
    const value = await getSession(apiKey, sessionId);
    setDetail(value);
    await refreshSessions();
  }

  function handleSse(raw: string) {
    const event = raw.match(/^event:\s*(.+)$/m)?.[1]?.trim() || "message";
    const data = raw
      .split(/\r?\n/)
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trim())
      .join("\n");
    if (!data) return;
    if (event === "snapshot") {
      setDetail(JSON.parse(data) as SessionDetail);
    }
    if (event === "task_event") {
      const item = JSON.parse(data) as TaskEventRecord;
      setEvents((current) => ({ ...current, [item.task_id]: [...(current[item.task_id] || []), item] }));
      void refreshDetail(selectedId);
    }
  }

  async function newChat() {
    const created = await createSession(apiKey);
    const items = await refreshSessions();
    setSelectedId(created.session.id || items[0]?.id || "");
  }

  async function send(payload: { text: string; model: string; n: number; file?: File | null }) {
    if (!selectedId) return;
    setBusy(true);
    setError("");
    try {
      if (mode === "chat") {
        const next = await submitTextMessage(apiKey, selectedId, payload.text, payload.model);
        setDetail(next);
      } else if (mode === "edit" && payload.file) {
        await submitEditMessage(apiKey, selectedId, payload.text, payload.model, payload.n, payload.file);
        await refreshDetail(selectedId);
      } else {
        const submitted = await submitImageMessage(apiKey, selectedId, payload.text, payload.model, payload.n);
        setQueues((current) => ({ ...current, [submitted.task.id]: submitted.queue }));
        await refreshDetail(selectedId);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function saveNotification(email: string, enabled: boolean) {
    const value = await updateNotification(apiKey, email, enabled);
    setKeyInfo(value.key);
  }

  async function loadShare(token: string) {
    setBusy(true);
    try {
      const value = await getShare(token);
      setShare(value);
      for (const artifact of value.artifacts) {
        const blob = await fetchSharedArtifactBlob(token, artifact.id);
        setShareUrls((current) => ({ ...current, [artifact.id]: URL.createObjectURL(blob) }));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  function logout() {
    localStorage.removeItem(STORAGE_KEY);
    setApiKey("");
    setDraftKey("");
    setKeyInfo(null);
    setDetail(null);
    setSessions([]);
  }

  if (shareToken) {
    return <ShareView share={share} urls={shareUrls} error={error} busy={busy} />;
  }

  if (!keyInfo) {
    return (
      <main className="login-screen">
        <section className="login-panel">
          <KeyRound size={28} />
          <h1>GPT2API Workspace</h1>
          <p>Sign in with a GPT2API key.</p>
          <input value={draftKey} onChange={(event) => setDraftKey(event.target.value)} placeholder="sk-..." />
          <button onClick={() => void bootstrap(draftKey)} disabled={busy || !draftKey.trim()}>
            {busy ? <Loader2 className="spin" size={16} /> : null}
            Continue
          </button>
          {error && <p className="error-line">{error}</p>}
        </section>
      </main>
    );
  }

  return (
    <main className="workspace">
      <SessionSidebar
        sessions={sessions}
        selectedId={selectedId}
        search={search}
        onSearch={setSearch}
        onSelect={(session) => setSelectedId(session.id)}
        onNew={() => void newChat()}
        onLogout={logout}
      />
      <section className="conversation">
        <header className="conversation-header">
          <div>
            <span className="eyebrow">{keyInfo.name} · {keyInfo.role}</span>
            <h2>{detail?.session.title || "New chat"}</h2>
          </div>
          <NotificationControl keyInfo={keyInfo} onSave={(email, enabled) => void saveNotification(email, enabled)} />
        </header>
        {error && <p className="error-line top-error">{error}</p>}
        <MessageStream
          detail={detail}
          artifactUrls={artifactUrls}
          queues={queues}
          events={events}
          onCancel={(taskId) => void cancelTask(apiKey, taskId).then(() => refreshDetail(selectedId))}
        />
        <Composer disabled={busy || !selectedId} mode={mode} onModeChange={setMode} onSubmit={send} />
      </section>
      <AdminPanel apiKey={apiKey} role={keyInfo.role} />
    </main>
  );
}

function MessageStream({
  detail,
  artifactUrls,
  queues,
  events,
  onCancel,
}: {
  detail: SessionDetail | null;
  artifactUrls: Record<string, string>;
  queues: Record<string, QueueSnapshot>;
  events: Record<string, TaskEventRecord[]>;
  onCancel: (taskId: string) => void;
}) {
  const artifactsByMessage = useMemo(() => {
    const map = new Map<string, ImageArtifactRecord[]>();
    for (const artifact of detail?.artifacts || []) {
      map.set(artifact.message_id, [...(map.get(artifact.message_id) || []), artifact]);
    }
    return map;
  }, [detail]);
  if (!detail) return <div className="empty-state">Create or select a chat.</div>;
  return (
    <div className="message-stream">
      {detail.messages.map((message) => {
        const parsed = parseMessage(message.content_json);
        const task = detail.tasks.find((item) => item.message_id === message.id);
        const artifacts = artifactsByMessage.get(message.id) || [];
        return (
          <article key={message.id} className={`message ${message.role}`}>
            <div className="message-bubble">
              {parsed.blocks.map((block, index) => <p key={index}>{block.text}</p>)}
              {task && task.status !== "succeeded" && (
                <PendingImageCard task={task} queue={queues[task.id]} events={events[task.id] || []} onCancel={onCancel} />
              )}
              {artifacts.length > 0 && (
                <div className="artifact-grid">
                  {artifacts.map((artifact) => (
                    <figure key={artifact.id}>
                      {artifactUrls[artifact.id] ? <img src={artifactUrls[artifact.id]} alt={artifact.revised_prompt || "Generated image"} /> : <div className="image-placeholder"><ImageIcon size={22} /></div>}
                      <figcaption>{artifact.revised_prompt || "Generated image"}</figcaption>
                    </figure>
                  ))}
                </div>
              )}
            </div>
          </article>
        );
      })}
    </div>
  );
}

function NotificationControl({
  keyInfo,
  onSave,
}: {
  keyInfo: ProductKey;
  onSave: (email: string, enabled: boolean) => void;
}) {
  const [email, setEmail] = useState(keyInfo.notification_email || "");
  const [enabled, setEnabled] = useState(keyInfo.notification_enabled);
  useEffect(() => {
    setEmail(keyInfo.notification_email || "");
    setEnabled(keyInfo.notification_enabled);
  }, [keyInfo.id, keyInfo.notification_email, keyInfo.notification_enabled]);
  return (
    <div className="notification-control">
      <Bell size={15} />
      <input value={email} onChange={(event) => setEmail(event.target.value)} placeholder="email@example.com" />
      <label><input type="checkbox" checked={enabled} onChange={(event) => setEnabled(event.target.checked)} /> Email</label>
      <button onClick={() => onSave(email, enabled)}>Save</button>
    </div>
  );
}

function ShareView({
  share,
  urls,
  error,
  busy,
}: {
  share: ShareResponse | null;
  urls: Record<string, string>;
  error: string;
  busy: boolean;
}) {
  return (
    <main className="share-view">
      {busy && <p>Loading...</p>}
      {error && <p className="error-line">{error}</p>}
      {share && (
        <section>
          <span className="eyebrow">Shared image task</span>
          <h1>{share.session.title}</h1>
          <p>{share.task.prompt}</p>
          <div className="artifact-grid">
            {share.artifacts.map((artifact) => (
              <figure key={artifact.id}>
                {urls[artifact.id] && <img src={urls[artifact.id]} alt={artifact.revised_prompt || "Shared image"} />}
                <figcaption>{artifact.revised_prompt || share.task.model}</figcaption>
              </figure>
            ))}
          </div>
        </section>
      )}
    </main>
  );
}

function parseMessage(raw: string): { blocks: { type: string; text: string }[] } {
  try {
    const value = JSON.parse(raw);
    const blocks = Array.isArray(value.blocks) ? value.blocks : [];
    const parsed = blocks
      .map((block: unknown) => {
        if (!block || typeof block !== "object") return null;
        const maybe = block as { type?: string; text?: string };
        return { type: maybe.type || "text", text: maybe.text || "" };
      })
      .filter(Boolean) as { type: string; text: string }[];
    return { blocks: parsed.length ? parsed : [{ type: "text", text: "" }] };
  } catch {
    return { blocks: [{ type: "text", text: raw }] };
  }
}

function shareTokenFromPath() {
  const match = window.location.pathname.match(/\/gpt2api\/share\/([^/]+)/);
  return match ? decodeURIComponent(match[1]) : "";
}
