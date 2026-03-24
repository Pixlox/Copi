import { useState } from "react";
import { FolderOpen, Folder, Plus, X } from "lucide-react";
import type { CollectionInfo } from "../hooks/useSearch";

interface CollectionSidebarProps {
  collections: CollectionInfo[];
  selectedId: number | null;
  onSelect: (id: number | null) => void;
  onCreate: (name: string, color: string) => void;
  onRename: (id: number, name: string) => void;
  onDelete: (id: number) => void;
}

const COLLECTION_COLORS = [
  "#0A84FF", "#34C759", "#FF9500", "#FF3B30",
  "#AF52DE", "#FF2D55", "#5AC8FA", "#FFD60A",
];

export default function CollectionSidebar({
  collections,
  selectedId,
  onSelect,
  onCreate,
  onRename,
  onDelete,
}: CollectionSidebarProps) {
  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState("");
  const [renamingId, setRenamingId] = useState<number | null>(null);
  const [renameValue, setRenameValue] = useState("");

  const handleCreate = () => {
    if (!newName.trim()) return;
    const color = COLLECTION_COLORS[collections.length % COLLECTION_COLORS.length];
    onCreate(newName.trim(), color);
    setNewName("");
    setCreating(false);
  };

  const handleRename = (id: number) => {
    if (!renameValue.trim()) return;
    onRename(id, renameValue.trim());
    setRenamingId(null);
    setRenameValue("");
  };

  return (
    <div
      className="flex h-full flex-col py-2"
      style={{ borderRight: "0.5px solid var(--border-subtle)" }}
    >
      <div className="flex-1 overflow-y-auto px-2">
        {/* All Clips */}
        <button
          type="button"
          data-no-drag
          onClick={() => onSelect(null)}
          className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-[12px] transition-colors"
          style={{
            background: selectedId === null ? "var(--surface-active)" : "transparent",
            color: selectedId === null ? "var(--text-primary)" : "var(--text-secondary)",
          }}
        >
          <FolderOpen size={13} />
          All Clips
        </button>

        {/* Collections */}
        {collections.map((col) => (
          <div key={col.id} className="group relative">
            {renamingId === col.id ? (
              <div className="flex items-center gap-1 px-1 py-1">
                <input
                  type="text"
                  value={renameValue}
                  onChange={(e) => setRenameValue(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") handleRename(col.id);
                    if (e.key === "Escape") setRenamingId(null);
                  }}
                  onBlur={() => setRenamingId(null)}
                  autoFocus
                  className="flex-1 rounded px-1.5 py-0.5 text-[12px]"
                  style={{
                    background: "var(--settings-input-bg)",
                    color: "var(--text-primary)",
                    outline: "none",
                  }}
                />
              </div>
            ) : (
              <button
                type="button"
                data-no-drag
                onClick={() => onSelect(col.id)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setRenamingId(col.id);
                  setRenameValue(col.name);
                }}
                className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-[12px] transition-colors"
                style={{
                  background: selectedId === col.id ? "var(--surface-active)" : "transparent",
                  color: selectedId === col.id ? "var(--text-primary)" : "var(--text-secondary)",
                }}
              >
                <Folder size={13} style={{ color: col.color }} />
                <span className="flex-1 truncate text-left">{col.name}</span>
                <span
                  className="text-[10px]"
                  style={{ color: "var(--text-muted)" }}
                >
                  {col.clip_count}
                </span>
                <button
                  type="button"
                  data-no-drag
                  onClick={(e) => {
                    e.stopPropagation();
                    onDelete(col.id);
                  }}
                  className="hidden rounded p-0.5 group-hover:block"
                  style={{ color: "var(--text-tertiary)" }}
                >
                  <X size={10} />
                </button>
              </button>
            )}
          </div>
        ))}
      </div>

      {/* Create collection */}
      <div className="px-2 pb-1">
        {creating ? (
          <div className="flex items-center gap-1">
            <input
              type="text"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              placeholder="Collection name…"
              onKeyDown={(e) => {
                if (e.key === "Enter") handleCreate();
                if (e.key === "Escape") {
                  setCreating(false);
                  setNewName("");
                }
              }}
              autoFocus
              className="flex-1 rounded px-1.5 py-1 text-[11px]"
              style={{
                background: "var(--settings-input-bg)",
                color: "var(--text-primary)",
                outline: "none",
              }}
            />
          </div>
        ) : (
          <button
            type="button"
            data-no-drag
            onClick={() => setCreating(true)}
            className="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-[11px] transition-colors"
            style={{ color: "var(--text-tertiary)" }}
          >
            <Plus size={11} />
            New Collection
          </button>
        )}
      </div>
    </div>
  );
}
