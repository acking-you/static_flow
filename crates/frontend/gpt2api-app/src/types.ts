export type KeyRole = "user" | "admin";
export type ImageSize = string;

export interface ProductKey {
  id: string;
  name: string;
  status: string;
  role: KeyRole;
  quota_total_calls: number;
  quota_used_calls: number;
  route_strategy: string;
  account_group_id?: string | null;
  request_max_concurrency?: number | null;
  request_min_start_interval_ms?: number | null;
  notification_email?: string | null;
  notification_enabled: boolean;
  secret_plaintext?: string | null;
}

export interface SessionRecord {
  id: string;
  key_id: string;
  title: string;
  source: "web" | "api";
  status: string;
  created_at: number;
  updated_at: number;
  last_message_at?: number | null;
}

export interface MessageRecord {
  id: string;
  session_id: string;
  key_id: string;
  role: "user" | "assistant" | string;
  content_json: string;
  status: "pending" | "streaming" | "done" | "failed";
  created_at: number;
  updated_at: number;
}

export type ImageTaskStatus = "queued" | "running" | "succeeded" | "failed" | "cancelled";

export interface ImageTaskRecord {
  id: string;
  session_id: string;
  message_id: string;
  key_id: string;
  status: ImageTaskStatus;
  mode: string;
  prompt: string;
  model: string;
  n: number;
  request_json: string;
  phase: string;
  queue_entered_at: number;
  started_at?: number | null;
  finished_at?: number | null;
  position_snapshot?: number | null;
  estimated_start_after_ms?: number | null;
  error_code?: string | null;
  error_message?: string | null;
}

export interface QueueSnapshot {
  task: ImageTaskRecord;
  position_ahead: number;
  estimated_start_after_ms?: number | null;
}

export interface AdminQueueSnapshot {
  running: ImageTaskRecord[];
  queued: ImageTaskRecord[];
  global_image_concurrency: number;
}

export interface ImageArtifactRecord {
  id: string;
  task_id: string;
  session_id: string;
  message_id: string;
  key_id: string;
  relative_path: string;
  mime_type: string;
  sha256: string;
  size_bytes: number;
  width?: number | null;
  height?: number | null;
  revised_prompt?: string | null;
  created_at: number;
}

export interface SessionDetail {
  session: SessionRecord;
  messages: MessageRecord[];
  tasks: ImageTaskRecord[];
  artifacts: ImageArtifactRecord[];
}

export interface ImageSubmissionResult {
  user_message: MessageRecord;
  assistant_message: MessageRecord;
  task: ImageTaskRecord;
  queue: QueueSnapshot;
}

export interface TaskResponse {
  task: ImageTaskRecord;
  queue: QueueSnapshot;
}

export interface RuntimeConfig {
  global_image_concurrency: number;
  signed_link_ttl_seconds: number;
  queue_eta_window_size: number;
  image_task_timeout_seconds: number;
}

export interface AdminQueueResponse {
  queue: AdminQueueSnapshot;
  config: RuntimeConfig;
}

export interface ShareResponse {
  scope: string;
  session: SessionRecord;
  task: ImageTaskRecord;
  messages: MessageRecord[];
  artifacts: ImageArtifactRecord[];
}

export interface TaskEventRecord {
  sequence: number;
  id: string;
  task_id: string;
  session_id: string;
  key_id: string;
  event_kind: string;
  payload_json: string;
  created_at: number;
}

export interface UsageEventRecord {
  event_id: string;
  request_id: string;
  key_id: string;
  key_name: string;
  account_name: string;
  endpoint: string;
  request_method: string;
  request_url: string;
  requested_model: string;
  resolved_upstream_model: string;
  session_id?: string | null;
  task_id?: string | null;
  mode: string;
  image_size?: string | null;
  requested_n: number;
  generated_n: number;
  billable_images: number;
  billable_credits: number;
  size_credit_units: number;
  context_text_count: number;
  context_image_count: number;
  context_credit_surcharge: number;
  prompt_preview?: string | null;
  last_message_content?: string | null;
  status_code: number;
  latency_ms: number;
  error_code?: string | null;
  error_message?: string | null;
  created_at: number;
}

export interface UsageEventsResponse {
  key: ProductKey;
  total: number;
  offset: number;
  limit: number;
  has_more: boolean;
  billable_credit_total: number;
  events: UsageEventRecord[];
}
