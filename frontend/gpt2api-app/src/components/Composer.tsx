import { ImagePlus, MessageSquare, Paperclip, Send, Sparkles } from "lucide-react";
import { FormEvent, useRef, useState } from "react";
import type { ReactNode } from "react";

export type ComposerMode = "chat" | "image" | "edit";

interface ComposerProps {
  disabled: boolean;
  mode: ComposerMode;
  onModeChange: (mode: ComposerMode) => void;
  onSubmit: (payload: { text: string; model: string; n: number; file?: File | null }) => void;
}

export function Composer({ disabled, mode, onModeChange, onSubmit }: ComposerProps) {
  const [text, setText] = useState("");
  const [model, setModel] = useState(mode === "chat" ? "gpt-5" : "gpt-image-1");
  const [n, setN] = useState(1);
  const [file, setFile] = useState<File | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  function switchMode(next: ComposerMode) {
    onModeChange(next);
    setModel(next === "chat" ? "gpt-5" : "gpt-image-1");
    if (next !== "edit") {
      setFile(null);
    }
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    if (!text.trim() || disabled) return;
    onSubmit({ text: text.trim(), model, n, file });
    setText("");
    setFile(null);
    if (fileRef.current) fileRef.current.value = "";
  }

  return (
    <form className="composer" onSubmit={submit}>
      <div className="composer-toolbar">
        <Segment active={mode === "chat"} onClick={() => switchMode("chat")} icon={<MessageSquare size={15} />} label="Chat" />
        <Segment active={mode === "image"} onClick={() => switchMode("image")} icon={<Sparkles size={15} />} label="Image" />
        <Segment active={mode === "edit"} onClick={() => switchMode("edit")} icon={<ImagePlus size={15} />} label="Edit" />
        <select value={model} onChange={(event) => setModel(event.target.value)} aria-label="Model">
          {mode === "chat" ? (
            <>
              <option value="gpt-5">gpt-5</option>
              <option value="gpt-5-mini">gpt-5-mini</option>
              <option value="auto">auto</option>
            </>
          ) : (
            <>
              <option value="gpt-image-1">gpt-image-1</option>
              <option value="gpt-image-2">gpt-image-2</option>
            </>
          )}
        </select>
        {mode !== "chat" && (
          <input
            className="count-input"
            type="number"
            min={1}
            max={4}
            value={n}
            onChange={(event) => setN(Number(event.target.value))}
            aria-label="Image count"
          />
        )}
        {mode === "edit" && (
          <>
            <button type="button" className="icon-button" onClick={() => fileRef.current?.click()} title="Attach image">
              <Paperclip size={16} />
            </button>
            <input
              ref={fileRef}
              hidden
              type="file"
              accept="image/*"
              onChange={(event) => setFile(event.target.files?.[0] ?? null)}
            />
          </>
        )}
      </div>
      {file && <div className="attachment-row">{file.name}</div>}
      <div className="composer-input-row">
        <textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          placeholder={mode === "chat" ? "Message GPT2API" : "Describe the image"}
          rows={2}
        />
        <button className="send-button" disabled={disabled || !text.trim() || (mode === "edit" && !file)} title="Send">
          <Send size={17} />
        </button>
      </div>
    </form>
  );
}

function Segment({
  active,
  onClick,
  icon,
  label,
}: {
  active: boolean;
  onClick: () => void;
  icon: ReactNode;
  label: string;
}) {
  return (
    <button type="button" className={`segment ${active ? "active" : ""}`} onClick={onClick}>
      {icon}
      <span>{label}</span>
    </button>
  );
}
