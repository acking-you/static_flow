import { ImagePlus, Paperclip, Send, Sparkles } from "lucide-react";
import { FormEvent, KeyboardEvent, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { ImageSize } from "../types";

export type ComposerMode = "image" | "edit";
const IMAGE_MODEL = "gpt-image-2";
const BASE_CREDIT_AREA = 1024 * 1024;
const MIN_SIZE = 256;
const MAX_SIZE = 4096;

export interface CreditEstimateContext {
  textCount: number;
  imageCount: number;
}

interface ComposerProps {
  disabled: boolean;
  mode: ComposerMode;
  creditContext: CreditEstimateContext;
  onModeChange: (mode: ComposerMode) => void;
  onSubmit: (payload: { text: string; model: string; n: number; size: ImageSize; file?: File | null }) => void;
}

export function Composer({ disabled, mode, creditContext, onModeChange, onSubmit }: ComposerProps) {
  const [text, setText] = useState("");
  const [n, setN] = useState(1);
  const [width, setWidth] = useState("1024");
  const [height, setHeight] = useState("1024");
  const [file, setFile] = useState<File | null>(null);
  const formRef = useRef<HTMLFormElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  function switchMode(next: ComposerMode) {
    onModeChange(next);
    if (next !== "edit") {
      setFile(null);
    }
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    if (!text.trim() || disabled) return;
    const size = normalizedSize(width, height);
    onSubmit({ text: text.trim(), model: IMAGE_MODEL, n, size, file });
    setText("");
    setFile(null);
    if (fileRef.current) fileRef.current.value = "";
  }

  const size = normalizedSize(width, height);
  const creditUnits = estimateCreditUnits(width, height);
  const contextSurcharge = estimateContextSurcharge(creditContext);
  const estimatedCredits = Math.max(1, n) * creditUnits + contextSurcharge;

  function submitOnEnter(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    formRef.current?.requestSubmit();
  }

  return (
    <form ref={formRef} className="composer" onSubmit={submit}>
      <div className="composer-toolbar">
        <Segment active={mode === "image"} onClick={() => switchMode("image")} icon={<Sparkles size={15} />} label="Image" />
        <Segment active={mode === "edit"} onClick={() => switchMode("edit")} icon={<ImagePlus size={15} />} label="Edit" />
        <input
          className="count-input"
          type="number"
          min={1}
          max={4}
          value={n}
          onChange={(event) => setN(Number(event.target.value))}
          aria-label="Image count"
        />
        <div className="size-inputs" title="Credits = ceil(width * height / 1024^2)">
          <input
            className="dimension-input"
            inputMode="numeric"
            value={width}
            onChange={(event) => setWidth(event.target.value)}
            aria-label="Image width"
          />
          <span>×</span>
          <input
            className="dimension-input"
            inputMode="numeric"
            value={height}
            onChange={(event) => setHeight(event.target.value)}
            aria-label="Image height"
          />
          <span className="size-cost">{`${size} · ${creditUnits} credit/image`}</span>
        </div>
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
      <div className="credit-estimate">
        <strong>{`${estimatedCredits} credits estimated`}</strong>
        <span>{`${n} image${n === 1 ? "" : "s"} × ${creditUnits}`}</span>
        {contextSurcharge > 0 && <span>{`context +${contextSurcharge}`}</span>}
      </div>
      {file && <div className="attachment-row">{file.name}</div>}
      <div className="composer-input-row">
        <textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={submitOnEnter}
          placeholder="Describe the image"
          rows={2}
        />
        <button className="send-button" disabled={disabled || !text.trim() || (mode === "edit" && !file)} title="Send">
          <Send size={17} />
        </button>
      </div>
    </form>
  );
}

function normalizedSize(width: string, height: string): ImageSize {
  return `${normalizeDimension(width)}x${normalizeDimension(height)}`;
}

function normalizeDimension(value: string): number {
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed)) return 1024;
  return Math.min(MAX_SIZE, Math.max(MIN_SIZE, parsed));
}

function estimateCreditUnits(width: string, height: string): number {
  const area = normalizeDimension(width) * normalizeDimension(height);
  return Math.max(1, Math.ceil(area / BASE_CREDIT_AREA));
}

function estimateContextSurcharge(context: CreditEstimateContext): number {
  const textCredit = context.textCount > 0 ? 1 : 0;
  const imageCredit = Math.min(context.imageCount, 3);
  return Math.min(textCredit + imageCredit, 4);
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
