import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ClipResult } from "../hooks/useSearch";

interface ResultRowProps {
  result: ClipResult;
  isSelected: boolean;
  index: number;
  onClick: () => void;
  onDoubleClick: () => void;
}

function timeAgo(timestamp: number): string {
  const now = Math.floor(Date.now() / 1000);
  const diff = now - timestamp;
  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`;
  return `${Math.floor(diff / 604800)}w ago`;
}

function getTypeBadge(type: string): string | null {
  switch (type) {
    case "url": return "URL";
    case "code": return "Code";
    case "image": return "IMG";
    default: return null;
  }
}

function getDomain(url: string): string {
  try { return new URL(url).hostname; } catch { return url; }
}

function cleanPreview(text: string): string {
  return text.replace(/[\r\n]+/g, " ").replace(/\s+/g, " ").trim();
}

function ResultRow({ result, isSelected, index, onClick, onDoubleClick }: ResultRowProps) {
  const badge = getTypeBadge(result.content_type);
  const preview = cleanPreview(result.content);
  const [imageThumbnail, setImageThumbnail] = useState<string | null>(null);

  // Fetch image thumbnail when selected
  useEffect(() => {
    if (isSelected && result.content_type === "image" && !imageThumbnail) {
      invoke<string | null>("get_image_thumbnail", { clipId: result.id })
        .then((data) => {
          if (data) setImageThumbnail(data);
        })
        .catch(() => {});
    }
  }, [isSelected, result.content_type, result.id, imageThumbnail]);

  return (
    <div
      data-no-drag
      className={`flex items-center gap-3 px-4 cursor-pointer transition-colors duration-75 rounded-lg mx-1 ${
        isSelected ? "bg-white/[0.12]" : "hover:bg-white/[0.05]"
      } ${result.content_type === "image" && isSelected && imageThumbnail ? "py-2 min-h-[80px]" : "h-full"}`}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
    >
      {/* Number hint */}
      {index < 9 ? (
        <span className="row-number">{index + 1}</span>
      ) : (
        <span className="w-[14px] shrink-0" />
      )}

      {/* Content — clamped to 2 lines max, no overflow */}
      <div className="flex-1 min-w-0 overflow-hidden">
        {result.content_type === "url" ? (
          <div className="flex flex-col min-w-0">
            <span className="text-[13px] font-medium truncate text-white/90">{getDomain(result.content)}</span>
            <span className="text-[11px] text-white/35 truncate">{result.content}</span>
          </div>
        ) : result.content_type === "code" && result.content_highlighted ? (
          <div
            className="text-[12px] text-white/80 leading-snug line-clamp-2 overflow-hidden"
            dangerouslySetInnerHTML={{ __html: result.content_highlighted }}
          />
        ) : result.content_type === "image" && isSelected && imageThumbnail ? (
          <div className="flex items-center gap-3">
            <img
              src={`data:image/png;base64,${imageThumbnail}`}
              alt="Image preview"
              className="rounded-md border border-white/[0.08] max-h-[64px] max-w-[200px] object-contain"
            />
            <span className="text-[12px] text-white/40">Image</span>
          </div>
        ) : result.content_type === "image" ? (
          <div className="flex items-center gap-2">
            <div className="w-8 h-8 rounded bg-white/[0.06] flex items-center justify-center text-[10px] text-white/40">IMG</div>
            <span className="text-[13px] text-white/60">Image</span>
          </div>
        ) : (
          <span className="text-[13px] text-white/85 leading-snug line-clamp-2 overflow-hidden block">
            {preview}
          </span>
        )}
      </div>

      {/* Meta */}
      <div className="flex flex-col items-end shrink-0 gap-0.5">
        {result.source_app && (
          <span className="text-[11px] text-white/35 truncate max-w-[100px]">{result.source_app}</span>
        )}
        <span className="text-[11px] text-white/30">{timeAgo(result.created_at)}</span>
      </div>

      {/* Badges + App icon on the right */}
      <div className="flex items-center gap-1.5 shrink-0">
        {badge && (
          <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-white/[0.08] text-white/45">{badge}</span>
        )}
        {result.pinned && <span className="text-[10px] text-white/40">📌</span>}
        {/* Source app icon — on the right */}
        <div className="w-4 h-4 flex items-center justify-center">
          {result.source_app_icon ? (
            <img
              src={`data:image/png;base64,${result.source_app_icon}`}
              alt=""
              className="w-4 h-4 rounded-sm"
            />
          ) : (
            <div className="w-4 h-4 rounded bg-white/[0.06]" />
          )}
        </div>
      </div>
    </div>
  );
}

export default ResultRow;
