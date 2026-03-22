import { useState, useCallback, useEffect, useRef } from "react";
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
}

export type FilterType = "all" | "text" | "url" | "code" | "image" | "pinned";

export function useSearch() {
  const [query, setQuery] = useState("");
  const [activeFilter, setActiveFilter] = useState<FilterType>("all");
  const [results, setResults] = useState<ClipResult[]>([]);
  const [isSearching, setIsSearching] = useState(false);
  const [totalCount, setTotalCount] = useState(0);

  const fetchResults = useCallback(async (searchQuery: string, filter: FilterType) => {
    setIsSearching(true);
    try {
      const clips = await invoke<ClipResult[]>("search_clips", {
        query: searchQuery,
        filter,
      });
      setResults(clips);
    } catch (error) {
      console.error("Search failed:", error);
      setResults([]);
    } finally {
      setIsSearching(false);
    }
  }, []);

  const fetchCount = useCallback(async () => {
    try {
      const count = await invoke<number>("get_total_clip_count");
      setTotalCount(count);
    } catch (error) {
      console.error("Failed to get clip count:", error);
    }
  }, []);

  // Store latest values in refs for the event listener
  const queryRef = useRef(query);
  const filterRef = useRef(activeFilter);
  queryRef.current = query;
  filterRef.current = activeFilter;

  // Debounced search
  useEffect(() => {
    const timer = setTimeout(() => {
      fetchResults(query, activeFilter);
    }, 80);

    return () => clearTimeout(timer);
  }, [query, activeFilter, fetchResults]);

  // Listen for new-clip events from clipboard watcher
  useEffect(() => {
    const unlisten = listen("new-clip", () => {
      // Refresh results and count
      fetchResults(queryRef.current, filterRef.current);
      fetchCount();
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [fetchResults, fetchCount]);

  // Listen for search-updated events from semantic search
  useEffect(() => {
    const unlisten = listen<ClipResult[]>("search-updated", (event) => {
      setResults(event.payload);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Fetch total count on mount
  useEffect(() => {
    fetchCount();
  }, [fetchCount]);

  return {
    query,
    setQuery,
    activeFilter,
    setActiveFilter,
    results,
    isSearching,
    totalCount,
    refresh: () => fetchResults(query, activeFilter),
  };
}
