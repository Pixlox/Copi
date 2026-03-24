import { useRef, useEffect, useCallback } from "react";
import { Search, X, PanelLeft } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { FilterType } from "../hooks/useSearch";

interface SearchBarProps {
  query: string;
  onQueryChange: (query: string) => void;
  activeFilter: FilterType;
  onFilterChange: (filter: FilterType) => void;
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
}

const FILTER_LABELS: Record<FilterType, string> = {
  all: "All",
  text: "Text",
  url: "URLs",
  code: "Code",
  image: "Images",
  pinned: "Pinned",
};

function SearchBar({ query, onQueryChange, activeFilter, onFilterChange, sidebarOpen, onToggleSidebar }: SearchBarProps) {
  const inputRef = useRef<HTMLInputElement>(null);

  const focusInput = useCallback(() => {
    const input = inputRef.current;
    if (!input) return;
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  }, []);

  useEffect(() => {
    focusInput();
    const onFocus = () => setTimeout(focusInput, 10);
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [focusInput]);

  useEffect(() => {
    const unlisten = listen("overlay:shown", () => {
      focusInput();
      window.setTimeout(focusInput, 30);
      window.setTimeout(focusInput, 120);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [focusInput]);

  const filters: FilterType[] = ["all", "text", "url", "code", "image", "pinned"];

  return (
    <div style={{ borderBottom: "1px solid var(--border-default)" }}>
      <div className="flex items-center gap-2 px-4 py-3">
        <button
          type="button"
          data-no-drag
          onClick={onToggleSidebar}
          className="rounded p-0.5 transition-colors"
          style={{
            color: sidebarOpen ? "var(--accent-text)" : "var(--text-tertiary)",
          }}
          title="Toggle sidebar"
        >
          <PanelLeft size={15} />
        </button>
        <Search size={16} style={{ color: "var(--text-tertiary)" }} className="shrink-0" />
        <input
          ref={inputRef}
          type="text"
          value={query}
          onChange={(e) => onQueryChange(e.target.value)}
          placeholder="Search your clipboard…"
          className="flex-1 bg-transparent outline-none text-[14px]"
          style={{ color: "var(--text-primary)" }}
          spellCheck={false}
          autoComplete="off"
          autoFocus
        />
        {query.length > 0 && (
          <button
            onClick={() => onQueryChange("")}
            className="p-0.5 rounded-full transition-colors"
            style={{ color: "var(--text-tertiary)" }}
          >
            <X size={14} />
          </button>
        )}
      </div>

      <div className="flex items-center gap-1 px-4 pb-2">
        {filters.map((filter) => (
          <button
            key={filter}
            onClick={() => onFilterChange(filter)}
            className={`filter-pill ${
              activeFilter === filter
                ? "active"
                : ""
            }`}
            style={activeFilter !== filter ? { color: "var(--text-tertiary)" } : undefined}
          >
            {FILTER_LABELS[filter]}
          </button>
        ))}
      </div>
    </div>
  );
}

export default SearchBar;
