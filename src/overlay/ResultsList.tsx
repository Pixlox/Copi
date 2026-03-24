import { useRef, useEffect, useCallback } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ClipResult } from "../hooks/useSearch";
import ResultRow from "./ResultRow";

interface ResultsListProps {
  results: ClipResult[];
  selectedIndex: number;
  totalCount: number;
  onSelect: (index: number) => void;
  onCopy: (index: number) => void;
}

function ResultsList({ results, selectedIndex, totalCount, onSelect, onCopy }: ResultsListProps) {
  const parentRef = useRef<HTMLDivElement>(null);

  const getRowHeight = useCallback(
    (index: number) => {
      const result = results[index];
      if (!result) return 48;
      if (result.content_type === "url") return 64;
      return 48;
    },
    [results]
  );

  const virtualizer = useVirtualizer({
    count: results.length,
    getScrollElement: () => parentRef.current,
    estimateSize: getRowHeight,
    overscan: 5,
  });

  // Scroll to selected item (in useEffect, not during render)
  const prevSelected = useRef(selectedIndex);
  useEffect(() => {
    if (selectedIndex !== prevSelected.current) {
      prevSelected.current = selectedIndex;
      virtualizer.scrollToIndex(selectedIndex, { align: "auto" });
    }
  }, [selectedIndex, virtualizer]);

  return (
    <div ref={parentRef} className="flex-1 min-h-0 overflow-y-auto">
      {results.length === 0 ? (
        <div className="flex h-full min-h-[280px] flex-col items-center justify-center gap-2 px-6 text-center">
          <div className="text-sm" style={{ color: "var(--text-tertiary)" }}>
            {totalCount === 0
              ? "No clips yet"
              : "No clips found"}
          </div>
          <div className="text-[11px]" style={{ color: "var(--text-muted)" }}>
            {totalCount === 0
              ? "Copy something to get started"
              : "Try a different search or filter"}
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
                  onClick={() => onSelect(virtualRow.index)}
                  onDoubleClick={() => onCopy(virtualRow.index)}
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
