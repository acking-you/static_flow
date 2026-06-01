import {
  BarChart3,
  Bell,
  Coins,
  Download,
  Expand,
  Image as ImageIcon,
  KeyRound,
  Loader2,
  RefreshCw,
  X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  cancelTask,
  createSession,
  deleteSession,
  fetchMyUsageEvents,
  fetchArtifactBlob,
  fetchArtifactThumbnailBlob,
  fetchSharedArtifactBlob,
  fetchSharedArtifactThumbnailBlob,
  getSession,
  getShare,
  getTask,
  listSessions,
  openSessionEventStream,
  patchSession,
  submitEditMessage,
  submitImageMessage,
  updateNotification,
  verifyKey,
} from "./api";
import { AdminPanel } from "./components/AdminPanel";
import { Composer, ComposerMode } from "./components/Composer";
import { PendingImageCard } from "./components/PendingImageCard";
import { SessionSidebar } from "./components/SessionSidebar";
import type {
  ImageArtifactRecord,
  ImageSize,
  ImageSubmissionResult,
  ImageTaskRecord,
  ProductKey,
  QueueSnapshot,
  SessionDetail,
  SessionRecord,
  ShareResponse,
  TaskEventRecord,
  UsageEventRecord,
  UsageEventsResponse,
} from "./types";

const STORAGE_KEY = "gpt2api.product.key";

interface ImageLightboxState {
  artifactId: string;
  previewUrl: string;
  originalUrl?: string;
  caption: string;
  downloadName: string;
  loading: boolean;
}

export default function App() {
  const shareToken = shareTokenFromPath();
  const [apiKey, setApiKey] = useState(() => localStorage.getItem(STORAGE_KEY) || "");
  const [draftKey, setDraftKey] = useState(apiKey);
  const [keyInfo, setKeyInfo] = useState<ProductKey | null>(null);
  const [sessions, setSessions] = useState<SessionRecord[]>([]);
  const [selectedId, setSelectedId] = useState<string>("");
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [search, setSearch] = useState("");
  const [mode, setMode] = useState<ComposerMode>("image");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [streamState, setStreamState] = useState<"idle" | "live" | "reconnecting">("idle");
  const [events, setEvents] = useState<Record<string, TaskEventRecord[]>>({});
  const [queues, setQueues] = useState<Record<string, QueueSnapshot>>({});
  const [thumbnailUrls, setThumbnailUrls] = useState<Record<string, string>>({});
  const [originalUrls, setOriginalUrls] = useState<Record<string, string>>({});
  const [originalLoading, setOriginalLoading] = useState<Record<string, boolean>>({});
  const [lightbox, setLightbox] = useState<ImageLightboxState | null>(null);
  const [share, setShare] = useState<ShareResponse | null>(null);
  const [shareThumbnailUrls, setShareThumbnailUrls] = useState<Record<string, string>>({});
  const [usageOpen, setUsageOpen] = useState(false);
  const [usageData, setUsageData] = useState<UsageEventsResponse | null>(null);
  const [usageQuery, setUsageQuery] = useState("");
  const [usageOffset, setUsageOffset] = useState(0);
  const [usageBusy, setUsageBusy] = useState(false);
  const [usageError, setUsageError] = useState("");
  const streamRefreshTimer = useRef<number | null>(null);
  const detailRequestSeq = useRef(0);
  const selectedIdRef = useRef(selectedId);

  useEffect(() => {
    return () => {
      if (streamRefreshTimer.current) window.clearTimeout(streamRefreshTimer.current);
    };
  }, []);

  useEffect(() => {
    if (shareToken) {
      void loadShare(shareToken);
      return;
    }
    if (apiKey) void bootstrap(apiKey);
  }, []);

  useEffect(() => {
    selectedIdRef.current = selectedId;
  }, [selectedId]);

  useEffect(() => {
    if (!apiKey || !selectedId || shareToken) return;
    const controller = new AbortController();
    const sessionId = selectedId;
    void refreshDetail(sessionId, { signal: controller.signal, showLoading: true }).catch((err) => {
      if (!controller.signal.aborted && !isAbortError(err)) {
        setDetailLoading(false);
        setError(err instanceof Error ? err.message : String(err));
      }
    });
    void connectEvents();
    async function connectEvents() {
      while (!controller.signal.aborted) {
        try {
          setStreamState("live");
          const stream = await openSessionEventStream(sessionId, apiKey, { signal: controller.signal });
          const reader = stream.getReader();
          const decoder = new TextDecoder();
          let buffer = "";
          try {
            while (!controller.signal.aborted) {
              const { value, done } = await reader.read();
              if (done) break;
              buffer += decoder.decode(value, { stream: true });
              const parts = buffer.split(/\n\n|\r\n\r\n/);
              buffer = parts.pop() || "";
              for (const part of parts) handleSse(part, sessionId);
            }
          } finally {
            void reader.cancel().catch(() => undefined);
          }
        } catch (err) {
          if (controller.signal.aborted || isAbortError(err)) return;
        }
        if (!controller.signal.aborted) {
          setStreamState("reconnecting");
          await sleep(1500);
        }
      }
    }
    return () => {
      controller.abort();
      setStreamState("idle");
    };
  }, [apiKey, selectedId, shareToken]);

  useEffect(() => {
    if (!apiKey || !selectedId || !detail || shareToken) return;
    const activeTasks = detail.tasks.filter((task) => task.status === "queued" || task.status === "running");
    if (activeTasks.length === 0) return;
    const controller = new AbortController();
    const sessionId = selectedId;
    const timer = window.setInterval(() => {
      void refreshDetail(sessionId, { signal: controller.signal }).catch(() => undefined);
      for (const task of activeTasks) {
        void getTask(apiKey, task.id, { signal: controller.signal })
          .then((value) => {
            if (!controller.signal.aborted && selectedIdRef.current === sessionId) {
              setQueues((current) => ({ ...current, [task.id]: value.queue }));
            }
          })
          .catch(() => undefined);
      }
    }, 2500);
    return () => {
      controller.abort();
      window.clearInterval(timer);
    };
  }, [apiKey, detail, selectedId, shareToken]);

  useEffect(() => {
    if (!apiKey || !detail) return;
    const controller = new AbortController();
    const sessionId = detail.session.id;
    const pending = detail.artifacts.filter((artifact) => !thumbnailUrls[artifact.id]);
    for (const artifact of pending) {
      void fetchArtifactThumbnailBlob(apiKey, artifact.id, { signal: controller.signal })
        .then((blob) => {
          if (!controller.signal.aborted && selectedIdRef.current === sessionId) {
            setThumbnailUrls((current) => ({ ...current, [artifact.id]: URL.createObjectURL(blob) }));
          }
        })
        .catch(() => undefined);
    }
    for (const task of detail.tasks.filter((task) => task.status === "queued" || task.status === "running")) {
      void getTask(apiKey, task.id, { signal: controller.signal })
        .then((value) => {
          if (!controller.signal.aborted && selectedIdRef.current === sessionId) {
            setQueues((current) => ({ ...current, [task.id]: value.queue }));
          }
        })
        .catch(() => undefined);
    }
    return () => controller.abort();
  }, [apiKey, detail, thumbnailUrls]);

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
      activateSession(sessionItems[0]?.id || "");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function refreshSessions(options: { signal?: AbortSignal } = {}) {
    const items = (await listSessions(apiKey, "", options)).items;
    if (options.signal?.aborted) return items;
    setSessions(items);
    return items;
  }

  async function refreshDetail(sessionId: string, options: { signal?: AbortSignal; showLoading?: boolean } = {}) {
    const requestSeq = ++detailRequestSeq.current;
    if (options.showLoading) {
      setDetailLoading(true);
    }
    let value: SessionDetail;
    try {
      value = await getSession(apiKey, sessionId, options);
    } catch (err) {
      if (options.signal?.aborted || isAbortError(err)) {
        if (options.showLoading && requestSeq === detailRequestSeq.current) {
          setDetailLoading(false);
        }
        return null;
      }
      if (options.showLoading && requestSeq === detailRequestSeq.current) {
        setDetailLoading(false);
      }
      throw err;
    }
    if (
      options.signal?.aborted ||
      requestSeq !== detailRequestSeq.current ||
      selectedIdRef.current !== sessionId
    ) {
      return null;
    }
    setDetail(value);
    setDetailLoading(false);
    return value;
  }

  function activateSession(sessionId: string, initialDetail: SessionDetail | null = null) {
    detailRequestSeq.current += 1;
    selectedIdRef.current = sessionId;
    setSelectedId(sessionId);
    setDetail(initialDetail);
    setDetailLoading(Boolean(sessionId) && !initialDetail);
    setError("");
  }

  function handleSse(raw: string, sessionId: string) {
    if (selectedIdRef.current !== sessionId) return;
    const event = raw.match(/^event:\s*(.+)$/m)?.[1]?.trim() || "message";
    const data = raw
      .split(/\r?\n/)
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trim())
      .join("\n");
    if (!data) return;
    if (event === "snapshot") {
      setDetail(JSON.parse(data) as SessionDetail);
      scheduleSessionsRefresh();
    }
    if (event === "task_event") {
      const item = JSON.parse(data) as TaskEventRecord;
      setEvents((current) => ({ ...current, [item.task_id]: [...(current[item.task_id] || []), item] }));
      void refreshDetail(sessionId).catch(() => undefined);
    }
  }

  function scheduleSessionsRefresh() {
    if (streamRefreshTimer.current) window.clearTimeout(streamRefreshTimer.current);
    streamRefreshTimer.current = window.setTimeout(() => {
      void refreshSessions().catch(() => undefined);
    }, 400);
  }

  function newChat() {
    activateSession("");
  }

  async function send(payload: { text: string; model: string; n: number; size: ImageSize; file?: File | null }) {
    setBusy(true);
    setError("");
    try {
      let sessionId = selectedId;
      let currentDetail = detail;
      let session = currentDetail?.session || sessions.find((item) => item.id === selectedId) || null;
      const title = titleFromPrompt(payload.text);

      if (!sessionId) {
        const created = await createSession(apiKey, title);
        session = created.session;
        sessionId = created.session.id;
        const createdDetail = { session: created.session, messages: [], tasks: [], artifacts: [] };
        activateSession(sessionId, createdDetail);
        setSessions((current) => [created.session, ...current.filter((item) => item.id !== created.session.id)]);
        currentDetail = createdDetail;
      } else if (session && shouldRetitleSession(session, currentDetail)) {
        const renamed = await patchSession(apiKey, sessionId, { title });
        session = renamed.session;
        currentDetail = renamed;
        setDetail(renamed);
        setSessions((current) => current.map((item) => (item.id === renamed.session.id ? renamed.session : item)));
      }

      if (mode === "edit" && payload.file) {
        const submitted = await submitEditMessage(
          apiKey,
          sessionId,
          payload.text,
          payload.model,
          payload.n,
          payload.size,
          payload.file,
        );
        setQueues((current) => ({ ...current, [submitted.task.id]: submitted.queue }));
        if (session) setDetail(optimisticDetail(session, currentDetail, submitted));
        await refreshDetail(sessionId);
      } else {
        const submitted = await submitImageMessage(
          apiKey,
          sessionId,
          payload.text,
          payload.model,
          payload.n,
          payload.size,
        );
        setQueues((current) => ({ ...current, [submitted.task.id]: submitted.queue }));
        if (session) setDetail(optimisticDetail(session, currentDetail, submitted));
        await refreshDetail(sessionId);
      }
      void refreshKeyInfo().catch(() => undefined);
      await refreshSessions();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function removeSession(session: SessionRecord) {
    const confirmed = window.confirm(`Delete "${session.title}"? This will hide the session from your list.`);
    if (!confirmed) return;
    setBusy(true);
    setError("");
    try {
      await deleteSession(apiKey, session.id);
      const nextSessions = sessions.filter((item) => item.id !== session.id);
      setSessions(nextSessions);
      if (selectedId === session.id) {
        activateSession(nextSessions[0]?.id || "");
      }
      await refreshSessions();
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

  async function refreshKeyInfo() {
    if (!apiKey) return;
    const value = await verifyKey(apiKey);
    setKeyInfo(value);
  }

  async function loadUsage(offset = usageOffset, query = usageQuery) {
    setUsageBusy(true);
    setUsageError("");
    try {
      const value = await fetchMyUsageEvents(apiKey, offset, 50, query);
      setUsageData(value);
      setKeyInfo(value.key);
      setUsageOffset(value.offset);
    } catch (err) {
      setUsageError(err instanceof Error ? err.message : String(err));
    } finally {
      setUsageBusy(false);
    }
  }

  function openUsage() {
    setUsageOpen(true);
    void loadUsage(0, usageQuery);
  }

  async function loadShare(token: string) {
    setBusy(true);
    try {
      const value = await getShare(token);
      setShare(value);
      for (const artifact of value.artifacts) {
        const blob = await fetchSharedArtifactThumbnailBlob(token, artifact.id);
        setShareThumbnailUrls((current) => ({ ...current, [artifact.id]: URL.createObjectURL(blob) }));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  function openOriginalPreview(artifact: ImageArtifactRecord) {
    const caption = artifact.revised_prompt || "Generated image";
    const downloadName = downloadNameForArtifact(artifact);
    const previewUrl = thumbnailUrls[artifact.id] || originalUrls[artifact.id] || "";
    const existingOriginal = originalUrls[artifact.id];
    setLightbox({
      artifactId: artifact.id,
      previewUrl,
      originalUrl: existingOriginal,
      caption,
      downloadName,
      loading: !existingOriginal,
    });
    if (existingOriginal || originalLoading[artifact.id]) return;
    setOriginalLoading((current) => ({ ...current, [artifact.id]: true }));
    void fetchArtifactBlob(apiKey, artifact.id)
      .then((blob) => {
        const url = URL.createObjectURL(blob);
        setOriginalUrls((current) => ({ ...current, [artifact.id]: url }));
        setLightbox((current) =>
          current?.artifactId === artifact.id ? { ...current, originalUrl: url, loading: false } : current,
        );
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : String(err));
        setLightbox((current) =>
          current?.artifactId === artifact.id ? { ...current, loading: false } : current,
        );
      })
      .finally(() => {
        setOriginalLoading((current) => ({ ...current, [artifact.id]: false }));
      });
  }

  function logout() {
    localStorage.removeItem(STORAGE_KEY);
    setApiKey("");
    setDraftKey("");
    setKeyInfo(null);
    setDetail(null);
    setSessions([]);
    setSelectedId("");
  }

  if (shareToken) {
    return (
      <ShareView
        share={share}
        token={shareToken}
        thumbnailUrls={shareThumbnailUrls}
        error={error}
        busy={busy}
      />
    );
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
        onSelect={(session) => {
          if (session.id === selectedId) return;
          activateSession(session.id);
        }}
        onDelete={(session) => void removeSession(session)}
        onNew={() => void newChat()}
        onLogout={logout}
      />
      <section className="conversation">
        <header className="conversation-header">
          <div>
            <span className="eyebrow">{keyInfo.name} · {keyInfo.role}</span>
            <h2>{detail?.session.title || "New image session"}</h2>
          </div>
          <button type="button" className="usage-button" onClick={openUsage} title="Usage and credits">
            <Coins size={15} />
            {`${keyInfo.quota_used_calls}/${keyInfo.quota_total_calls}`}
          </button>
          <span className={`stream-state ${streamState}`}>{streamState === "live" ? "Live" : streamState === "reconnecting" ? "Reconnecting" : "Idle"}</span>
          <NotificationControl keyInfo={keyInfo} onSave={(email, enabled) => void saveNotification(email, enabled)} />
        </header>
        {error && <p className="error-line top-error">{error}</p>}
        <MessageStream
          detail={detail}
          loading={detailLoading}
          hasSelection={Boolean(selectedId)}
          thumbnailUrls={thumbnailUrls}
          originalLoading={originalLoading}
          queues={queues}
          events={events}
          onPreview={openOriginalPreview}
          onCancel={(taskId) => void cancelTask(apiKey, taskId).then(() => refreshDetail(selectedId))}
        />
        <Composer
          disabled={busy}
          mode={mode}
          creditContext={estimateSessionCreditContext(detail)}
          onModeChange={setMode}
          onSubmit={send}
        />
      </section>
      <AdminPanel apiKey={apiKey} role={keyInfo.role} />
      {usageOpen && (
        <UsagePanel
          data={usageData}
          query={usageQuery}
          busy={usageBusy}
          error={usageError}
          onQueryChange={setUsageQuery}
          onSearch={() => void loadUsage(0, usageQuery)}
          onRefresh={() => void loadUsage(usageOffset, usageQuery)}
          onPage={(offset) => void loadUsage(offset, usageQuery)}
          onClose={() => setUsageOpen(false)}
        />
      )}
      {lightbox && (
        <ImageLightbox
          previewUrl={lightbox.previewUrl}
          originalUrl={lightbox.originalUrl}
          caption={lightbox.caption}
          downloadName={lightbox.downloadName}
          loading={lightbox.loading}
          onClose={() => setLightbox(null)}
        />
      )}
    </main>
  );
}

function MessageStream({
  detail,
  loading,
  hasSelection,
  thumbnailUrls,
  originalLoading,
  queues,
  events,
  onPreview,
  onCancel,
}: {
  detail: SessionDetail | null;
  loading: boolean;
  hasSelection: boolean;
  thumbnailUrls: Record<string, string>;
  originalLoading: Record<string, boolean>;
  queues: Record<string, QueueSnapshot>;
  events: Record<string, TaskEventRecord[]>;
  onPreview: (artifact: ImageArtifactRecord) => void;
  onCancel: (taskId: string) => void;
}) {
  const streamRef = useRef<HTMLDivElement>(null);
  const artifactsByMessage = useMemo(() => {
    const map = new Map<string, ImageArtifactRecord[]>();
    for (const artifact of detail?.artifacts || []) {
      map.set(artifact.message_id, [...(map.get(artifact.message_id) || []), artifact]);
    }
    return map;
  }, [detail]);
  const activityKey = useMemo(() => {
    if (!detail) return "empty";
    const tasks = detail.tasks.map((task) => `${task.id}:${task.status}:${task.phase}`).join("|");
    return `${detail.messages.length}:${detail.artifacts.length}:${tasks}`;
  }, [detail]);

  useEffect(() => {
    streamRef.current?.scrollTo({ top: streamRef.current.scrollHeight, behavior: "smooth" });
  }, [activityKey]);

  if (!detail && loading) return <ConversationSkeleton />;
  if (!detail) {
    return (
      <div className="empty-state">
        {hasSelection ? "Loading conversation..." : "Start a new image session with a prompt."}
      </div>
    );
  }
  return (
    <div ref={streamRef} className="message-stream">
      {detail.messages.map((message) => {
        const parsed = parseMessage(message.content_json);
        const task = detail.tasks.find((item) => item.message_id === message.id);
        const artifacts = artifactsByMessage.get(message.id) || [];
        return (
          <article key={message.id} className={`message ${message.role}`}>
            <div className="message-bubble">
              {parsed.blocks.map((block, index) => <p key={index}>{displayBlockText(block.text)}</p>)}
              {task && <TaskCreditBadge task={task} />}
              {task && task.status !== "succeeded" && (
                <PendingImageCard task={task} queue={queues[task.id]} events={events[task.id] || []} onCancel={onCancel} />
              )}
              {artifacts.length > 0 && (
                <div className="artifact-grid">
                  {artifacts.map((artifact) => (
                    <figure key={artifact.id}>
                      {thumbnailUrls[artifact.id] ? (
                        <GeneratedImage
                          artifact={artifact}
                          thumbnailUrl={thumbnailUrls[artifact.id]}
                          originalLoading={Boolean(originalLoading[artifact.id])}
                          onPreview={() => onPreview(artifact)}
                        />
                      ) : (
                        <ImageLoadingPlaceholder />
                      )}
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

function GeneratedImage({
  artifact,
  thumbnailUrl,
  originalLoading,
  onPreview,
}: {
  artifact: ImageArtifactRecord;
  thumbnailUrl: string;
  originalLoading?: boolean;
  onPreview: () => void;
}) {
  const [loaded, setLoaded] = useState(false);
  const caption = artifact.revised_prompt || "Generated image";
  return (
    <div className={`image-frame ${loaded ? "loaded" : "loading"}`}>
      <button type="button" className="image-preview-button" onClick={onPreview} title="Open original image">
        {!loaded && <ImageLoadingOverlay />}
        <img src={thumbnailUrl} alt={caption} onLoad={() => setLoaded(true)} />
        <span className="thumbnail-badge">Thumbnail</span>
      </button>
      <div className="image-actions">
        <button
          type="button"
          onClick={onPreview}
          title={originalLoading ? "Loading original image" : "Open original image"}
        >
          {originalLoading ? <Loader2 className="spin" size={15} /> : <Expand size={15} />}
        </button>
      </div>
      <div className="thumbnail-note">
        <ImageIcon size={13} />
        <span>Thumbnail preview. Click to load original.</span>
      </div>
    </div>
  );
}

function ImageLoadingPlaceholder() {
  return (
    <div className="image-placeholder image-loading-surface">
      <Loader2 className="spin" size={22} />
    </div>
  );
}

function ImageLoadingOverlay({ label }: { label?: string }) {
  return (
    <div className="image-loading-overlay">
      <Loader2 className="spin" size={22} />
      {label && <span>{label}</span>}
    </div>
  );
}

function ConversationSkeleton() {
  return (
    <div className="message-stream loading-stream" aria-busy="true">
      <article className="message user">
        <div className="message-bubble skeleton-bubble narrow" />
      </article>
      <article className="message">
        <div className="message-bubble skeleton-bubble">
          <span />
          <span />
          <div className="skeleton-image" />
        </div>
      </article>
    </div>
  );
}

function TaskCreditBadge({ task }: { task: ImageTaskRecord }) {
  const billing = taskBilling(task);
  if (!billing) return null;
  return (
    <div className="task-credit-badge">
      <Coins size={14} />
      <strong>{`${billing.billableCredits} credits`}</strong>
      <span>{`${billing.size || "image"} · ${billing.requestedN} image${billing.requestedN === 1 ? "" : "s"}`}</span>
      {billing.contextSurcharge > 0 && <span>{`context +${billing.contextSurcharge}`}</span>}
    </div>
  );
}

function UsagePanel({
  data,
  query,
  busy,
  error,
  onQueryChange,
  onSearch,
  onRefresh,
  onPage,
  onClose,
}: {
  data: UsageEventsResponse | null;
  query: string;
  busy: boolean;
  error: string;
  onQueryChange: (query: string) => void;
  onSearch: () => void;
  onRefresh: () => void;
  onPage: (offset: number) => void;
  onClose: () => void;
}) {
  const key = data?.key;
  const used = key?.quota_used_calls ?? 0;
  const total = key?.quota_total_calls ?? 0;
  const remaining = Math.max(0, total - used);
  const percent = total > 0 ? Math.min(100, Math.round((used / total) * 100)) : 0;
  const offset = data?.offset ?? 0;
  const limit = data?.limit ?? 50;
  const events = (data?.events || []).filter((event) => (event.billable_credits || event.billable_images || 0) > 0);
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);
  return (
    <div className="usage-overlay" role="dialog" aria-modal="true" onClick={onClose}>
      <section className="usage-panel" onClick={(event) => event.stopPropagation()}>
        <header className="usage-panel-header">
          <div>
            <span className="eyebrow">Account credits</span>
            <h2>{key?.name || "Usage"}</h2>
          </div>
          <button type="button" className="icon-button" onClick={onClose} title="Close usage">
            <X size={17} />
          </button>
        </header>
        <div className="usage-summary-grid">
          <UsageSummary label="Used" value={String(used)} />
          <UsageSummary label="Remaining" value={String(remaining)} />
          <UsageSummary label="Total" value={String(total)} />
        </div>
        <div className="usage-meter" aria-label={`${percent}% used`}>
          <span style={{ width: `${percent}%` }} />
        </div>
        <div className="usage-toolbar">
          <div className="usage-search">
            <BarChart3 size={15} />
            <input
              value={query}
              onChange={(event) => onQueryChange(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") onSearch();
              }}
              placeholder="Search charged events"
            />
          </div>
          <button type="button" className="secondary-button" onClick={onSearch}>Search</button>
          <button type="button" className="icon-button" onClick={onRefresh} title="Refresh usage">
            {busy ? <Loader2 className="spin" size={15} /> : <RefreshCw size={15} />}
          </button>
        </div>
        {error && <p className="error-line">{error}</p>}
        <div className="usage-event-list">
          {events.length === 0 && !busy ? (
            <div className="usage-empty">No charged events for this filter.</div>
          ) : (
            events.map((event) => <UsageEventRow event={event} key={event.event_id} />)
          )}
        </div>
        <footer className="usage-pagination">
          <span>{`${data?.total ?? 0} events · offset ${offset}`}</span>
          <div>
            <button
              type="button"
              className="secondary-button"
              disabled={offset === 0 || busy}
              onClick={() => onPage(Math.max(0, offset - limit))}
            >
              Previous
            </button>
            <button
              type="button"
              className="secondary-button"
              disabled={!data?.has_more || busy}
              onClick={() => onPage(offset + limit)}
            >
              Next
            </button>
          </div>
        </footer>
      </section>
    </div>
  );
}

function UsageSummary({ label, value }: { label: string; value: string }) {
  return (
    <div className="usage-summary-card">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function UsageEventRow({ event }: { event: UsageEventRecord }) {
  const credits = event.billable_credits || event.billable_images || 0;
  const prompt = event.last_message_content || event.prompt_preview || event.request_url;
  return (
    <article className="usage-event-row">
      <div>
        <strong>{`${credits} credits`}</strong>
        <span>{formatTimestamp(event.created_at)}</span>
      </div>
      <p>{prompt || "No prompt captured"}</p>
      <div className="usage-event-meta">
        <span>{event.image_size || "-"}</span>
        <span>{`${event.generated_n}/${event.requested_n} images`}</span>
        {event.context_credit_surcharge > 0 && <span>{`context +${event.context_credit_surcharge}`}</span>}
      </div>
    </article>
  );
}

function ImageLightbox({
  previewUrl,
  originalUrl,
  caption,
  downloadName,
  loading,
  onClose,
}: {
  previewUrl: string;
  originalUrl?: string;
  caption: string;
  downloadName: string;
  loading: boolean;
  onClose: () => void;
}) {
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const displayUrl = originalUrl || previewUrl;
  return (
    <div className="lightbox" role="dialog" aria-modal="true" onClick={onClose}>
      <div className="lightbox-content" onClick={(event) => event.stopPropagation()}>
        <div className="lightbox-toolbar">
          <p>{caption}</p>
          {originalUrl ? (
            <a href={originalUrl} download={downloadName}>
              <Download size={16} />
              Download original
            </a>
          ) : (
            <button type="button" disabled>
              {loading && <Loader2 className="spin" size={16} />}
              {loading ? "Preparing original" : "Original unavailable"}
            </button>
          )}
          <button type="button" onClick={onClose} title="Close preview">
            <X size={18} />
          </button>
        </div>
        <div className="lightbox-image-frame">
          {displayUrl ? (
            <img className={originalUrl ? "" : "lightbox-preview"} src={displayUrl} alt={caption} />
          ) : (
            <div className="image-placeholder image-loading-surface" />
          )}
          {loading && <ImageLoadingOverlay label="Loading original..." />}
        </div>
      </div>
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
  token,
  thumbnailUrls,
  error,
  busy,
}: {
  share: ShareResponse | null;
  token: string;
  thumbnailUrls: Record<string, string>;
  error: string;
  busy: boolean;
}) {
  const [originalUrls, setOriginalUrls] = useState<Record<string, string>>({});
  const [originalLoading, setOriginalLoading] = useState<Record<string, boolean>>({});
  const [lightbox, setLightbox] = useState<ImageLightboxState | null>(null);
  function openSharedOriginal(artifact: ImageArtifactRecord) {
    const caption = artifact.revised_prompt || "Shared image";
    const downloadName = downloadNameForArtifact(artifact);
    const previewUrl = thumbnailUrls[artifact.id] || originalUrls[artifact.id] || "";
    const existingOriginal = originalUrls[artifact.id];
    setLightbox({
      artifactId: artifact.id,
      previewUrl,
      originalUrl: existingOriginal,
      caption,
      downloadName,
      loading: !existingOriginal,
    });
    if (existingOriginal || originalLoading[artifact.id]) return;
    setOriginalLoading((current) => ({ ...current, [artifact.id]: true }));
    void fetchSharedArtifactBlob(token, artifact.id)
      .then((blob) => {
        const url = URL.createObjectURL(blob);
        setOriginalUrls((current) => ({ ...current, [artifact.id]: url }));
        setLightbox((current) =>
          current?.artifactId === artifact.id ? { ...current, originalUrl: url, loading: false } : current,
        );
      })
      .catch(() => {
        setLightbox((current) =>
          current?.artifactId === artifact.id ? { ...current, loading: false } : current,
        );
      })
      .finally(() => {
        setOriginalLoading((current) => ({ ...current, [artifact.id]: false }));
      });
  }
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
                {thumbnailUrls[artifact.id] ? (
                  <GeneratedImage
                    artifact={artifact}
                    thumbnailUrl={thumbnailUrls[artifact.id]}
                    originalLoading={Boolean(originalLoading[artifact.id])}
                    onPreview={() => openSharedOriginal(artifact)}
                  />
                ) : (
                  <ImageLoadingPlaceholder />
                )}
                <figcaption>{artifact.revised_prompt || share.task.model}</figcaption>
              </figure>
            ))}
          </div>
        </section>
      )}
      {lightbox && (
        <ImageLightbox
          previewUrl={lightbox.previewUrl}
          originalUrl={lightbox.originalUrl}
          caption={lightbox.caption}
          downloadName={lightbox.downloadName}
          loading={lightbox.loading}
          onClose={() => setLightbox(null)}
        />
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

function displayBlockText(text: string) {
  if (isTransientImageTransportMessage(text)) {
    return "Image response stream was interrupted before completion. Please send again.";
  }
  return text;
}

function isTransientImageTransportMessage(text: string) {
  const normalized = text.toLowerCase();
  return normalized.includes("conversation body read failed")
    || normalized.includes("edit conversation body read failed")
    || normalized.includes("conversation request failed")
    || normalized.includes("conversation poll body read failed");
}

function estimateSessionCreditContext(detail: SessionDetail | null) {
  if (!detail) return { textCount: 0, imageCount: 0 };
  let textCount = 0;
  for (const message of [...detail.messages].reverse()) {
    if (message.status !== "done") continue;
    const parsed = parseMessage(message.content_json);
    for (const block of [...parsed.blocks].reverse()) {
      if (block.text.trim()) textCount += 1;
      if (textCount >= 8) break;
    }
    if (textCount >= 8) break;
  }
  return { textCount, imageCount: Math.min(detail.artifacts.length, 4) };
}

function taskBilling(task: ImageTaskRecord) {
  try {
    const request = JSON.parse(task.request_json || "{}") as {
      size?: string;
      n?: number;
      billing?: {
        billable_credits?: number;
        size_credit_units?: number;
        context_credit_surcharge?: number;
      };
    };
    const requestedN = Number(request.n || task.n || 1);
    const sizeUnits = Number(request.billing?.size_credit_units || 1);
    const contextSurcharge = Number(request.billing?.context_credit_surcharge || 0);
    const billableCredits = Number(
      request.billing?.billable_credits || requestedN * sizeUnits + contextSurcharge,
    );
    return {
      size: request.size,
      requestedN,
      sizeUnits,
      contextSurcharge,
      billableCredits,
    };
  } catch {
    return null;
  }
}

function formatTimestamp(seconds: number) {
  if (!seconds) return "-";
  return new Date(seconds * 1000).toLocaleString();
}

function titleFromPrompt(prompt: string) {
  const normalized = prompt.replace(/\s+/g, " ").trim();
  if (!normalized) return "New image session";
  return normalized.length > 48 ? `${normalized.slice(0, 48)}...` : normalized;
}

function shouldRetitleSession(session: SessionRecord, detail: SessionDetail | null) {
  const title = session.title.trim().toLowerCase();
  return (title === "" || title === "new chat" || title === "new image session") && (detail?.messages.length ?? 0) === 0;
}

function optimisticDetail(
  session: SessionRecord,
  detail: SessionDetail | null,
  submitted: ImageSubmissionResult,
): SessionDetail {
  const existingMessages = detail?.messages || [];
  const existingTasks = detail?.tasks || [];
  return {
    session,
    messages: [...existingMessages, submitted.user_message, submitted.assistant_message],
    tasks: [...existingTasks, submitted.task],
    artifacts: detail?.artifacts || [],
  };
}

function downloadNameForArtifact(artifact: ImageArtifactRecord) {
  const extension = artifact.mime_type.includes("jpeg")
    ? "jpg"
    : artifact.mime_type.includes("webp")
      ? "webp"
      : "png";
  return `gpt2api-${artifact.id}.${extension}`;
}

function shareTokenFromPath() {
  const match = window.location.pathname.match(/\/gpt2api\/share\/([^/]+)/);
  return match ? decodeURIComponent(match[1]) : "";
}

function sleep(ms: number) {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

function isAbortError(error: unknown) {
  return error instanceof DOMException && error.name === "AbortError";
}
