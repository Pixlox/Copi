import { useState, useCallback, useMemo, useRef, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { useSearch, FilterType } from "../hooks/useSearch";
import { useKeyboard } from "../hooks/useKeyboard";
import ActionsSheet, { buildSheetActions } from "./ActionsSheet";
import SearchBar from "./SearchBar";
import ResultsList from "./ResultsList";
import StatusBar from "./StatusBar";
import CollectionSidebar from "./CollectionSidebar";
import DetailPanel from "./DetailPanel";
import { getClipIconData } from "./clipMediaCache";

const FILTERS: FilterType[] = ["all", "text", "url", "code", "image", "pinned"];

function Overlay() {
  const {
    query,
    setQuery,
    activeFilter,
    setActiveFilter,
    results,
    totalCount,
    collectionId,
    setCollectionId,
    collections,
    searchStatus,
    fetchCollections,
    optimisticDelete,
    optimisticTogglePin,
  } = useSearch();

  const [selectedIndex, setSelectedIndex] = useState(0);
  const [actionsOpen, setActionsOpen] = useState(false);
  const [selectedActionIndex, setSelectedActionIndex] = useState(0);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [detailOpen, setDetailOpen] = useState(false);
  const [defaultPasteBehaviour, setDefaultPasteBehaviour] = useState<"copy" | "paste">("paste");
  const [compactMode, setCompactMode] = useState(false);
  const [showAppIcons, setShowAppIcons] = useState(true);
  const [selectedClipFullContent, setSelectedClipFullContent] = useState<string | null>(null);

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
    setActionsOpen(false);
  }, [activeFilter, setActiveFilter]);

  useEffect(() => {
    let mounted = true;

    const loadConfig = async () => {
      try {
        const config = await invoke<{
          general: { default_paste_behaviour: string };
          appearance: { compact_mode: boolean; show_app_icons: boolean };
        }>("get_config");
        if (!mounted) {
          return;
        }
        setDefaultPasteBehaviour(
          config.general.default_paste_behaviour === "copy" ? "copy" : "paste"
        );
        setCompactMode(Boolean(config.appearance.compact_mode));
        setShowAppIcons(Boolean(config.appearance.show_app_icons));
      } catch (error) {
        console.error("Failed to load overlay config:", error);
      }
    };

    void loadConfig();

    const unlisten = listen<{
      general?: { default_paste_behaviour?: string };
      appearance?: { compact_mode?: boolean; show_app_icons?: boolean };
    }>("config-changed", (event) => {
      const payload = event.payload;
      const behaviour = payload.general?.default_paste_behaviour;
      if (behaviour) {
        setDefaultPasteBehaviour(behaviour === "copy" ? "copy" : "paste");
      }
      if (typeof payload.appearance?.compact_mode === "boolean") {
        setCompactMode(payload.appearance.compact_mode);
      }
      if (typeof payload.appearance?.show_app_icons === "boolean") {
        setShowAppIcons(payload.appearance.show_app_icons);
      }
    });

    return () => {
      mounted = false;
      unlisten.then((fn) => fn());
    };
  }, []);

  const handleDelete = useCallback(
    async (index: number) => {
      if (index < 0 || index >= results.length) return;
      const clipId = results[index].id;
      const rollback = optimisticDelete(clipId);
      setActionsOpen(false);
      try {
        await invoke("delete_clip", { clipId });
      } catch (error) {
        rollback();
        console.error("Delete failed:", error);
      }
    },
    [optimisticDelete, results]
  );

  const handlePin = useCallback(
    async (index: number) => {
      if (index < 0 || index >= results.length) return;
      const clipId = results[index].id;
      const rollback = optimisticTogglePin(clipId);
      setActionsOpen(false);
      try {
        await invoke("toggle_pin", { clipId });
      } catch (error) {
        rollback();
        console.error("Pin toggle failed:", error);
      }
    },
    [optimisticTogglePin, results]
  );

  const handleActions = useCallback(
    (index: number) => {
      if (index < 0 || index >= results.length) return;
      const shouldOpen = !actionsOpen || selectedIndex !== index;
      setSelectedIndex(index);
      setSelectedActionIndex(0);
      setActionsOpen(shouldOpen);
    },
    [actionsOpen, results.length, selectedIndex]
  );

  const handleEdit = useCallback(
    async (clipId: number, newContent: string) => {
      try {
        await invoke("update_clip_content", { clipId, newContent });
      } catch (error) {
        console.error("Edit failed:", error);
      }
    },
    []
  );

  const handleCreateCollection = useCallback(
    async (name: string, color: string) => {
      try {
        await invoke("create_collection", { name, color });
        await fetchCollections();
      } catch (error) {
        console.error("Create collection failed:", error);
      }
    },
    [fetchCollections]
  );

  const handleRenameCollection = useCallback(
    async (id: number, name: string) => {
      try {
        await invoke("rename_collection", { id, name });
        await fetchCollections();
      } catch (error) {
        console.error("Rename collection failed:", error);
      }
    },
    [fetchCollections]
  );

  const handleDeleteCollection = useCallback(
    async (id: number) => {
      try {
        await invoke("delete_collection", { id });
        await fetchCollections();
        if (collectionId === id) {
          setCollectionId(null);
        }
      } catch (error) {
        console.error("Delete collection failed:", error);
      }
    },
    [fetchCollections, collectionId, setCollectionId]
  );

  const selectedResult =
    selectedIndex >= 0 && selectedIndex < results.length ? results[selectedIndex] : null;
  const selectedClipContent = selectedClipFullContent ?? selectedResult?.content ?? "";
  const selectedMedia = selectedResult ? getClipIconData(selectedResult.id) : undefined;
  const actions = useMemo(
    () => (selectedResult ? buildSheetActions(selectedResult) : []),
    [selectedResult?.id, selectedResult?.pinned]
  );

  // Detail panel opens only via Cmd+I or explicit toggle — not on selection change

  const triggerAction = useCallback(
    (actionIndex: number) => {
      const action = actions[actionIndex];
      if (!action) return;

      switch (action.id) {
        case "pin":
          void handlePin(selectedIndex);
          break;
        case "copy":
          setActionsOpen(false);
          void handleCopy(selectedIndex);
          break;
        case "delete":
          void handleDelete(selectedIndex);
          break;
        default:
          break;
      }
    },
    [actions, handleCopy, handleDelete, handlePin, selectedIndex]
  );

  useKeyboard({
    resultCount: results.length,
    selectedIndex,
    actionsOpen,
    actionCount: actions.length,
    selectedActionIndex,
    onSelect: setSelectedIndex,
    onSelectAction: setSelectedActionIndex,
    onAction: triggerAction,
    onCopy: handleCopy,
    onPaste: handlePaste,
    onNumberCopy: handleNumberCopy,
    defaultEnterAction: defaultPasteBehaviour,
    onFilterCycle: handleFilterCycle,
    onDelete: handleDelete,
    onPin: handlePin,
    onCloseActions: () => setActionsOpen(false),
    onActions: handleActions,
    onToggleDetail: () => setDetailOpen((o) => !o),
  });

  useEffect(() => {
    if (selectedIndex >= results.length) {
      setSelectedIndex(Math.max(0, results.length - 1));
    }
  }, [results.length, selectedIndex]);

  useEffect(() => {
    if (!selectedResult) {
      setSelectedClipFullContent(null);
      return;
    }

    let cancelled = false;
    setSelectedClipFullContent(null);
    void invoke<string>("get_clip_full_content", { clipId: selectedResult.id })
      .then((content) => {
        if (!cancelled) {
          setSelectedClipFullContent(content);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setSelectedClipFullContent(selectedResult.content);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [selectedResult?.id]);

  useEffect(() => {
    setActionsOpen(false);
    setSelectedActionIndex(0);
  }, [activeFilter, query]);

  const toggleActions = useCallback(() => {
    if (!selectedResult) return;
    setSelectedActionIndex(0);
    setActionsOpen((open) => !open);
  }, [selectedResult]);

  // Drag handlers
  const onMouseDown = useCallback((e: React.MouseEvent) => {
    const target = e.target as HTMLElement;
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
      className="relative flex h-full min-h-0 overflow-hidden rounded-2xl border shadow-2xl animate-overlay-open"
      style={{ background: "var(--overlay-bg)", borderColor: "var(--overlay-border)" }}
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
    >
      {/* Sidebar */}
      {sidebarOpen && (
        <div className="w-[160px] shrink-0">
          <CollectionSidebar
            collections={collections}
            selectedId={collectionId}
            onSelect={(id) => {
              setCollectionId(id);
              setSelectedIndex(0);
              setActionsOpen(false);
            }}
            onCreate={handleCreateCollection}
            onRename={handleRenameCollection}
            onDelete={handleDeleteCollection}
          />
        </div>
      )}

      {/* Main content */}
      <div className="flex min-w-0 flex-1 flex-col">
        <SearchBar
          query={query}
          onQueryChange={(q) => {
            setQuery(q);
            setSelectedIndex(0);
            setActionsOpen(false);
          }}
          activeFilter={activeFilter}
          onFilterChange={(f) => {
            setActiveFilter(f);
            setSelectedIndex(0);
            setActionsOpen(false);
          }}
          sidebarOpen={sidebarOpen}
          onToggleSidebar={() => setSidebarOpen((o) => !o)}
        />

        <ResultsList
          results={results}
          selectedIndex={selectedIndex}
          totalCount={totalCount}
          query={query}
          onSelect={setSelectedIndex}
          onCopy={handleCopy}
          compactMode={compactMode}
          showAppIcons={showAppIcons}
        />

      <StatusBar
        totalCount={totalCount}
        query={query}
        searchStatus={searchStatus}
        actionsOpen={actionsOpen}
        canOpenActions={!!selectedResult}
        onToggleActions={toggleActions}
        />

      {actionsOpen && selectedResult && (
        <ActionsSheet
          clip={selectedResult}
          clipContent={selectedClipContent}
          thumbnail={selectedMedia?.thumbnail ?? null}
          actions={actions}
          selectedIndex={selectedActionIndex}
          collections={collections}
          onClose={() => setActionsOpen(false)}
          onSelect={setSelectedActionIndex}
          onActivate={(actionIndex) => triggerAction(actionIndex)}
          onTransform={(_clipId, transformedContent) => {
            void navigator.clipboard.writeText(transformedContent)
              .then(() => {
                setActionsOpen(false);
              })
              .catch((error) => {
                console.error("Transform copy failed:", error);
              });
          }}
          onMoveToCollection={(clipId, collectionId) => {
            void invoke("move_clip_to_collection", { clipId, collectionId });
          }}
          onOpenUrl={(url) => {
            void invoke("open_external_url", { url }).catch((error) => {
              console.error("Open URL failed:", error);
            });
          }}
        />
      )}
      </div>

      {/* Detail panel */}
      {detailOpen && selectedResult && (
        <div className="w-[280px] shrink-0">
          <DetailPanel
            clip={selectedResult}
            onClose={() => setDetailOpen(false)}
            onEdit={handleEdit}
            onCopy={(clipId) => {
              void invoke("copy_to_clipboard", { clipId });
            }}
            onOpenUrl={(url: string) => {
              void invoke("open_external_url", { url }).catch((error) => {
                console.error("Open URL failed:", error);
              });
            }}
          />
        </div>
      )}
    </div>
  );
}

export default Overlay;
