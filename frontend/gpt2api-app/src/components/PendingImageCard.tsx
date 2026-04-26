import { CheckCircle2, Clock, Loader2, OctagonX, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { ImageTaskRecord, QueueSnapshot, TaskEventRecord } from "../types";

const phaseLabels: Record<string, string> = {
  queued: "Queued",
  allocating: "Starting",
  running: "Running",
  saving: "Finishing",
  succeeded: "Finishing",
  done: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
};

interface PendingImageCardProps {
  task: ImageTaskRecord;
  queue?: QueueSnapshot;
  events: TaskEventRecord[];
  onCancel: (taskId: string) => void;
}

export function PendingImageCard({ task, queue, events, onCancel }: PendingImageCardProps) {
  const [now, setNow] = useState(Date.now());
  const phase = task.phase || task.status;
  const started = (task.started_at || task.queue_entered_at) * 1000;
  const elapsed = Math.max(0, now - started);
  const eta = queue?.estimated_start_after_ms ?? task.estimated_start_after_ms ?? null;
  const tasksAhead = queue?.position_ahead ?? task.position_snapshot ?? 0;
  const cancellable = task.status === "queued";
  const succeeded = task.status === "succeeded";
  const terminal = succeeded || task.status === "failed" || task.status === "cancelled";
  const etaLabel = eta
    ? formatDuration(eta)
    : task.status === "queued"
      ? "Pending slot"
      : terminal
        ? "Stopped"
        : "Waiting";

  useEffect(() => {
    if (terminal) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [terminal]);

  return (
    <section className={`pending-image ${task.status}`}>
      <div className="pending-heading">
        <div>
          <span className="eyebrow">Image task</span>
          <h3>{phaseLabels[phase] || phase}</h3>
        </div>
        {task.status === "failed" ? (
          <OctagonX size={20} />
        ) : task.status === "cancelled" ? (
          <X size={20} />
        ) : succeeded ? (
          <CheckCircle2 size={20} />
        ) : task.status === "queued" ? (
          <Clock size={20} />
        ) : (
          <Loader2 className="spin" size={20} />
        )}
      </div>
      <div className="pending-grid">
        <Metric label="Ahead" value={String(tasksAhead)} />
        <Metric label="Elapsed" value={formatDuration(elapsed)} />
        <Metric label="ETA" value={etaLabel} />
      </div>
      <div className="progress-track">
        <span style={{ width: `${progressForPhase(phase)}%` }} />
      </div>
      <div className="pending-log">
        {events.length === 0 ? (
          <p>
            <Clock size={14} /> {emptyLogMessage(task.status)}
          </p>
        ) : (
          events.slice(-4).map((event) => <p key={event.id}>{event.event_kind}</p>)
        )}
      </div>
      {task.error_message && <p className="error-line">{displayTaskError(task.error_message)}</p>}
      {cancellable && (
        <button type="button" className="cancel-button" onClick={() => onCancel(task.id)}>
          <X size={15} />
          Cancel
        </button>
      )}
    </section>
  );
}

function emptyLogMessage(status: string) {
  if (status === "queued") {
    return "Waiting for an available image slot";
  }
  if (status === "cancelled") {
    return "Task was cancelled";
  }
  if (status === "failed") {
    return "Task failed";
  }
  return "Waiting for the first worker update";
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function progressForPhase(phase: string) {
  switch (phase) {
    case "queued":
      return 16;
    case "allocating":
      return 35;
    case "running":
      return 64;
    case "saving":
    case "succeeded":
      return 86;
    case "done":
    case "failed":
    case "cancelled":
      return 100;
    default:
      return 22;
  }
}

function displayTaskError(message: string) {
  const normalized = message.toLowerCase();
  if (
    normalized.includes("conversation body read failed") ||
    normalized.includes("edit conversation body read failed") ||
    normalized.includes("conversation request failed") ||
    normalized.includes("conversation poll body read failed")
  ) {
    return "Image response stream was interrupted before completion. Please send again.";
  }
  return message;
}

function formatDuration(ms: number) {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}m ${seconds % 60}s`;
}
