import { useState, useEffect, useCallback, useMemo, useRef, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Copy, Pin, PinOff, Trash2, X, Shuffle, FolderOpen, ExternalLink } from "lucide-react";
import { ClipResult, CollectionInfo } from "../hooks/useSearch";
import { transforms } from "../utils/transforms";
import { formatShortcut, isMacPlatform, normalizeAppName } from "../utils/platform";
import { getImagePreviewData, setImagePreviewData } from "./clipMediaCache";

export interface SheetAction {
  id: string;
  icon: ReactNode;
  label: string;
  shortcut: string;
  tone?: "default" | "danger";
  children?: SheetAction[];
}

interface ActionsSheetProps {
  clip: ClipResult;
  clipContent: string;
  thumbnail: string | null;
  actions: SheetAction[];
  selectedIndex: number;
  collections: CollectionInfo[];
  onClose: () => void;
  onSelect: (index: number) => void;
  onActivate: (index: number) => void;
  onTransform: (clipId: number, transformedContent: string) => void;
  onMoveToCollection: (clipId: number, collectionId: number | null) => void;
  onOpenUrl: (url: string) => void;
}

function wrapIndex(current: number, delta: number, total: number): number {
  if (total <= 0) return 0;
  return (current + delta + total) % total;
}

function previewText(clip: ClipResult): string {
  const source = clip.content_type === "image" ? clip.ocr_text || "Image clip" : clip.content;
  return source.replace(/[\r\n]+/g, " ").replace(/\s+/g, " ").trim();
}

function actionShortcut(shortcut: string): string {
  return formatShortcut(shortcut);
}

function ActionButton({
  icon,
  label,
  shortcut,
  selected,
  tone = "default",
  onMouseEnter,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  shortcut: string;
  selected: boolean;
  tone?: "default" | "danger";
  onMouseEnter: () => void;
  onClick: () => void;
}) {
  const toneStyle = (() => {
    if (tone === "danger") {
      return selected
        ? { borderColor: "var(--danger-border)", background: "var(--danger-bg)", color: "var(--danger-text)" }
        : { borderColor: "var(--border-default)", background: "var(--surface-secondary)", color: "var(--danger-text)" };
    }

    return selected
      ? { borderColor: "var(--accent-border)", background: "var(--accent-bg)", color: "var(--text-primary)" }
      : { borderColor: "var(--border-default)", background: "var(--surface-secondary)", color: "var(--text-primary)" };
  })();

  return (
    <button
      type="button"
      data-no-drag
      onMouseEnter={onMouseEnter}
      onClick={onClick}
      className="flex w-full items-center justify-between rounded-[14px] border px-4 py-2.5 text-left transition-colors"
      style={toneStyle}
    >
      <span className="flex items-center gap-3">
        <span style={{ color: selected ? "var(--text-primary)" : "var(--text-secondary)" }}>{icon}</span>
        <span className="text-[13px]">{label}</span>
      </span>
      {shortcut ? (
        <span
          className="rounded-md px-2 py-0.5 text-[10px]"
          style={{ background: "var(--surface-primary)", color: "var(--text-tertiary)" }}
        >
          {shortcut}
        </span>
      ) : (
        <span className="w-8 shrink-0" />
      )}
    </button>
  );
}

function ImagePreview({ clipId, thumbnail }: { clipId: number; thumbnail: string | null }) {
  const [preview, setPreview] = useState<string | null>(() => getImagePreviewData(clipId) ?? null);

  useEffect(() => {
    let cancelled = false;
    const cached = getImagePreviewData(clipId);
    if (cached) {
      setPreview(cached);
      return () => {
        cancelled = true;
      };
    }

    setPreview(null);
    invoke<string | null>("get_image_preview", { clipId, maxSize: 400 })
      .then((result) => {
        if (cancelled) return;
        if (result) {
          setImagePreviewData(clipId, result);
        }
        setPreview(result);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [clipId]);

  const imgSrc = preview
    ? `data:image/png;base64,${preview}`
    : thumbnail
      ? `data:image/png;base64,${thumbnail}`
      : null;

  if (!imgSrc) return null;

  return (
    <div
      className="flex w-full items-center justify-center overflow-hidden rounded-lg"
      style={{
        maxHeight: "170px",
        background: "var(--surface-primary)",
        border: "0.5px solid var(--border-default)",
      }}
    >
      <img
        src={imgSrc}
        alt="Clipboard image"
        className="block h-auto w-auto max-w-full object-contain"
        style={{
          maxHeight: "170px",
        }}
      />
    </div>
  );
}

type SubMenu = "none" | "transform" | "collection";

function ActionsSheet({
  clip,
  clipContent,
  thumbnail,
  actions,
  selectedIndex,
  collections,
  onClose,
  onSelect,
  onActivate,
  onTransform,
  onMoveToCollection,
  onOpenUrl,
}: ActionsSheetProps) {
  const [subMenu, setSubMenu] = useState<SubMenu>("none");
  const [subMenuIndex, setSubMenuIndex] = useState(0);
  const mainListRef = useRef<HTMLDivElement>(null);
  const transformListRef = useRef<HTMLDivElement>(null);
  const collectionListRef = useRef<HTMLDivElement>(null);

  const isImage = clip.content_type === "image";
  const canTransform = !isImage;

  const allActions = useMemo<SheetAction[]>(() => {
    const items: SheetAction[] = [...actions];

    if (clip.content_type === "url") {
      items.splice(1, 0, {
        id: "open-url",
        icon: <ExternalLink size={16} />,
        label: "Open in Browser",
        shortcut: actionShortcut(isMacPlatform ? "cmd+o" : "ctrl+o"),
      });
    }

    if (canTransform) {
      items.push({
        id: "transform",
        icon: <Shuffle size={16} />,
        label: "Transform",
        shortcut: "",
      });
    }

    items.push({
      id: "move-collection",
      icon: <FolderOpen size={16} />,
      label: "Move to Collection",
      shortcut: "",
    });

    return items;
  }, [actions, canTransform, clip.content_type]);

  const handleTransformSelect = useCallback(
    (transformId: string) => {
      const t = transforms.find((tr) => tr.id === transformId);
      if (t && !isImage) {
        const transformed = t.fn(clipContent);
        onTransform(clip.id, transformed);
      }
      setSubMenu("none");
      onClose();
    },
    [clip.id, clipContent, isImage, onTransform, onClose]
  );

  const handleCollectionSelect = useCallback(
    (collectionId: number | null) => {
      onMoveToCollection(clip.id, collectionId);
      setSubMenu("none");
      onClose();
    },
    [clip.id, onMoveToCollection, onClose]
  );

  const handleMainActivate = useCallback(
    (actionIndex: number) => {
      const action = allActions[actionIndex];
      if (!action) return;

      switch (action.id) {
        case "transform":
          setSubMenu("transform");
          setSubMenuIndex(0);
          return;
        case "move-collection":
          setSubMenu("collection");
          setSubMenuIndex(0);
          return;
        case "open-url":
          onOpenUrl(clip.content);
          onClose();
          return;
        default:
          // Map through base action list so inserted URL-only actions
          // (e.g. "open-url") do not shift pin/copy/delete indices.
          {
            const baseIndex = actions.findIndex((baseAction) => baseAction.id === action.id);
            if (baseIndex >= 0) {
              onActivate(baseIndex);
            }
          }
      }
    },
    [actions, allActions, clip.content, onActivate, onClose, onOpenUrl]
  );

  useEffect(() => {
    if (subMenu !== "none") {
      return;
    }
    const container = mainListRef.current;
    if (!container) {
      return;
    }
    const selected = container.querySelector<HTMLElement>(`[data-action-index=\"${selectedIndex}\"]`);
    selected?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex, subMenu]);

  useEffect(() => {
    if (subMenu !== "transform") {
      return;
    }
    const container = transformListRef.current;
    if (!container) {
      return;
    }
    const selected = container.querySelector<HTMLElement>(`[data-submenu-index=\"${subMenuIndex}\"]`);
    selected?.scrollIntoView({ block: "nearest" });
  }, [subMenu, subMenuIndex]);

  useEffect(() => {
    if (subMenu !== "collection") {
      return;
    }
    const container = collectionListRef.current;
    if (!container) {
      return;
    }
    const selected = container.querySelector<HTMLElement>(`[data-submenu-index=\"${subMenuIndex}\"]`);
    selected?.scrollIntoView({ block: "nearest" });
  }, [subMenu, subMenuIndex]);

  useEffect(() => {
    if (subMenu === "none") {
      const clamped = Math.max(0, Math.min(selectedIndex, allActions.length - 1));
      if (clamped !== selectedIndex) {
        onSelect(clamped);
      }
    }
  }, [allActions.length, onSelect, selectedIndex, subMenu]);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (subMenu === "none") {
        if (allActions.length === 0) {
          return;
        }
        if (e.key === "ArrowDown" || e.key === "ArrowRight") {
          e.preventDefault();
          onSelect(wrapIndex(selectedIndex, 1, allActions.length));
          return;
        }

        if (e.key === "ArrowUp" || e.key === "ArrowLeft") {
          e.preventDefault();
          onSelect(wrapIndex(selectedIndex, -1, allActions.length));
          return;
        }

        if (e.key === "Enter" && !e.metaKey && !e.ctrlKey) {
          e.preventDefault();
          handleMainActivate(selectedIndex);
          return;
        }

        if (e.key === "Escape") {
          e.preventDefault();
          onClose();
          return;
        }

        return;
      }

      if (subMenu === "transform") {
        if (e.key === "Escape") {
          e.preventDefault();
          setSubMenu("none");
          return;
        }
        if (e.key === "ArrowDown" || e.key === "ArrowRight") {
          e.preventDefault();
          setSubMenuIndex((prev) => wrapIndex(prev, 1, transforms.length));
          return;
        }
        if (e.key === "ArrowUp" || e.key === "ArrowLeft") {
          e.preventDefault();
          setSubMenuIndex((prev) => wrapIndex(prev, -1, transforms.length));
          return;
        }
        if (e.key === "Enter" && !e.metaKey && !e.ctrlKey) {
          e.preventDefault();
          const selectedTransform = transforms[subMenuIndex];
          if (selectedTransform) {
            handleTransformSelect(selectedTransform.id);
          }
        }
        return;
      }

      if (subMenu === "collection") {
        const collectionCount = collections.length + 1;
        if (e.key === "Escape") {
          e.preventDefault();
          setSubMenu("none");
          return;
        }
        if (e.key === "ArrowDown" || e.key === "ArrowRight") {
          e.preventDefault();
          setSubMenuIndex((prev) => wrapIndex(prev, 1, collectionCount));
          return;
        }
        if (e.key === "ArrowUp" || e.key === "ArrowLeft") {
          e.preventDefault();
          setSubMenuIndex((prev) => wrapIndex(prev, -1, collectionCount));
          return;
        }
        if (e.key === "Enter" && !e.metaKey && !e.ctrlKey) {
          e.preventDefault();
          if (subMenuIndex === 0) {
            handleCollectionSelect(null);
          } else {
            const selectedCollection = collections[subMenuIndex - 1];
            if (selectedCollection) {
              handleCollectionSelect(selectedCollection.id);
            }
          }
        }
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [
    allActions.length,
    collections,
    handleCollectionSelect,
    handleMainActivate,
    handleTransformSelect,
    onClose,
    onSelect,
    selectedIndex,
    subMenu,
    subMenuIndex,
  ]);

  // Submenu view
  if (subMenu === "transform" && !isImage) {
    return (
      <div className="absolute inset-0 z-30 flex items-end justify-end p-4" style={{ background: "rgba(0,0,0,0.08)" }}>
        <button type="button" className="absolute inset-0 cursor-default" onClick={() => setSubMenu("none")} />
        <div
          data-no-drag
          className="relative flex w-full max-w-[348px] max-h-[calc(100%-2rem)] flex-col overflow-hidden rounded-[20px] border p-3 backdrop-blur-2xl"
          style={{
            background: "var(--overlay-bg)",
            borderColor: "var(--border-default)",
            boxShadow: "var(--overlay-shadow)",
          }}
        >
          <div className="mb-2 flex items-center justify-between px-1">
            <span className="text-[11px] uppercase tracking-[0.10em]" style={{ color: "var(--text-tertiary)" }}>
              Transform
            </span>
            <button
              type="button"
              data-no-drag
              onClick={() => setSubMenu("none")}
              className="rounded-full p-1"
              style={{ color: "var(--text-tertiary)" }}
            >
              <X size={12} />
            </button>
          </div>
          <div ref={transformListRef} className="min-h-0 flex-1 space-y-1 overflow-y-auto pr-1">
            {transforms.map((t, i) => (
              <button
                key={t.id}
                data-submenu-index={i}
                type="button"
                data-no-drag
                onMouseEnter={() => setSubMenuIndex(i)}
                onClick={() => handleTransformSelect(t.id)}
                className="flex w-full items-center rounded-lg px-3 py-2 text-left transition-colors"
                style={{
                  background: i === subMenuIndex ? "var(--accent-bg)" : "transparent",
                  color: i === subMenuIndex ? "var(--accent-text)" : "var(--text-secondary)",
                }}
              >
                <span className="text-[12px]">{t.label}</span>
              </button>
            ))}
          </div>
        </div>
      </div>
    );
  }

  if (subMenu === "collection") {
    return (
      <div className="absolute inset-0 z-30 flex items-end justify-end p-4" style={{ background: "rgba(0,0,0,0.08)" }}>
        <button type="button" className="absolute inset-0 cursor-default" onClick={() => setSubMenu("none")} />
        <div
          data-no-drag
          className="relative flex w-full max-w-[348px] max-h-[calc(100%-2rem)] flex-col overflow-hidden rounded-[20px] border p-3 backdrop-blur-2xl"
          style={{
            background: "var(--overlay-bg)",
            borderColor: "var(--border-default)",
            boxShadow: "var(--overlay-shadow)",
          }}
        >
          <div className="mb-2 flex items-center justify-between px-1">
            <span className="text-[11px] uppercase tracking-[0.10em]" style={{ color: "var(--text-tertiary)" }}>
              Move to Collection
            </span>
            <button
              type="button"
              data-no-drag
              onClick={() => setSubMenu("none")}
              className="rounded-full p-1"
              style={{ color: "var(--text-tertiary)" }}
            >
              <X size={12} />
            </button>
          </div>
          <div ref={collectionListRef} className="min-h-0 flex-1 space-y-1 overflow-y-auto pr-1">
            <button
              data-submenu-index={0}
              type="button"
              data-no-drag
              onClick={() => handleCollectionSelect(null)}
              className="flex w-full items-center gap-2 rounded-lg px-3 py-2 text-left transition-colors"
              style={{
                background: subMenuIndex === 0 ? "var(--accent-bg)" : "transparent",
                color: "var(--text-secondary)",
              }}
            >
              <span className="text-[12px]">No Collection</span>
            </button>
            {collections.map((col, i) => (
              <button
                key={col.id}
                data-submenu-index={i + 1}
                type="button"
                data-no-drag
                onMouseEnter={() => setSubMenuIndex(i + 1)}
                onClick={() => handleCollectionSelect(col.id)}
                className="flex w-full items-center gap-2 rounded-lg px-3 py-2 text-left transition-colors"
                style={{
                  background: i + 1 === subMenuIndex ? "var(--accent-bg)" : "transparent",
                  color: i + 1 === subMenuIndex ? "var(--accent-text)" : "var(--text-secondary)",
                }}
              >
                <FolderOpen size={12} style={{ color: col.color }} />
                <span className="text-[12px] flex-1">{col.name}</span>
                <span className="text-[10px]" style={{ color: "var(--text-muted)" }}>
                  {col.clip_count}
                </span>
              </button>
            ))}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="absolute inset-0 z-30 flex items-end justify-end p-4" style={{ background: "rgba(0,0,0,0.08)" }}>
      <button type="button" className="absolute inset-0 cursor-default" onClick={onClose} />
      <div
        data-no-drag
        className="relative flex w-full max-w-[348px] max-h-[calc(100%-2rem)] flex-col overflow-hidden rounded-[20px] border p-3 backdrop-blur-2xl"
        style={{
          background: "var(--overlay-bg)",
          borderColor: "var(--border-default)",
          boxShadow: "var(--overlay-shadow)",
        }}
      >
        {/* Header */}
        <div className="mb-3 shrink-0 rounded-[14px] p-3" style={{ background: "var(--surface-secondary)" }}>
          <div className="mb-2 flex items-start justify-between gap-3">
            <div className="min-w-0 flex-1">
              <div className="mb-1 text-[11px] uppercase tracking-[0.10em]" style={{ color: "var(--text-tertiary)" }}>
                Actions
              </div>

              {isImage && (
                <div className="mb-2">
                  <ImagePreview clipId={clip.id} thumbnail={thumbnail} />
                </div>
              )}

              <div className="line-clamp-2 text-[13px]" style={{ color: "var(--text-primary)" }}>
                {previewText(clip) || "Untitled clip"}
              </div>
              <div className="mt-1.5 flex items-center gap-2 text-[11px]" style={{ color: "var(--text-tertiary)" }}>
                <span>{normalizeAppName(clip.source_app || "") || "Unknown app"}</span>
                <span>·</span>
                <span>{clip.content_type}</span>
              </div>
            </div>
            <button
              type="button"
              data-no-drag
              onClick={onClose}
              className="rounded-full p-1.5 transition-colors"
              style={{ background: "var(--surface-primary)", color: "var(--text-secondary)" }}
            >
              <X size={14} />
            </button>
          </div>
          <div className="text-[11px]" style={{ color: "var(--text-tertiary)" }}>
            Use <span style={{ color: "var(--text-secondary)" }}>↑↓</span> to move,{" "}
            <span style={{ color: "var(--text-secondary)" }}>{formatShortcut("enter")}</span> to confirm
          </div>
        </div>

        {/* Actions */}
        <div ref={mainListRef} className="min-h-0 flex-1 space-y-1.5 overflow-y-auto pr-1">
          {allActions.map((action, index) => (
            <div key={action.id} data-action-index={index}>
              <ActionButton
                icon={action.icon}
                label={action.label}
                shortcut={action.shortcut}
                tone={action.tone}
                selected={index === selectedIndex}
                onMouseEnter={() => onSelect(index)}
                onClick={() => handleMainActivate(index)}
              />
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

export function buildSheetActions(clip: ClipResult): SheetAction[] {
  return [
    {
      id: "pin",
      icon: clip.pinned ? <PinOff size={16} /> : <Pin size={16} />,
      label: clip.pinned ? "Unpin Clip" : "Pin Clip",
      shortcut: actionShortcut(isMacPlatform ? "cmd+p" : "ctrl+p"),
    },
    {
      id: "copy",
      icon: <Copy size={16} />,
      label: "Copy to Clipboard",
      shortcut: actionShortcut("shift+enter"),
    },
    {
      id: "delete",
      icon: <Trash2 size={16} />,
      label: "Delete Entry",
      shortcut: actionShortcut(isMacPlatform ? "cmd+d" : "ctrl+d"),
      tone: "danger",
    },
  ];
}

export default ActionsSheet;
