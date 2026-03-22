import { useState, useCallback, useRef, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useSearch, FilterType } from "../hooks/useSearch";
import { useKeyboard } from "../hooks/useKeyboard";
import SearchBar from "./SearchBar";
import ResultsList from "./ResultsList";
import StatusBar from "./StatusBar";

const FILTERS: FilterType[] = ["all", "text", "url", "code", "image", "pinned"];

function Overlay() {
  const {
    query,
    setQuery,
    activeFilter,
    setActiveFilter,
    results,
    totalCount,
    refresh,
  } = useSearch();

  const [selectedIndex, setSelectedIndex] = useState(0);

  // Drag tracking state
  const dragStart = useRef<{ x: number; y: number } | null>(null);
  const isDragging = useRef(false);

  const handleCopy = useCallback(
    async (index: number) => {
      if (index >= 0 && index < results.length) {
        try {
          await invoke("copy_to_clipboard", { clipId: results[index].id });
          await invoke("hide_overlay", { paste: false });
        } catch (error) {
          console.error("Copy failed:", error);
        }
      }
    },
    [results]
  );

  const handlePaste = useCallback(
    async (index: number) => {
      if (index >= 0 && index < results.length) {
        try {
          await invoke("copy_to_clipboard", { clipId: results[index].id });
          await invoke("hide_overlay", { paste: true });
        } catch (error) {
          console.error("Paste failed:", error);
        }
      }
    },
    [results]
  );

  const handleNumberCopy = useCallback(
    async (resultIndex: number) => {
      await handleCopy(resultIndex);
    },
    [handleCopy]
  );

  const handleFilterCycle = useCallback(() => {
    const currentIndex = FILTERS.indexOf(activeFilter);
    const nextIndex = (currentIndex + 1) % FILTERS.length;
    setActiveFilter(FILTERS[nextIndex]);
    setSelectedIndex(0);
  }, [activeFilter, setActiveFilter]);

  const handleDelete = useCallback(async () => {}, []);
  const handlePin = useCallback(async () => {}, []);
  const handleActions = useCallback(() => {}, []);

  useKeyboard({
    resultCount: results.length,
    selectedIndex,
    onSelect: setSelectedIndex,
    onCopy: handleCopy,
    onPaste: handlePaste,
    onNumberCopy: handleNumberCopy,
    onFilterCycle: handleFilterCycle,
    onDelete: handleDelete,
    onPin: handlePin,
    onActions: handleActions,
  });

  // Reset selection when results change
  const prevResultsLen = useRef(results.length);
  if (results.length !== prevResultsLen.current) {
    prevResultsLen.current = results.length;
    if (selectedIndex >= results.length) {
      setSelectedIndex(Math.max(0, results.length - 1));
    }
  }

  // Drag handlers — track mouse movement and start drag after 3px threshold
  const onMouseDown = useCallback((e: React.MouseEvent) => {
    const target = e.target as HTMLElement;
    // Don't start drag on result rows or explicit no-drag elements
    if (target.closest("[data-no-drag]") || e.button !== 0) return;
    dragStart.current = { x: e.clientX, y: e.clientY };
    isDragging.current = false;
  }, []);

  const onMouseMove = useCallback((e: React.MouseEvent) => {
    if (!dragStart.current) return;
    const dx = e.clientX - dragStart.current.x;
    const dy = e.clientY - dragStart.current.y;
    if (!isDragging.current && (Math.abs(dx) > 3 || Math.abs(dy) > 3)) {
      isDragging.current = true;
      getCurrentWindow().startDragging();
    }
  }, []);

  const onMouseUp = useCallback(() => {
    dragStart.current = null;
    isDragging.current = false;
  }, []);

  return (
    <div
      className="flex flex-col h-full rounded-2xl overflow-hidden border border-white/[0.06] shadow-2xl animate-overlay-open"
      style={{ background: "rgba(25, 25, 28, 0.82)" }}
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
    >
      <SearchBar
        query={query}
        onQueryChange={(q) => {
          setQuery(q);
          setSelectedIndex(0);
        }}
        activeFilter={activeFilter}
        onFilterChange={(f) => {
          setActiveFilter(f);
          setSelectedIndex(0);
        }}
      />

      <ResultsList
        results={results}
        selectedIndex={selectedIndex}
        onSelect={setSelectedIndex}
        onCopy={handleCopy}
      />

      <StatusBar totalCount={totalCount} query={query} />
    </div>
  );
}

export default Overlay;
