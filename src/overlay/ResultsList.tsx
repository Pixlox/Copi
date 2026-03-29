import { useRef, useEffect, useCallback, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ClipResult } from "../hooks/useSearch";
import ResultRow from "./ResultRow";
import {
  ClipIconData,
  getClipIconData,
  hasClipIconData,
  setClipIconData,
} from "./clipMediaCache";

interface ResultsListProps {
  results: ClipResult[];
  selectedIndex: number;
  totalCount: number;
  query: string;
  compactMode: boolean;
  showAppIcons: boolean;
  onSelect: (index: number) => void;
  onCopy: (index: number) => void;
}

function ResultsList({
  results,
  selectedIndex,
  totalCount,
  query,
  compactMode,
  showAppIcons,
  onSelect,
  onCopy,
}: ResultsListProps) {
  const parentRef = useRef<HTMLDivElement>(null);
  const [, forceUpdate] = useState(0);
  const pendingIconIdsRef = useRef(new Set<number>());

  const getRowHeight = useCallback(
    (index: number) => {
      const result = results[index];
      if (!result) return compactMode ? 40 : 48;
      if (compactMode) return 40;
      if (result.content_type === "url") return 64;
      return 48;
    },
    [compactMode, results]
  );

  const virtualizer = useVirtualizer({
    count: results.length,
    getScrollElement: () => parentRef.current,
    estimateSize: getRowHeight,
    overscan: 5,
  });
  const virtualItems = virtualizer.getVirtualItems();
  const visibleSignature = virtualItems.map((item) => item.index).join(",");

  // Scroll to selected item
  const prevSelected = useRef(selectedIndex);
  useEffect(() => {
    if (selectedIndex !== prevSelected.current) {
      prevSelected.current = selectedIndex;
      virtualizer.scrollToIndex(selectedIndex, { align: "auto" });
    }
  }, [selectedIndex, virtualizer]);

  // Batch-fetch icons for visible items
  useEffect(() => {
    const candidateIndices = new Set<number>();

    if (virtualItems.length > 0) {
      for (const item of virtualItems) {
        candidateIndices.add(item.index);
      }
    } else {
      const initialCount = Math.min(results.length, 18);
      for (let i = 0; i < initialCount; i += 1) {
        candidateIndices.add(i);
      }
    }

    if (selectedIndex >= 0 && selectedIndex < results.length) {
      candidateIndices.add(selectedIndex);
      if (selectedIndex + 1 < results.length) candidateIndices.add(selectedIndex + 1);
      if (selectedIndex - 1 >= 0) candidateIndices.add(selectedIndex - 1);
    }

    const prefetchStart = Math.max(0, selectedIndex - 20);
    const prefetchEnd = Math.min(results.length, selectedIndex + 80);
    for (let i = prefetchStart; i < prefetchEnd; i += 1) {
      candidateIndices.add(i);
    }

    const visibleIds = Array.from(candidateIndices)
      .map((index) => results[index]?.id)
      .filter((id): id is number => typeof id === "number");

    const missingIds = visibleIds.filter(
      (id) => !hasClipIconData(id) && !pendingIconIdsRef.current.has(id)
    );
    if (missingIds.length === 0) return;

    const requestIds = missingIds.slice(0, 96);

    for (const id of requestIds) {
      pendingIconIdsRef.current.add(id);
    }

    invoke<Record<string, ClipIconData>>("get_clip_icons_batch", {
      clipIds: requestIds,
    })
      .then((batch) => {
        let updated = false;
        for (const [idStr, data] of Object.entries(batch)) {
          const id = Number(idStr);
          setClipIconData(id, data);
          pendingIconIdsRef.current.delete(id);
          updated = true;
        }
        // Any requested IDs not returned should be retried later.
        for (const id of requestIds) {
          if (!(id in batch)) {
            pendingIconIdsRef.current.delete(id);
          }
        }
        if (updated) forceUpdate((n) => n + 1);
      })
      .catch(() => {
        // Clear pending state so a later render can retry.
        for (const id of requestIds) {
          pendingIconIdsRef.current.delete(id);
        }
      });
  }, [results, selectedIndex, visibleSignature]);

  return (
    <div ref={parentRef} className="flex-1 min-h-0 overflow-y-auto">
      {results.length === 0 ? (
        <div className="flex h-full min-h-[280px] flex-col items-center justify-center gap-2 px-6 text-center">
          <div className="text-sm" style={{ color: "var(--text-tertiary)" }}>
            {totalCount === 0 ? "No clips yet" : "No clips found"}
          </div>
          <div className="text-[11px]" style={{ color: "var(--text-muted)" }}>
            {totalCount === 0
              ? "Copy something to get started"
              : query.trim()
                ? `Try: 'yesterday', 'from slack', 'code', 'url', 'screenshot'`
                : "Start typing to search"}
          </div>
        </div>
      ) : (
        <div
          style={{
            height: `${virtualizer.getTotalSize()}px`,
            width: "100%",
            position: "relative",
          }}
        >
          {virtualizer.getVirtualItems().map((virtualRow) => {
            const result = results[virtualRow.index];
            const icons = getClipIconData(result.id);
            return (
              <div
                key={`${result.id}-${virtualRow.key}`}
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  height: `${virtualRow.size}px`,
                  transform: `translateY(${virtualRow.start}px)`,
                }}
              >
                <ResultRow
                  result={result}
                  isSelected={virtualRow.index === selectedIndex}
                  index={virtualRow.index}
                  query={query}
                  imageThumbnailData={icons?.thumbnail}
                  compactMode={compactMode}
                  showAppIcons={showAppIcons}
                  onClick={() => onSelect(virtualRow.index)}
                  onDoubleClick={() => onCopy(virtualRow.index)}
                  appIcon={icons?.app_icon ?? null}
                />
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

export default ResultsList;
