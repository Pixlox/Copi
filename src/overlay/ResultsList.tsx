import { useRef, useEffect, useCallback } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ClipResult } from "../hooks/useSearch";
import ResultRow from "./ResultRow";

interface ResultsListProps {
  results: ClipResult[];
  selectedIndex: number;
  onSelect: (index: number) => void;
  onCopy: (index: number) => void;
}

function ResultsList({ results, selectedIndex, onSelect, onCopy }: ResultsListProps) {
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

  const prevSelected = useRef(selectedIndex);
  if (selectedIndex !== prevSelected.current) {
    prevSelected.current = selectedIndex;
    virtualizer.scrollToIndex(selectedIndex, { align: "auto" });
  }

  if (results.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center py-12 text-white/25 text-sm">
        No clips found
      </div>
    );
  }

  return (
    <div ref={parentRef} className="flex-1 overflow-y-auto" style={{ maxHeight: 420 }}>
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
              key={virtualRow.key}
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
    </div>
  );
}

export default ResultsList;
