import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X, Edit3, Save, ExternalLink, Copy } from "lucide-react";
import type { ClipResult } from "../hooks/useSearch";

interface DetailPanelProps {
  clip: ClipResult;
  onClose: () => void;
  onEdit: (clipId: number, newContent: string) => void;
  onCopy: (clipId: number) => void;
}

function formatTimestamp(ts: number): string {
  const d = new Date(ts * 1000);
  const now = new Date();
  const isToday =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();

  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

  if (isToday) return `Today at ${time}`;

  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  if (
    d.getFullYear() === yesterday.getFullYear() &&
    d.getMonth() === yesterday.getMonth() &&
    d.getDate() === yesterday.getDate()
  ) {
    return `Yesterday at ${time}`;
  }

  return d.toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" }) + ` at ${time}`;
}

function ImageDetail({ clipId }: { clipId: number }) {
  const [preview, setPreview] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    invoke<string | null>("get_image_preview", { clipId, maxSize: 400 })
      .then((result) => {
        if (!cancelled) setPreview(result);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [clipId]);

  if (!preview) return null;

  return (
    <img
      src={`data:image/png;base64,${preview}`}
      alt=""
      className="max-h-[260px] max-w-full rounded-lg object-contain"
      style={{ border: "0.5px solid var(--border-default)" }}
    />
  );
}

export default function DetailPanel({ clip, onClose, onEdit, onCopy }: DetailPanelProps) {
  const [editing, setEditing] = useState(false);
  const [editContent, setEditContent] = useState(clip.content);

  useEffect(() => {
    setEditing(false);
    setEditContent(clip.content);
  }, [clip.id]);

  const handleSave = () => {
    if (editContent !== clip.content) {
      onEdit(clip.id, editContent);
    }
    setEditing(false);
  };

  const isUrl = clip.content_type === "url";
  const isCode = clip.content_type === "code";
  const isImage = clip.content_type === "image";
  const displayContent = isImage ? (clip.ocr_text || "[Image]") : clip.content;

  return (
    <div
      className="flex h-full flex-col overflow-hidden"
      style={{ borderLeft: "0.5px solid var(--border-subtle)" }}
    >
      {/* Header */}
      <div
        className="flex items-center justify-between px-3 py-2"
        style={{ borderBottom: "0.5px solid var(--border-subtle)" }}
      >
        <div className="flex items-center gap-2">
          {clip.source_app_icon && (
            <img
              src={`data:image/png;base64,${clip.source_app_icon}`}
              alt=""
              className="w-4 h-4 rounded-sm"
            />
          )}
          <span className="text-[12px] font-medium" style={{ color: "var(--text-primary)" }}>
            {clip.source_app || "Unknown"}
          </span>
        </div>
        <div className="flex items-center gap-1">
          {!editing && !isImage && (
            <button
              type="button"
              data-no-drag
              onClick={() => setEditing(true)}
              className="rounded p-1 transition-colors"
              style={{ color: "var(--text-tertiary)" }}
              title="Edit"
            >
              <Edit3 size={12} />
            </button>
          )}
          <button
            type="button"
            data-no-drag
            onClick={onClose}
            className="rounded p-1 transition-colors"
            style={{ color: "var(--text-tertiary)" }}
          >
            <X size={12} />
          </button>
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-3">
        {/* Image preview */}
        {isImage && (
          <div className="mb-3">
            <ImageDetail clipId={clip.id} />
          </div>
        )}

        {/* Text content */}
        {editing ? (
          <div className="flex flex-col gap-2">
            <textarea
              value={editContent}
              onChange={(e) => setEditContent(e.target.value)}
              className="w-full flex-1 resize-none rounded-lg p-2 text-[12px]"
              style={{
                background: "var(--settings-input-bg)",
                color: "var(--text-primary)",
                outline: "none",
                fontFamily: isCode ? "'SF Mono', monospace" : "inherit",
                minHeight: "200px",
              }}
              autoFocus
            />
            <div className="flex items-center gap-2">
              <button
                type="button"
                data-no-drag
                onClick={handleSave}
                className="inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-[11px]"
                style={{ background: "var(--accent-bg)", color: "var(--accent-text)" }}
              >
                <Save size={11} />
                Save
              </button>
              <button
                type="button"
                data-no-drag
                onClick={() => {
                  setEditing(false);
                  setEditContent(clip.content);
                }}
                className="rounded-md px-2.5 py-1 text-[11px]"
                style={{ background: "var(--surface-primary)", color: "var(--text-secondary)" }}
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <div
            className={`text-[12px] leading-relaxed ${isCode ? "code-preview" : ""}`}
            style={{
              color: "var(--text-secondary)",
              fontFamily: isCode ? "'SF Mono', monospace" : "inherit",
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
            }}
          >
            {isCode && clip.content_highlighted ? (
              <div dangerouslySetInnerHTML={{ __html: clip.content_highlighted }} />
            ) : (
              displayContent
            )}
          </div>
        )}
      </div>

      {/* Footer actions */}
      <div
        className="flex items-center gap-2 px-3 py-2"
        style={{ borderTop: "0.5px solid var(--border-subtle)" }}
      >
        <button
          type="button"
          data-no-drag
          onClick={() => onCopy(clip.id)}
          className="inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-[11px]"
          style={{ background: "var(--surface-primary)", color: "var(--text-secondary)" }}
        >
          <Copy size={11} />
          Copy
        </button>
        {isUrl && (
          <button
            type="button"
            data-no-drag
            onClick={() => window.open(clip.content, "_blank")}
            className="inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-[11px]"
            style={{ background: "var(--surface-primary)", color: "var(--text-secondary)" }}
          >
            <ExternalLink size={11} />
            Open
          </button>
        )}
        <div className="flex-1" />
        <span className="text-[10px]" style={{ color: "var(--text-muted)" }}>
          {formatTimestamp(clip.created_at)}
        </span>
      </div>
    </div>
  );
}
