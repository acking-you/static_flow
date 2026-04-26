import type {
  AdminQueueResponse,
  ImageSubmissionResult,
  ImageSize,
  ProductKey,
  SessionDetail,
  SessionRecord,
  ShareResponse,
  TaskResponse,
  UsageEventsResponse,
} from "./types";

const API_BASE = "/api/gpt2api";

interface RequestOptions {
  signal?: AbortSignal;
}

export function authHeaders(key: string): HeadersInit {
  return {
    authorization: `Bearer ${key}`,
    "content-type": "application/json",
  };
}

export async function fetchJson<T>(path: string, key: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    ...init,
    headers: {
      ...authHeaders(key),
      ...(init.headers || {}),
    },
  });
  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `HTTP ${response.status}`);
  }
  return (await response.json()) as T;
}

export async function verifyKey(key: string): Promise<ProductKey> {
  const value = await fetchJson<{ key: ProductKey }>("/auth/verify", key, {
    method: "POST",
    body: "{}",
  });
  return value.key;
}

export function listSessions(key: string, query = "", options: RequestOptions = {}) {
  return fetchJson<{ items: SessionRecord[] }>(`/sessions?limit=80${query}`, key, {
    signal: options.signal,
  });
}

export function createSession(key: string, title = "New image session") {
  return fetchJson<{ session: SessionRecord }>("/sessions", key, {
    method: "POST",
    body: JSON.stringify({ title }),
  });
}

export function getSession(key: string, sessionId: string, options: RequestOptions = {}) {
  return fetchJson<SessionDetail>(`/sessions/${encodeURIComponent(sessionId)}`, key, {
    signal: options.signal,
  });
}

export function patchSession(key: string, sessionId: string, patch: { title?: string; status?: string }) {
  return fetchJson<SessionDetail>(`/sessions/${encodeURIComponent(sessionId)}`, key, {
    method: "PATCH",
    body: JSON.stringify(patch),
  });
}

export function deleteSession(key: string, sessionId: string) {
  return fetchJson<{ deleted: boolean; id: string }>(`/sessions/${encodeURIComponent(sessionId)}`, key, {
    method: "DELETE",
    body: "{}",
  });
}

export function submitTextMessage(key: string, sessionId: string, text: string, model: string) {
  return fetchJson<SessionDetail>(`/sessions/${encodeURIComponent(sessionId)}/messages`, key, {
    method: "POST",
    body: JSON.stringify({ kind: "text", text, model }),
  });
}

export function submitImageMessage(
  key: string,
  sessionId: string,
  prompt: string,
  model: string,
  n: number,
  size: ImageSize,
) {
  return fetchJson<ImageSubmissionResult>(`/sessions/${encodeURIComponent(sessionId)}/messages`, key, {
    method: "POST",
    body: JSON.stringify({ kind: "image_generation", prompt, model, n, size }),
  });
}

export async function submitEditMessage(
  key: string,
  sessionId: string,
  prompt: string,
  model: string,
  n: number,
  size: ImageSize,
  file: File,
) {
  const form = new FormData();
  form.set("prompt", prompt);
  form.set("model", model);
  form.set("n", String(n));
  form.set("size", size);
  form.set("image", file);
  const response = await fetch(`${API_BASE}/sessions/${encodeURIComponent(sessionId)}/messages/edit`, {
    method: "POST",
    headers: { authorization: `Bearer ${key}` },
    body: form,
  });
  if (!response.ok) {
    throw new Error(await response.text());
  }
  return (await response.json()) as ImageSubmissionResult;
}

export function cancelTask(key: string, taskId: string) {
  return fetchJson<{ cancelled: boolean }>(`/tasks/${encodeURIComponent(taskId)}/cancel`, key, {
    method: "POST",
    body: "{}",
  });
}

export function getTask(key: string, taskId: string, options: RequestOptions = {}) {
  return fetchJson<TaskResponse>(`/tasks/${encodeURIComponent(taskId)}`, key, {
    signal: options.signal,
  });
}

export async function fetchArtifactBlob(
  key: string,
  artifactId: string,
  options: RequestOptions = {},
): Promise<Blob> {
  const response = await fetch(`${API_BASE}/artifacts/${encodeURIComponent(artifactId)}`, {
    headers: { authorization: `Bearer ${key}` },
    signal: options.signal,
  });
  if (!response.ok) {
    throw new Error(await response.text());
  }
  return await response.blob();
}

export function updateNotification(key: string, email: string, enabled: boolean) {
  return fetchJson<{ key: ProductKey }>("/me/notification", key, {
    method: "PATCH",
    body: JSON.stringify({ notification_email: email, notification_enabled: enabled }),
  });
}

export function fetchMyUsageEvents(key: string, offset = 0, limit = 50, query = "") {
  const params = new URLSearchParams({ offset: String(offset), limit: String(limit) });
  if (query.trim()) params.set("q", query.trim());
  return fetchJson<UsageEventsResponse>(`/me/usage/events?${params.toString()}`, key);
}

export function adminSessions(key: string, params: URLSearchParams) {
  return fetchJson<{ items: SessionRecord[] }>(`/admin/sessions?${params.toString()}`, key);
}

export function adminQueue(key: string) {
  return fetchJson<AdminQueueResponse>("/admin/queue", key);
}

export function patchAdminQueue(key: string, globalImageConcurrency: number, imageTaskTimeoutSeconds: number) {
  return fetchJson<{
    config: { global_image_concurrency: number; image_task_timeout_seconds: number };
  }>("/admin/queue/config", key, {
    method: "PATCH",
    body: JSON.stringify({
      global_image_concurrency: globalImageConcurrency,
      image_task_timeout_seconds: imageTaskTimeoutSeconds,
    }),
  });
}

export function adminCancelTask(key: string, taskId: string) {
  return fetchJson<{ cancelled: boolean }>(`/admin/tasks/${encodeURIComponent(taskId)}/cancel`, key, {
    method: "POST",
    body: "{}",
  });
}

export function adminKeys(key: string) {
  return fetchJson<ProductKey[]>("/admin/keys", key);
}

export function patchAdminKey(key: string, keyId: string, patch: Partial<ProductKey>) {
  return fetchJson<ProductKey>(`/admin/keys/${encodeURIComponent(keyId)}`, key, {
    method: "PATCH",
    body: JSON.stringify(patch),
  });
}

export async function openSessionEventStream(
  sessionId: string,
  key: string,
  options: RequestOptions = {},
): Promise<ReadableStream<Uint8Array>> {
  const response = await fetch(`${API_BASE}/sessions/${encodeURIComponent(sessionId)}/events`, {
    headers: { authorization: `Bearer ${key}` },
    signal: options.signal,
  });
  if (!response.ok || !response.body) {
    throw new Error(`event stream failed: HTTP ${response.status}`);
  }
  return response.body;
}

export function getShare(token: string) {
  return fetch(`${API_BASE}/share/${encodeURIComponent(token)}`).then(async (response) => {
    if (!response.ok) {
      throw new Error(await response.text());
    }
    return (await response.json()) as ShareResponse;
  });
}

export async function fetchSharedArtifactBlob(token: string, artifactId: string): Promise<Blob> {
  const response = await fetch(
    `${API_BASE}/share/${encodeURIComponent(token)}/artifacts/${encodeURIComponent(artifactId)}`,
  );
  if (!response.ok) {
    throw new Error(await response.text());
  }
  return await response.blob();
}
