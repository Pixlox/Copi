import { useState, useCallback, useEffect, useRef, useDeferredValue } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface ClipResult {
  id: number;
  content: string;
  content_type: string;
  source_app: string;
  created_at: number;
  pinned: boolean;
  source_app_icon: string | null;
  content_highlighted: string | null;
  ocr_text: string | null;
  image_thumbnail: string | null;
  copy_count: number;
}

export interface CollectionInfo {
  id: number;
  name: string;
  color: string;
  clip_count: number;
  created_at: number;
}

export interface SearchStatus {
  phase: string;
  queuedItems: number;
  completedItems: number;
  totalItems: number;
  semanticReady: boolean;
}

interface SearchUpdatedEvent {
  query: string;
  filter: FilterType;
  collectionId: number | null;
  results: ClipResult[];
}

export type FilterType = "all" | "text" | "url" | "code" | "image" | "pinned";

export function useSearch() {
  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query);
  const [activeFilter, setActiveFilter] = useState<FilterType>("all");
  const [results, setResults] = useState<ClipResult[]>([]);
  const [totalCount, setTotalCount] = useState(0);
  const [collectionId, setCollectionId] = useState<number | null>(null);
  const [collections, setCollections] = useState<CollectionInfo[]>([]);
  const [searchStatus, setSearchStatus] = useState<SearchStatus>({
    phase: "idle",
    queuedItems: 0,
    completedItems: 0,
    totalItems: 0,
    semanticReady: false,
  });
  const requestIdRef = useRef(0);
  const resultsRef = useRef<ClipResult[]>([]);
  const totalCountRef = useRef(0);
  const refreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const applyResults = useCallback((nextResults: ClipResult[]) => {
    resultsRef.current = nextResults;
    setResults(nextResults);
  }, []);

  const applyTotalCount = useCallback((nextTotalCount: number) => {
    totalCountRef.current = nextTotalCount;
    setTotalCount(nextTotalCount);
  }, []);

  const fetchCollections = useCallback(async () => {
    try {
      const cols = await invoke<CollectionInfo[]>("list_collections");
      setCollections(cols);
    } catch (error) {
      console.error("Failed to list collections:", error);
    }
  }, []);

  const fetchResults = useCallback(async (searchQuery: string, filter: FilterType, colId: number | null) => {
    const requestId = ++requestIdRef.current;
    try {
      const clips = await invoke<ClipResult[]>("search_clips", {
        query: searchQuery,
        filter,
        collectionId: colId,
      });
      if (requestId !== requestIdRef.current) return;
      applyResults(clips);
    } catch (error) {
      console.error("Search failed:", error);
      // Don't clear results on error — keep previous results visible
    }
  }, [applyResults]);

  const fetchCount = useCallback(async () => {
    try {
      const count = await invoke<number>("get_total_clip_count");
      applyTotalCount(count);
    } catch (error) {
      console.error("Failed to get clip count:", error);
    }
  }, [applyTotalCount]);

  const fetchSearchStatus = useCallback(async () => {
    try {
      const status = await invoke<SearchStatus>("get_search_status");
      setSearchStatus(status);
    } catch (error) {
      console.error("Failed to get search status:", error);
    }
  }, []);

  // Store latest values in refs for the event listener
  const queryRef = useRef(query);
  const filterRef = useRef(activeFilter);
  const collectionIdRef = useRef(collectionId);
  queryRef.current = query;
  filterRef.current = activeFilter;
  collectionIdRef.current = collectionId;

  const scheduleRefresh = useCallback((includeCollections: boolean) => {
    if (refreshTimerRef.current) {
      clearTimeout(refreshTimerRef.current);
    }
    refreshTimerRef.current = setTimeout(() => {
      fetchResults(queryRef.current, filterRef.current, collectionIdRef.current);
      fetchCount();
      fetchSearchStatus();
      if (includeCollections) {
        fetchCollections();
      }
    }, 90);
  }, [fetchCollections, fetchCount, fetchResults, fetchSearchStatus]);

  // Debounced search
  useEffect(() => {
    const timer = setTimeout(() => {
      fetchResults(deferredQuery, activeFilter, collectionId);
    }, 140);

    return () => clearTimeout(timer);
  }, [deferredQuery, activeFilter, collectionId, fetchResults]);

  useEffect(() => {
    const unlistenNew = listen("new-clip", () => scheduleRefresh(false));
    const unlistenChanged = listen("clips-changed", () => scheduleRefresh(false));
    const unlistenCollections = listen("collections-changed", () => scheduleRefresh(true));

    return () => {
      if (refreshTimerRef.current) {
        clearTimeout(refreshTimerRef.current);
      }
      unlistenNew.then((fn) => fn());
      unlistenChanged.then((fn) => fn());
      unlistenCollections.then((fn) => fn());
    };
  }, [scheduleRefresh]);

  useEffect(() => {
    const unlisten = listen<SearchUpdatedEvent>("search-updated", (event) => {
      const payload = event.payload;
      if (
        payload.query !== queryRef.current ||
        payload.filter !== filterRef.current ||
        payload.collectionId !== collectionIdRef.current
      ) {
        return;
      }
      requestIdRef.current += 1;
      applyResults(payload.results);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [applyResults]);

  useEffect(() => {
    const unlisten = listen<SearchStatus>("search-status-updated", (event) => {
      setSearchStatus(event.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    fetchCount();
    fetchCollections();
    fetchSearchStatus();
  }, [fetchCount, fetchCollections, fetchSearchStatus]);

  const optimisticDelete = useCallback(
    (clipId: number) => {
      const previousResults = resultsRef.current;
      const previousCount = totalCountRef.current;
      const clipExists = previousResults.some((clip) => clip.id === clipId);
      const nextResults = previousResults.filter((clip) => clip.id !== clipId);

      requestIdRef.current += 1;
      applyResults(nextResults);
      if (clipExists) {
        applyTotalCount(Math.max(0, previousCount - 1));
      }

      return () => {
        requestIdRef.current += 1;
        applyResults(previousResults);
        applyTotalCount(previousCount);
      };
    },
    [applyResults, applyTotalCount]
  );

  const optimisticTogglePin = useCallback(
    (clipId: number) => {
      const previousResults = resultsRef.current;
      const nextResults = previousResults.flatMap((clip) => {
        if (clip.id !== clipId) {
          return [clip];
        }

        const nextClip = { ...clip, pinned: !clip.pinned };
        if (filterRef.current === "pinned" && !nextClip.pinned) {
          return [];
        }

        return [nextClip];
      });

      requestIdRef.current += 1;
      applyResults(nextResults);

      return () => {
        requestIdRef.current += 1;
        applyResults(previousResults);
      };
    },
    [applyResults]
  );

  return {
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
    refresh: () => fetchResults(queryRef.current, filterRef.current, collectionIdRef.current),
  };
}
